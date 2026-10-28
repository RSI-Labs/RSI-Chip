//! Optional [slang](https://github.com/MikePopoloski/slang) front-end adapter.
//!
//! slang owns the commercial-strength lexer, preprocessor, parser, type
//! checker, and elaborator.  This crate first asks slang to validate the
//! source files, then asks it for a single preprocessed compilation unit and
//! lowers that unit through the repository HDL elaborator.  Keeping the
//! validated/preprocessed boundary explicit avoids silently accepting source
//! that the downstream bounded elaborator cannot represent.

use eda_core::ir::Design;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod ast_util;
mod lowering;
mod summary;
mod types;

pub const SUPPORTED_SLANG_AST_SCHEMA: &str = lowering::SUPPORTED_SLANG_AST_SCHEMA;

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
    ast_cache: Arc<Mutex<SlangAstCache>>,
}

const PINNED_SLANG_VERSION: &str = "slang version 12.0.535+54c19b53";
const DEFAULT_SLANG_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_AST_CACHE_ENTRIES: usize = 4;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SlangNamedSourceCacheKey {
    path: PathBuf,
    content: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SlangAstCacheInput {
    Source {
        source: String,
        dependencies: Vec<SlangNamedSourceCacheKey>,
    },
    NamedFiles(Vec<SlangNamedSourceCacheKey>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SlangAstCacheKey {
    program: String,
    top: Option<String>,
    include_directories: Vec<PathBuf>,
    defines: Vec<String>,
    undefines: Vec<String>,
    library_directories: Vec<PathBuf>,
    library_extensions: Vec<String>,
    scopes: Vec<String>,
    input: SlangAstCacheInput,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SlangAstCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub entries: usize,
}

#[derive(Debug)]
struct SlangAstCache {
    capacity: usize,
    entries: BTreeMap<SlangAstCacheKey, Arc<String>>,
    order: VecDeque<SlangAstCacheKey>,
    hits: u64,
    misses: u64,
    insertions: u64,
    evictions: u64,
}

impl SlangAstCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
            insertions: 0,
            evictions: 0,
        }
    }

    fn get(&mut self, key: &SlangAstCacheKey) -> Option<Arc<String>> {
        let result = self.entries.get(key).cloned();
        if result.is_some() {
            if let Some(index) = self.order.iter().position(|entry| entry == key) {
                let entry = self.order.remove(index).expect("checked LRU entry exists");
                self.order.push_back(entry);
            }
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        result
    }

    fn insert(&mut self, key: SlangAstCacheKey, value: Arc<String>) {
        if self.capacity == 0 {
            return;
        }
        if self.entries.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
            self.insertions += 1;
        } else if let Some(index) = self.order.iter().position(|entry| *entry == key) {
            let entry = self.order.remove(index).expect("checked LRU entry exists");
            self.order.push_back(entry);
        }
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
            self.evictions += 1;
        }
    }

    fn stats(&self) -> SlangAstCacheStats {
        SlangAstCacheStats {
            hits: self.hits,
            misses: self.misses,
            insertions: self.insertions,
            evictions: self.evictions,
            entries: self.entries.len(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PreprocessOptions {
    pub include_directories: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub undefines: Vec<String>,
    pub library_directories: Vec<PathBuf>,
    pub library_extensions: Vec<String>,
}

impl PreprocessOptions {
    fn uses_libraries(&self) -> bool {
        !self.library_directories.is_empty() || !self.library_extensions.is_empty()
    }
}

/// A fail-closed JSON project manifest for a small file-list/library subset.
///
/// Paths are interpreted relative to the manifest file. Unknown fields,
/// heterogeneous arrays, empty paths, and extensions without a leading dot are
/// rejected rather than silently ignored.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SlangProjectManifest {
    pub top: Option<String>,
    pub sources: Vec<PathBuf>,
    pub include_directories: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub undefines: Vec<String>,
    pub library_directories: Vec<PathBuf>,
    pub library_extensions: Vec<String>,
}

impl SlangProjectManifest {
    pub fn parse_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read project manifest {}: {error}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        Self::parse(&text, base)
    }

    pub fn parse(text: &str, base_directory: &Path) -> Result<Self, String> {
        let root = serde_json::from_str::<serde_json::Value>(text)
            .map_err(|error| format!("invalid project manifest JSON: {error}"))?;
        let object = root
            .as_object()
            .ok_or("project manifest must be an object")?;
        let mut manifest = Self::default();
        let mut recognized = std::collections::BTreeSet::new();
        for (name, value) in object {
            match name.as_str() {
                "top" => {
                    manifest.top = match value {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(top) => Some(top.clone()),
                        _ => return Err("project top must be a string or null".into()),
                    };
                    if manifest.top.as_deref() == Some("") {
                        return Err("project top must not be empty".into());
                    }
                }
                "sources" => manifest.sources = parse_path_array(value, base_directory, "sources")?,
                "include_directories" => {
                    manifest.include_directories =
                        parse_path_array(value, base_directory, "include_directories")?
                }
                "defines" => manifest.defines = parse_string_array(value, "defines", false)?,
                "undefines" => manifest.undefines = parse_string_array(value, "undefines", false)?,
                "library_directories" => {
                    manifest.library_directories =
                        parse_path_array(value, base_directory, "library_directories")?
                }
                "library_extensions" => {
                    manifest.library_extensions =
                        parse_string_array(value, "library_extensions", true)?
                }
                other => return Err(format!("unknown project manifest field '{other}'")),
            }
            recognized.insert(name.clone());
        }
        if !recognized.contains("sources") || manifest.sources.is_empty() {
            return Err("project manifest requires a nonempty sources array".into());
        }
        Ok(manifest)
    }

    pub fn preprocess_options(&self) -> PreprocessOptions {
        PreprocessOptions {
            include_directories: self.include_directories.clone(),
            defines: self.defines.clone(),
            undefines: self.undefines.clone(),
            library_directories: self.library_directories.clone(),
            library_extensions: self.library_extensions.clone(),
        }
    }
}

fn parse_path_array(
    value: &serde_json::Value,
    base_directory: &Path,
    field: &str,
) -> Result<Vec<PathBuf>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    values
        .iter()
        .map(|value| {
            let text = value
                .as_str()
                .ok_or_else(|| format!("{field} entries must be strings"))?;
            if text.is_empty() {
                return Err(format!("{field} entries must not be empty"));
            }
            let path = Path::new(text);
            Ok(if path.is_absolute() {
                path.to_path_buf()
            } else {
                base_directory.join(path)
            })
        })
        .collect()
}

fn parse_string_array(
    value: &serde_json::Value,
    field: &str,
    require_leading_dot: bool,
) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    values
        .iter()
        .map(|value| {
            let text = value
                .as_str()
                .ok_or_else(|| format!("{field} entries must be strings"))?;
            if text.is_empty() {
                return Err(format!("{field} entries must not be empty"));
            }
            if require_leading_dot && !text.starts_with('.') {
                return Err(format!("{field} entries must begin with '.'"));
            }
            Ok(text.to_string())
        })
        .collect()
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
            ast_cache: Arc::new(Mutex::new(SlangAstCache::new(
                ast_cache_capacity_from_env()?
            ))),
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

    pub fn ast_cache_stats(&self) -> Result<SlangAstCacheStats, String> {
        self.ast_cache
            .lock()
            .map(|cache| cache.stats())
            .map_err(|_| "slang AST cache mutex poisoned".to_string())
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
        let cache_key = self.named_file_cache_key(files, top, scopes, options);
        let cached_ast = cache_key
            .as_ref()
            .map(|key| {
                self.ast_cache
                    .lock()
                    .map(|mut cache| cache.get(key))
                    .map_err(|_| self.cache_poisoned_failure())
            })
            .transpose()?
            .flatten();
        if let Some(json) = cached_ast {
            return Ok((*json).clone());
        }

        let json = self.ast_json_without_cache(files, top, scopes, options)?;
        if let Some(key) = cache_key {
            if let Ok(mut cache) = self.ast_cache.lock() {
                cache.insert(key, Arc::new(json.clone()));
            }
        }
        Ok(json)
    }

    fn ast_json_without_cache(
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

    fn named_file_cache_key(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
        options: &PreprocessOptions,
    ) -> Option<SlangAstCacheKey> {
        if options.uses_libraries() {
            return None;
        }
        if options
            .defines
            .iter()
            .chain(options.undefines.iter())
            .any(|value| value.contains("`include"))
        {
            return None;
        }
        let mut contents = Vec::with_capacity(files.len());
        for path in files {
            contents.push((path.clone(), std::fs::read_to_string(path).ok()?));
        }
        let dependencies = include_dependencies(&contents, &options.include_directories)?;

        let mut named_files = Vec::with_capacity(files.len() + dependencies.len());
        for (path, content) in contents {
            named_files.push(SlangNamedSourceCacheKey { path, content });
        }
        named_files.extend(dependencies);
        Some(SlangAstCacheKey {
            program: self.program.to_string_lossy().into_owned(),
            top: top.map(str::to_string),
            include_directories: options.include_directories.clone(),
            defines: options.defines.clone(),
            undefines: options.undefines.clone(),
            library_directories: options.library_directories.clone(),
            library_extensions: options.library_extensions.clone(),
            scopes: scopes.to_vec(),
            input: SlangAstCacheInput::NamedFiles(named_files),
        })
    }

    fn source_cache_key(
        &self,
        source_path: &Path,
        source: &str,
        top: Option<&str>,
        options: &PreprocessOptions,
    ) -> Option<SlangAstCacheKey> {
        if options.uses_libraries() {
            return None;
        }
        if options
            .defines
            .iter()
            .chain(options.undefines.iter())
            .any(|value| value.contains("`include"))
        {
            return None;
        }
        let dependencies = include_dependencies(
            &[(source_path.to_path_buf(), source.to_string())],
            &options.include_directories,
        )?;
        Some(SlangAstCacheKey {
            program: self.program.to_string_lossy().into_owned(),
            top: top.map(str::to_string),
            include_directories: options.include_directories.clone(),
            defines: options.defines.clone(),
            undefines: options.undefines.clone(),
            library_directories: options.library_directories.clone(),
            library_extensions: options.library_extensions.clone(),
            scopes: Vec::new(),
            input: SlangAstCacheInput::Source {
                source: source.to_string(),
                dependencies,
            },
        })
    }

    fn cache_poisoned_failure(&self) -> SlangFailure {
        SlangFailure {
            status: None,
            stdout: String::new(),
            stderr: "slang AST cache mutex poisoned".into(),
        }
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

    /// Compiles a fail-closed JSON project manifest.
    ///
    /// Manifest paths are resolved before this call, and slang's library
    /// search options are forwarded unchanged. Library-aware inputs currently
    /// bypass the AST cache because automatic module resolution dependencies
    /// are not yet tracked.
    pub fn compile_project_manifest(&self, manifest_path: &Path) -> Result<Design, String> {
        let manifest = SlangProjectManifest::parse_file(manifest_path)?;
        self.compile_files(
            &manifest.sources,
            manifest.top.as_deref(),
            &manifest.preprocess_options(),
        )
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
        // The AST request is itself a full slang parse/type/elaboration run.
        // It validates the source before the compatibility elaborator can run,
        // so an additional check-only invocation would duplicate the expensive
        // frontend lifecycle for large official-UVM compilations.
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
        // Requesting slang's elaborated AST is itself the authoritative parse,
        // type-check, and elaboration gate. Do not run a second check-only
        // slang process before it.
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
        let cache_key = self.source_cache_key(&path, source, top, options);
        let cached_ast = cache_key
            .as_ref()
            .map(|key| {
                self.ast_cache
                    .lock()
                    .map(|mut cache| cache.get(key))
                    .map_err(|_| "slang AST cache mutex poisoned".to_string())
            })
            .transpose()?
            .flatten();
        let result = if let Some(json) = cached_ast {
            self.lower_ast_json_with_interface_array_scopes(
                &json,
                std::slice::from_ref(&path),
                top,
                options,
            )
        } else {
            let json = self
                .ast_json_without_cache(std::slice::from_ref(&path), top, &[], options)
                .map_err(|error| error.to_string());
            let json = match json {
                Ok(json) => Arc::new(json),
                Err(error) => {
                    let _ = std::fs::remove_file(&path);
                    return Err(error);
                }
            };
            if let Some(key) = cache_key {
                self.ast_cache
                    .lock()
                    .map_err(|_| "slang AST cache mutex poisoned".to_string())?
                    .insert(key, Arc::clone(&json));
            }
            self.lower_ast_json_with_interface_array_scopes(
                &json,
                std::slice::from_ref(&path),
                top,
                options,
            )
        };
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

fn ast_cache_capacity_from_env() -> Result<usize, String> {
    let Some(value) = std::env::var_os("SLANG_AST_CACHE_ENTRIES") else {
        return Ok(DEFAULT_AST_CACHE_ENTRIES);
    };
    let capacity = value
        .to_string_lossy()
        .parse::<usize>()
        .map_err(|_| "SLANG_AST_CACHE_ENTRIES must be a nonnegative integer".to_string())?;
    Ok(capacity)
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
    for directory in &options.library_directories {
        command.arg("-y").arg(directory);
    }
    for extension in &options.library_extensions {
        command.arg("-Y").arg(extension);
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct SlangIncludeSpec {
    filename: String,
    angle: bool,
}

fn include_dependencies(
    roots: &[(PathBuf, String)],
    include_directories: &[PathBuf],
) -> Option<Vec<SlangNamedSourceCacheKey>> {
    let mut dependencies = BTreeMap::new();
    let mut seen = roots
        .iter()
        .map(|(path, _)| canonical_path(path))
        .collect::<BTreeSet<_>>();
    for (path, content) in roots {
        scan_include_content(
            path,
            content,
            include_directories,
            &mut dependencies,
            &mut seen,
        )?;
    }
    Some(
        dependencies
            .into_iter()
            .map(|(path, content)| SlangNamedSourceCacheKey { path, content })
            .collect(),
    )
}

fn scan_include_content(
    path: &Path,
    content: &str,
    include_directories: &[PathBuf],
    dependencies: &mut BTreeMap<PathBuf, String>,
    seen: &mut BTreeSet<PathBuf>,
) -> Option<()> {
    let specs = literal_include_specs(content)?;
    for spec in specs {
        let dependency_path = resolve_include_path(&spec, path.parent(), include_directories)?;
        if !seen.insert(dependency_path.clone()) {
            continue;
        }
        let dependency_content = std::fs::read_to_string(&dependency_path).ok()?;
        dependencies.insert(dependency_path.clone(), dependency_content.clone());
        scan_include_content(
            &dependency_path,
            &dependency_content,
            include_directories,
            dependencies,
            seen,
        )?;
    }
    Some(())
}

fn literal_include_specs(content: &str) -> Option<Vec<SlangIncludeSpec>> {
    let characters = content.chars().collect::<Vec<_>>();
    let mut specs = Vec::new();
    let mut index = 0_usize;
    while index < characters.len() {
        match characters[index] {
            '/' if index + 1 < characters.len() && characters[index + 1] == '/' => {
                while index < characters.len() && characters[index] != '\n' {
                    index += 1;
                }
            }
            '/' if index + 1 < characters.len() && characters[index + 1] == '*' => {
                index += 2;
                loop {
                    if index + 1 >= characters.len() {
                        return None;
                    }
                    if characters[index] == '*' && characters[index + 1] == '/' {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            '"' => {
                index += 1;
                while index < characters.len() {
                    if characters[index] == '\\' {
                        index += 2;
                    } else if characters[index] == '"' {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            }
            '`' => {
                index += 1;
                let start = index;
                while index < characters.len()
                    && (characters[index].is_ascii_alphanumeric() || characters[index] == '_')
                {
                    index += 1;
                }
                let directive = characters[start..index].iter().collect::<String>();
                match directive.as_str() {
                    "include" => {
                        let (spec, next_index) = parse_include_argument(&characters, index)?;
                        specs.push(spec);
                        index = next_index;
                    }
                    "define"
                    | "undef"
                    | "undefineall"
                    | "ifdef"
                    | "ifndef"
                    | "elsif"
                    | "else"
                    | "endif"
                    | "timescale"
                    | "default_nettype"
                    | "resetall"
                    | "celldefine"
                    | "endcelldefine"
                    | "unconnected_drive"
                    | "nounconnected_drive"
                    | "line"
                    | "pragma"
                    | "begin_keywords"
                    | "end_keywords"
                    | "protected"
                    | "endprotected"
                    | "__FILE__"
                    | "__LINE__" => {}
                    _ => return None,
                }
            }
            _ => index += 1,
        }
    }
    Some(specs)
}

fn parse_include_argument(
    characters: &[char],
    mut index: usize,
) -> Option<(SlangIncludeSpec, usize)> {
    loop {
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        if index + 1 < characters.len() && characters[index] == '/' && characters[index + 1] == '/'
        {
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if index + 1 < characters.len() && characters[index] == '/' && characters[index + 1] == '*'
        {
            index += 2;
            loop {
                if index + 1 >= characters.len() {
                    return None;
                }
                if characters[index] == '*' && characters[index + 1] == '/' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        break;
    }

    let (delimiter, angle) = match characters.get(index)? {
        '"' => ('"', false),
        '<' => ('>', true),
        _ => return None,
    };
    index += 1;
    let start = index;
    while index < characters.len() && characters[index] != delimiter {
        if characters[index] == '\\' || characters[index] == '\n' {
            return None;
        }
        index += 1;
    }
    if index >= characters.len() {
        return None;
    }
    let filename = characters[start..index].iter().collect::<String>();
    if filename.is_empty() {
        return None;
    }
    Some((SlangIncludeSpec { filename, angle }, index + 1))
}

fn resolve_include_path(
    spec: &SlangIncludeSpec,
    current_directory: Option<&Path>,
    include_directories: &[PathBuf],
) -> Option<PathBuf> {
    let filename = Path::new(&spec.filename);
    if filename.is_absolute() {
        return filename.is_file().then(|| canonical_path(filename));
    }

    let mut candidates = Vec::new();
    if !spec.angle {
        if let Some(directory) = current_directory {
            candidates.push(directory.join(filename));
        }
    }
    candidates.extend(include_directories.iter().map(|dir| dir.join(filename)));
    if let Ok(directory) = std::env::current_dir() {
        candidates.push(directory.join(filename));
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| canonical_path(&candidate))
}

fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
    fn project_manifest_parses_and_fails_closed() {
        let manifest = SlangProjectManifest::parse(
            r#"
{
  "top": "top",
  "sources": ["rtl/top.sv", "/absolute/pkg.sv"],
  "include_directories": ["include"],
  "defines": ["FOO=1"],
  "undefines": ["BAR"],
  "library_directories": ["lib"],
  "library_extensions": [".sv"]
}
"#,
            Path::new("/project"),
        )
        .expect("parse valid project manifest");
        assert_eq!(manifest.top.as_deref(), Some("top"));
        assert_eq!(
            manifest.sources,
            vec![
                Path::new("/project/rtl/top.sv"),
                Path::new("/absolute/pkg.sv")
            ]
        );
        assert_eq!(
            manifest.include_directories,
            vec![Path::new("/project/include")]
        );
        assert_eq!(
            manifest.library_directories,
            vec![Path::new("/project/lib")]
        );
        assert_eq!(manifest.defines, vec!["FOO=1"]);
        assert_eq!(manifest.library_extensions, vec![".sv"]);

        let error = SlangProjectManifest::parse(
            r#"{"sources":["a.sv"],"unknown":1}"#,
            Path::new("/project"),
        )
        .expect_err("unknown fields must fail closed");
        assert!(error.contains("unknown project manifest field"), "{error}");
        assert!(
            SlangProjectManifest::parse(
                r#"{"sources":["a.sv"],"library_extensions":["sv"]}"#,
                Path::new("/project")
            )
            .is_err(),
            "library extensions must begin with a dot"
        );
    }

    #[test]
    fn literal_include_scanner_ignores_non_directives_and_rejects_macros() {
        let specs = literal_include_specs(
            r#"
// `include "commented.svh"
/* `include "blocked.svh" */
string s = "`include string.svh";
`include "real.svh"
`include <angle.svh>
"#,
        )
        .expect("literal include directives are cacheable");
        assert_eq!(
            specs,
            vec![
                SlangIncludeSpec {
                    filename: "real.svh".into(),
                    angle: false,
                },
                SlangIncludeSpec {
                    filename: "angle.svh".into(),
                    angle: true,
                },
            ]
        );

        assert!(
            literal_include_specs("`include INCLUDE_MACRO\n").is_none(),
            "macro-expanded include names must bypass the cache"
        );
        assert!(
            literal_include_specs("`UNKNOWN_MACRO\n").is_none(),
            "unknown macro expansions must bypass the cache"
        );
    }

    #[test]
    fn include_dependencies_resolve_nested_literal_files() {
        let directory = std::env::temp_dir().join(format!(
            "eda-slang-nested-includes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create nested include directory");
        let root = directory.join("root.sv");
        let child = directory.join("child.svh");
        let grandchild = directory.join("grandchild.svh");
        std::fs::write(&root, "`include \"child.svh\"\n").expect("write root");
        std::fs::write(&child, "`include \"grandchild.svh\"\n").expect("write child");
        std::fs::write(&grandchild, "parameter NESTED = 1;\n").expect("write grandchild");

        let dependencies = include_dependencies(
            &[(root.clone(), std::fs::read_to_string(&root).unwrap())],
            std::slice::from_ref(&directory),
        )
        .expect("resolve nested literal includes");
        let paths = dependencies
            .iter()
            .map(|dependency| dependency.path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                std::fs::canonicalize(&child).unwrap(),
                std::fs::canonicalize(&grandchild).unwrap()
            ]
        );
        assert_eq!(dependencies[1].content, "parameter NESTED = 1;\n");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn ast_cache_is_bounded_and_least_recently_used() {
        let key = |source: &str| SlangAstCacheKey {
            program: "slang".into(),
            top: Some("top".into()),
            include_directories: Vec::new(),
            defines: Vec::new(),
            undefines: Vec::new(),
            library_directories: Vec::new(),
            library_extensions: Vec::new(),
            scopes: Vec::new(),
            input: SlangAstCacheInput::Source {
                source: source.into(),
                dependencies: Vec::new(),
            },
        };
        let mut cache = SlangAstCache::new(2);
        let first = key("first");
        let second = key("second");
        let third = key("third");

        cache.insert(first.clone(), Arc::new("one".into()));
        cache.insert(second.clone(), Arc::new("two".into()));
        assert!(cache.get(&first).is_some());
        cache.insert(third.clone(), Arc::new("three".into()));

        assert!(cache.entries.contains_key(&first));
        assert!(!cache.entries.contains_key(&second));
        assert!(cache.entries.contains_key(&third));
        assert_eq!(
            cache.stats(),
            SlangAstCacheStats {
                hits: 1,
                misses: 0,
                insertions: 3,
                evictions: 1,
                entries: 2,
            }
        );
    }

    #[test]
    fn slang_version_matching_is_exact() {
        let frontend = SlangFrontend {
            program: PathBuf::from("slang"),
            timeout: DEFAULT_SLANG_TIMEOUT,
            ast_cache: Arc::new(Mutex::new(SlangAstCache::new(DEFAULT_AST_CACHE_ENTRIES))),
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
            ast_cache: Arc::new(Mutex::new(SlangAstCache::new(DEFAULT_AST_CACHE_ENTRIES))),
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
