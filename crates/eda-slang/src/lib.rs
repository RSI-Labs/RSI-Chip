//! Optional [slang](https://github.com/MikePopoloski/slang) front-end adapter.
//!
//! slang owns the commercial-strength lexer, preprocessor, parser, type
//! checker, and elaborator.  This crate first asks slang to validate the
//! source files, then asks it for a single preprocessed compilation unit and
//! lowers that unit through the repository HDL elaborator.  Keeping the
//! validated/preprocessed boundary explicit avoids silently accepting source
//! that the downstream bounded elaborator cannot represent.

use eda_core::ir::Design;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct SlangFrontend {
    program: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct PreprocessOptions {
    pub include_directories: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub undefines: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangFailure {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl std::fmt::Display for SlangFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "slang failed with status {:?}", self.status)?;
        if !self.stdout.trim().is_empty() {
            write!(f, "\nstdout:\n{}", self.stdout)?;
        }
        if !self.stderr.trim().is_empty() {
            write!(f, "\nstderr:\n{}", self.stderr)?;
        }
        Ok(())
    }
}

impl std::error::Error for SlangFailure {}

impl SlangFrontend {
    /// Finds slang through `SLANG_BIN`, `SLANG`, or `PATH`.
    pub fn discover() -> Result<Self, String> {
        if let Some(path) = std::env::var_os("SLANG_BIN") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(Self { program: path });
            }
            return Err(format!(
                "SLANG_BIN does not point to slang: {}",
                path.display()
            ));
        }
        if let Some(path) = std::env::var_os("SLANG") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(Self { program: path });
            }
            return Err(format!("SLANG does not point to slang: {}", path.display()));
        }
        which("slang").map(|program| Self { program })
    }

    /// Used by tests so machines without slang can still run the workspace.
    pub fn discover_optional() -> Option<Self> {
        Self::discover().ok()
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Runs slang's full parse/type/elaboration action.
    pub fn check_files(&self, files: &[PathBuf], top: Option<&str>) -> Result<(), SlangFailure> {
        self.check_files_with_options(files, top, &PreprocessOptions::default())
    }

    /// Runs slang's full action with the same include/define environment used
    /// for preprocessing.
    pub fn check_files_with_options(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<(), SlangFailure> {
        let mut command = self.command();
        if let Some(top) = top {
            command.arg("--top").arg(top);
        }
        add_preprocessor_options(&mut command, options);
        command.args(files);
        self.run(command)
    }

    pub fn preprocess_files(
        &self,
        files: &[PathBuf],
        options: &PreprocessOptions,
    ) -> Result<String, SlangFailure> {
        let mut command = self.command();
        command.arg("--preprocess").arg("--single-unit");
        add_preprocessor_options(&mut command, options);
        command.args(files);
        let output = self.run_capturing(command)?;
        String::from_utf8(output.stdout).map_err(|error| SlangFailure {
            status: None,
            stdout: String::from_utf8_lossy(&error.into_bytes()).into_owned(),
            stderr: "slang preprocessed output was not UTF-8".into(),
        })
    }

    /// Returns slang's post-elaboration AST JSON.
    pub fn ast_json(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
    ) -> Result<String, SlangFailure> {
        let mut command = self.command();
        if let Some(top) = top {
            command.arg("--top").arg(top);
        }
        command.arg("--ast-json").arg("-");
        for scope in scopes {
            command.arg("--ast-json-scope").arg(scope);
        }
        command.args(files);
        let output = self.run_capturing(command)?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let start = text.find('{').unwrap_or(0);
        Ok(text[start..].to_string())
    }

    /// Checks source with slang, preprocesses it, and lowers the result.
    pub fn compile_files(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        self.check_files_with_options(files, top, options)
            .map_err(|error| error.to_string())?;
        let preprocessed = self
            .preprocess_files(files, options)
            .map_err(|error| error.to_string())?;
        eda_hdl::compile(&preprocessed, top)
    }

    /// Convenience wrapper for an in-memory source text.
    pub fn compile_source(
        &self,
        source: &str,
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        let path = write_temporary_source(source)?;
        let result = self.compile_files(std::slice::from_ref(&path), top, options);
        let _ = std::fs::remove_file(&path);
        result
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("--quiet");
        command
    }

    fn run(&self, command: Command) -> Result<(), SlangFailure> {
        self.run_capturing(command).map(|_| ())
    }

    fn run_capturing(&self, mut command: Command) -> Result<std::process::Output, SlangFailure> {
        command
            .output()
            .map_err(|error| SlangFailure {
                status: None,
                stdout: String::new(),
                stderr: format!("cannot execute {}: {error}", self.program.display()),
            })
            .and_then(|output| {
                if output.status.success() {
                    Ok(output)
                } else {
                    Err(SlangFailure {
                        status: output.status.code(),
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    })
                }
            })
    }
}

fn add_preprocessor_options(command: &mut Command, options: &PreprocessOptions) {
    for directory in &options.include_directories {
        command.arg("-I").arg(directory);
    }
    for define in &options.defines {
        command.arg("-D").arg(define);
    }
    for undefine in &options.undefines {
        command.arg("-U").arg(undefine);
    }
}

fn which(program: &str) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("could not find '{program}' on PATH; set SLANG_BIN"))
}

fn write_temporary_source(source: &str) -> Result<PathBuf, String> {
    let unique = format!(
        "eda-slang-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0)
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::write(&path, source).map_err(|error| {
        format!(
            "cannot create temporary slang source {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}
