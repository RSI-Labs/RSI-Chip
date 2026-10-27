//! Optional [slang](https://github.com/MikePopoloski/slang) front-end adapter.
//!
//! slang owns the commercial-strength lexer, preprocessor, parser, type
//! checker, and elaborator.  This crate first asks slang to validate the
//! source files, then asks it for a single preprocessed compilation unit and
//! lowers that unit through the repository HDL elaborator.  Keeping the
//! validated/preprocessed boundary explicit avoids silently accepting source
//! that the downstream bounded elaborator cannot represent.

use eda_core::ir::Design;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

mod ast_util;
mod lowering;
mod summary;
mod types;

use lowering::{
    annotate_interface_array_port_connections, interface_array_port_member_count,
    interface_array_port_plan_has_members, interface_array_port_scope_paths, lower_root,
    parse_ast_root, plan_interface_array_port_scopes,
};
pub use summary::{
    SlangAssignmentSummary, SlangAstSummary, SlangContinuousTarget, SlangDeclarationSummary,
    SlangEnumMemberSummary, SlangEnumTypeSummary, SlangExpressionSummary, SlangInstanceSummary,
    SlangParameterSummary, SlangPortSummary, SlangProceduralBlockSummary, SlangSourceSpan,
};

#[derive(Clone, Debug)]
pub struct SlangFrontend {
    program: PathBuf,
    timeout: Duration,
}

const PINNED_SLANG_VERSION: &str = "slang version 12.0.535+54c19b53";
const DEFAULT_SLANG_TIMEOUT: Duration = Duration::from_secs(600);

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

/// A shallow, typed view over slang's elaborated AST JSON.
///
/// This is not a full AST lowering layer yet. It is the first validation
/// boundary: after the bounded repository elaborator runs, its top-module,
/// ports, simple declarations, and direct-instance inventory are checked
/// against slang's authoritative elaboration so silent drops cannot masquerade
/// as successful compilation.
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
        let program = self::discover_program()?;
        let frontend = Self {
            program,
            timeout: timeout_from_env()?,
        };
        let version = frontend.version()?;
        if !frontend.version_matches(&version) {
            return Err(format!(
                "unsupported slang runtime: expected '{PINNED_SLANG_VERSION}', found '{version}'"
            ));
        }
        Ok(frontend)
    }

    /// Used by tests so machines without slang can still run the workspace.
    pub fn discover_optional() -> Option<Self> {
        Self::discover().ok()
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    pub fn version(&self) -> Result<String, String> {
        let mut command = Command::new(&self.program);
        command.arg("--version");
        let output = self
            .run_capturing(command)
            .map_err(|failure| failure.to_string())?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if version.starts_with("slang version ") {
            Ok(version)
        } else {
            Err(format!(
                "unrecognized slang version output from {}: {version}",
                self.program.display()
            ))
        }
    }

    fn version_matches(&self, version: &str) -> bool {
        version.trim() == PINNED_SLANG_VERSION
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("--quiet");
        command
    }
}

fn discover_program() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("SLANG_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "SLANG_BIN does not point to slang: {}",
            path.display()
        ));
    }
    if let Some(path) = std::env::var_os("SLANG") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("SLANG does not point to slang: {}", path.display()));
    }
    which("slang")
}

impl SlangFrontend {
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
        self.ast_json_with_options(files, top, scopes, &PreprocessOptions::default())
    }

    /// Returns slang's post-elaboration AST JSON using the same include and
    /// macro environment as compilation.
    pub fn ast_json_with_options(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
        options: &PreprocessOptions,
    ) -> Result<String, SlangFailure> {
        let mut command = self.command();
        if let Some(top) = top {
            command.arg("--top").arg(top);
        }
        command.arg("--ast-json").arg("-");
        command.arg("--ast-json-detailed-types");
        command.arg("--ast-json-source-info");
        for scope in scopes {
            command.arg("--ast-json-scope").arg(scope);
        }
        add_preprocessor_options(&mut command, options);
        command.args(files);
        let output = self.run_capturing(command)?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let start = text.find('{').unwrap_or(0);
        Ok(text[start..].to_string())
    }

    /// Parses slang's AST JSON into the shallow authoritative summary used by
    /// post-lowering structural validation.
    pub fn summarize_ast_json(&self, json: &str) -> Result<SlangAstSummary, String> {
        SlangAstSummary::parse(json)
    }

    /// Compiles through the authoritative direct slang elaborated-AST path.
    ///
    /// This is the default `eda-slang` executable path. Unsupported constructs
    /// fail closed; they do not silently fall back to reparsing preprocessed
    /// source text.
    pub fn compile_files(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        self.compile_files_via_ast(files, top, options)
    }

    /// Checks source with slang, preprocesses it, and lowers the result through
    /// the bounded compatibility elaborator.
    ///
    /// This path is retained only for legacy differentials and explicit callers.
    /// New executable paths must use the direct AST path.
    pub fn compile_files_bounded(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        self.check_files_with_options(files, top, options)
            .map_err(|error| error.to_string())?;
        let authoritative_ast = self
            .ast_json_with_options(files, top, &[], options)
            .map_err(|error| error.to_string())?;
        let authoritative = self.summarize_ast_json(&authoritative_ast)?;
        let preprocessed = self
            .preprocess_files(files, options)
            .map_err(|error| error.to_string())?;
        let source_dir = files.first().and_then(|file| file.parent());
        let design = eda_hdl::compile_with_source_dir(&preprocessed, top, source_dir)?;
        authoritative.validate_against_design_legacy_compatibility(&design)?;
        Ok(design)
    }

    /// Compiles through slang and lowers a deliberately small subset directly
    /// from slang's elaborated AST JSON.
    ///
    /// Unsupported constructs return explicit errors; this path never silently
    /// falls back to preprocessing and reparsing the source text.
    pub fn compile_files_via_ast(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        self.check_files_with_options(files, top, options)
            .map_err(|error| error.to_string())?;
        let json = self
            .ast_json_with_options(files, top, &[], options)
            .map_err(|error| error.to_string())?;
        self.lower_ast_json_with_interface_array_scopes(&json, files, top, options)
    }

    /// Directly lowers the current simple-module AST subset into `Design`.
    pub fn lower_ast_json(&self, json: &str) -> Result<Design, String> {
        let root = parse_ast_root(json)?;
        let mut design = lower_root(&root)?;
        design.infer_memory_kinds();
        let authoritative = SlangAstSummary::parse(&root.to_string())?;
        authoritative.validate_against_design(&design)?;
        Ok(design)
    }

    fn lower_ast_json_with_interface_array_scopes(
        &self,
        json: &str,
        files: &[PathBuf],
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Result<Design, String> {
        let mut root = parse_ast_root(json)?;
        let mut plan = plan_interface_array_port_scopes(&root)?;
        if interface_array_port_plan_has_members(&plan) {
            let top_name = top
                .map(str::to_string)
                .or_else(|| {
                    root.get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .ok_or_else(|| "slang AST Instance has no name".to_string())?;
            let mut scopes = vec![top_name];
            scopes.extend(interface_array_port_scope_paths(&plan));
            let scoped_json = self
                .ast_json_with_options(files, top, &scopes, options)
                .map_err(|error| error.to_string())?;
            let scoped_objects = parse_concatenated_json_objects(&scoped_json)?;
            if scoped_objects.len() != scopes.len() {
                return Err(format!(
                    "slang emitted {} interface-array AST scopes but {} were expected",
                    scoped_objects.len(),
                    scopes.len()
                ));
            }
            let addresses = scoped_objects[1..]
                .iter()
                .map(|object| {
                    object
                        .get("addr")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| {
                            "slang interface-array member scope has no address".to_string()
                        })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let scoped_root_json = scoped_objects[0].to_string();
            root = parse_ast_root(&scoped_root_json)?;
            plan = plan_interface_array_port_scopes(&root)?;
            if scoped_objects.len() - 1 != interface_array_port_member_count(&plan) * 2 {
                return Err("slang interface-array scope plan changed between invocations".into());
            }
            annotate_interface_array_port_connections(&mut root, &plan, &addresses)?;
        }

        let mut design = lower_root(&root)?;
        design.infer_memory_kinds();
        let authoritative = SlangAstSummary::parse(&root.to_string())?;
        authoritative.validate_against_design(&design)?;
        Ok(design)
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

    fn run(&self, command: Command) -> Result<(), SlangFailure> {
        self.run_capturing(command).map(|_| ())
    }

    fn run_capturing(&self, mut command: Command) -> Result<std::process::Output, SlangFailure> {
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|error| SlangFailure {
            status: None,
            stdout: String::new(),
            stderr: format!("cannot execute {}: {error}", self.program.display()),
        })?;
        let mut child_stdout = child.stdout.take();
        let mut child_stderr = child.stderr.take();
        let stdout = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(stdout) = child_stdout.as_mut() {
                stdout.read_to_end(&mut buffer)?;
            }
            Ok::<_, std::io::Error>(buffer)
        });
        let stderr = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            if let Some(stderr) = child_stderr.as_mut() {
                stderr.read_to_end(&mut buffer)?;
            }
            Ok::<_, std::io::Error>(buffer)
        });

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(SlangFailure {
                            status: None,
                            stdout: String::new(),
                            stderr: format!(
                                "slang command '{}' exceeded its {:?} timeout",
                                self.program.display(),
                                self.timeout
                            ),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(SlangFailure {
                        status: None,
                        stdout: String::new(),
                        stderr: format!(
                            "cannot wait for slang command {}: {error}",
                            self.program.display()
                        ),
                    });
                }
            }
        };

        let stdout = stdout
            .join()
            .map_err(|_| SlangFailure {
                status: None,
                stdout: String::new(),
                stderr: "slang stdout reader panicked".into(),
            })
            .and_then(|result| {
                result.map_err(|error| SlangFailure {
                    status: None,
                    stdout: String::new(),
                    stderr: format!("cannot read slang stdout: {error}"),
                })
            })?;
        let stderr = stderr
            .join()
            .map_err(|_| SlangFailure {
                status: None,
                stdout: String::new(),
                stderr: "slang stderr reader panicked".into(),
            })
            .and_then(|result| {
                result.map_err(|error| SlangFailure {
                    status: None,
                    stdout: String::new(),
                    stderr: format!("cannot read slang stderr: {error}"),
                })
            })?;

        if status.success() {
            Ok(std::process::Output {
                status,
                stdout,
                stderr,
            })
        } else {
            Err(SlangFailure {
                status: status.code(),
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        }
    }
}

fn timeout_from_env() -> Result<Duration, String> {
    let Some(value) = std::env::var_os("SLANG_TIMEOUT_SECONDS") else {
        return Ok(DEFAULT_SLANG_TIMEOUT);
    };
    let seconds = value
        .to_string_lossy()
        .parse::<u64>()
        .map_err(|_| "SLANG_TIMEOUT_SECONDS must be a positive integer".to_string())?;
    if seconds == 0 {
        return Err("SLANG_TIMEOUT_SECONDS must be a positive integer".into());
    }
    Ok(Duration::from_secs(seconds))
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

fn parse_concatenated_json_objects(text: &str) -> Result<Vec<serde_json::Value>, String> {
    let mut objects = Vec::new();
    let mut object_start = None;
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    object_start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    return Err("slang AST scope output had an unmatched object close".into());
                }
                depth -= 1;
                if depth == 0 {
                    let start = object_start
                        .ok_or_else(|| "slang AST scope object had no start".to_string())?;
                    let object = serde_json::from_str(&text[start..=index])
                        .map_err(|error| format!("invalid slang AST scope JSON: {error}"))?;
                    objects.push(object);
                    object_start = None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("slang AST scope output ended inside an object".into());
    }
    Ok(objects)
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
    static NEXT_UNIQUE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let unique = format!(
        "eda-slang-{}-{}-{}.sv",
        std::process::id(),
        NEXT_UNIQUE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slang_version_matching_is_exact() {
        let frontend = SlangFrontend {
            program: PathBuf::from("slang"),
            timeout: DEFAULT_SLANG_TIMEOUT,
        };
        assert!(frontend.version_matches("slang version 12.0.535+54c19b53"));
        assert!(frontend.version_matches("slang version 12.0.535+54c19b53\n"));
        assert!(!frontend.version_matches("slang version 13.0.0"));
        assert!(!frontend.version_matches("slang version 12.0.535"));
    }

    #[cfg(unix)]
    #[test]
    fn slang_commands_fail_closed_on_timeout() {
        let frontend = SlangFrontend {
            program: PathBuf::from("/bin/sleep"),
            timeout: Duration::from_millis(50),
        };
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let error = frontend
            .run_capturing(command)
            .expect_err("a hung slang process must time out");
        assert_eq!(
            error.stderr,
            "slang command '/bin/sleep' exceeded its 50ms timeout"
        );
    }
}
