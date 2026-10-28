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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SlangLibraryPlanKey {
    program: String,
    roots: Vec<SlangNamedSourceCacheKey>,
    top: Option<String>,
    include_directories: Vec<PathBuf>,
    defines: Vec<String>,
    undefines: Vec<String>,
    library_directories: Vec<PathBuf>,
    library_extensions: Vec<String>,
    scopes: Vec<String>,
    directory_inventory: Vec<(PathBuf, Vec<PathBuf>)>,
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
    library_plans: BTreeMap<SlangLibraryPlanKey, Vec<PathBuf>>,
    library_plan_order: VecDeque<SlangLibraryPlanKey>,
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
            library_plans: BTreeMap::new(),
            library_plan_order: VecDeque::new(),
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

    fn record_miss(&mut self) {
        self.misses += 1;
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

    fn library_plan(&mut self, key: &SlangLibraryPlanKey) -> Option<Vec<PathBuf>> {
        let result = self.library_plans.get(key).cloned();
        if result.is_some() {
            if let Some(index) = self
                .library_plan_order
                .iter()
                .position(|entry| entry == key)
            {
                let entry = self
                    .library_plan_order
                    .remove(index)
                    .expect("checked library plan exists");
                self.library_plan_order.push_back(entry);
            }
        }
        result
    }

    fn insert_library_plan(&mut self, key: SlangLibraryPlanKey, files: Vec<PathBuf>) {
        if self.capacity == 0 {
            return;
        }
        if self.library_plans.insert(key.clone(), files).is_none() {
            self.library_plan_order.push_back(key);
        } else if let Some(index) = self
            .library_plan_order
            .iter()
            .position(|entry| *entry == key)
        {
            let entry = self
                .library_plan_order
                .remove(index)
                .expect("checked library plan exists");
            self.library_plan_order.push_back(entry);
        }
        while self.library_plans.len() > self.capacity {
            let Some(oldest) = self.library_plan_order.pop_front() else {
                break;
            };
            self.library_plans.remove(&oldest);
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

#[derive(Clone, Debug)]
pub struct SlangCachedDesign {
    pub design: Design,
    pub cache_hit: bool,
}

/// A persistent database of project records used to plan partial rebuilds.
///
/// The database intentionally stores fingerprints rather than source or AST
/// contents. Per-project AST bytes remain in their own project cache
/// directories; this database owns the cross-project dependency index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangProjectDependencyDatabase {
    path: PathBuf,
    schema: u32,
    projects: BTreeMap<PathBuf, SlangProjectCacheRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangPartialRecompilationPlan {
    pub clean_projects: Vec<PathBuf>,
    pub dirty_projects: Vec<PathBuf>,
    pub dirty_compilation_units: Vec<SlangCompilationUnit>,
    pub changed_files: Vec<PathBuf>,
    pub missing_files: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SlangCompilationUnit {
    pub project_manifest: PathBuf,
    pub name: String,
    pub kind: String,
    pub source_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlangReverseDependency {
    pub file: PathBuf,
    pub projects: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SlangProjectFileRecord {
    path: PathBuf,
    sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SlangCompilationUnitRecord {
    name: String,
    kind: String,
    source_path: PathBuf,
    sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SlangProjectCacheRecord {
    schema: u32,
    program: String,
    runtime_version: String,
    manifest_path: PathBuf,
    manifest_sha256: String,
    top: Option<String>,
    include_directories: Vec<PathBuf>,
    defines: Vec<String>,
    undefines: Vec<String>,
    library_directories: Vec<PathBuf>,
    library_extensions: Vec<String>,
    roots: Vec<SlangProjectFileRecord>,
    dependencies: Vec<SlangProjectFileRecord>,
    compilation_units: Vec<SlangCompilationUnitRecord>,
    library_inventory: Vec<(PathBuf, Vec<PathBuf>)>,
    ast_sha256: String,
}

const SLANG_PROJECT_CACHE_SCHEMA: u32 = 3;
const SLANG_PROJECT_DATABASE_SCHEMA: u32 = 1;

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

fn project_file_record(path: &Path) -> Result<SlangProjectFileRecord, String> {
    let content = std::fs::read(path)
        .map_err(|error| format!("cannot read project file {}: {error}", path.display()))?;
    Ok(SlangProjectFileRecord {
        path: canonical_path(path),
        sha256: sha256_hex(&content),
    })
}

fn project_file_records(paths: &[PathBuf]) -> Result<Vec<SlangProjectFileRecord>, String> {
    paths.iter().map(|path| project_file_record(path)).collect()
}

fn project_compilation_units(ast: &str) -> Option<Vec<SlangCompilationUnitRecord>> {
    let document = serde_json::from_str::<serde_json::Value>(ast).ok()?;
    let definitions = document.get("definitions")?.as_array()?;
    definitions
        .iter()
        .map(|definition| {
            let name = definition.get("name")?.as_str()?.to_string();
            let kind = definition
                .get("definitionKind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Unknown")
                .to_string();
            let source = definition.get("source_file")?.as_str()?;
            let source_path = canonical_path(Path::new(source));
            let sha256 = std::fs::read(&source_path)
                .ok()
                .map(|content| sha256_hex(&content))?;
            Some(SlangCompilationUnitRecord {
                name,
                kind,
                source_path,
                sha256,
            })
        })
        .collect()
}

fn canonical_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().map(|path| canonical_path(path)).collect()
}

fn project_library_inventory(directories: &[PathBuf]) -> Vec<(PathBuf, Vec<PathBuf>)> {
    directories
        .iter()
        .map(|directory| {
            let mut entries = Vec::new();
            if let Ok(iterator) = std::fs::read_dir(directory) {
                for entry in iterator.flatten() {
                    entries.push(canonical_path(&entry.path()));
                }
            }
            entries.sort();
            (canonical_path(directory), entries)
        })
        .collect()
}

fn project_cache_record_json(record: &SlangProjectCacheRecord) -> String {
    serde_json::json!({
        "schema": record.schema,
        "program": record.program,
        "runtime_version": record.runtime_version,
        "manifest_path": record.manifest_path,
        "manifest_sha256": record.manifest_sha256,
        "top": record.top,
        "include_directories": record.include_directories,
        "defines": record.defines,
        "undefines": record.undefines,
        "library_directories": record.library_directories,
        "library_extensions": record.library_extensions,
        "roots": record.roots.iter().map(|file| serde_json::json!({
            "path": file.path,
            "sha256": file.sha256,
        })).collect::<Vec<_>>(),
        "dependencies": record.dependencies.iter().map(|file| serde_json::json!({
            "path": file.path,
            "sha256": file.sha256,
        })).collect::<Vec<_>>(),
        "compilation_units": record.compilation_units.iter().map(|unit| serde_json::json!({
            "name": unit.name,
            "kind": unit.kind,
            "source_path": unit.source_path,
            "sha256": unit.sha256,
        })).collect::<Vec<_>>(),
        "library_inventory": record.library_inventory.iter().map(|(directory, entries)| {
            serde_json::json!({"directory": directory, "entries": entries})
        }).collect::<Vec<_>>(),
        "ast_sha256": record.ast_sha256,
    })
    .to_string()
}

fn json_path(value: &serde_json::Value, field: &str) -> Option<PathBuf> {
    value.get(field)?.as_str().map(PathBuf::from)
}

fn json_path_array(value: &serde_json::Value, field: &str) -> Option<Vec<PathBuf>> {
    value
        .get(field)?
        .as_array()?
        .iter()
        .map(|entry| entry.as_str().map(PathBuf::from))
        .collect()
}

fn json_string_array_value(value: &serde_json::Value, field: &str) -> Option<Vec<String>> {
    value
        .get(field)?
        .as_array()?
        .iter()
        .map(|entry| entry.as_str().map(str::to_string))
        .collect()
}

fn json_file_records(
    value: &serde_json::Value,
    field: &str,
) -> Option<Vec<SlangProjectFileRecord>> {
    value
        .get(field)?
        .as_array()?
        .iter()
        .map(|entry| {
            Some(SlangProjectFileRecord {
                path: json_path(entry, "path")?,
                sha256: entry.get("sha256")?.as_str()?.to_string(),
            })
        })
        .collect()
}

fn json_compilation_units(
    value: &serde_json::Value,
    field: &str,
) -> Option<Vec<SlangCompilationUnitRecord>> {
    value
        .get(field)?
        .as_array()?
        .iter()
        .map(|entry| {
            Some(SlangCompilationUnitRecord {
                name: entry.get("name")?.as_str()?.to_string(),
                kind: entry.get("kind")?.as_str()?.to_string(),
                source_path: json_path(entry, "source_path")?,
                sha256: entry.get("sha256")?.as_str()?.to_string(),
            })
        })
        .collect()
}

fn json_library_inventory(
    value: &serde_json::Value,
    field: &str,
) -> Option<Vec<(PathBuf, Vec<PathBuf>)>> {
    value
        .get(field)?
        .as_array()?
        .iter()
        .map(|entry| {
            Some((
                json_path(entry, "directory")?,
                json_path_array(entry, "entries")?,
            ))
        })
        .collect()
}

fn load_project_cache_record(path: &Path) -> Option<SlangProjectCacheRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    parse_project_cache_record(&value)
}

fn parse_project_cache_record(value: &serde_json::Value) -> Option<SlangProjectCacheRecord> {
    Some(SlangProjectCacheRecord {
        schema: value.get("schema")?.as_u64()?.try_into().ok()?,
        program: value.get("program")?.as_str()?.to_string(),
        runtime_version: value.get("runtime_version")?.as_str()?.to_string(),
        manifest_path: json_path(value, "manifest_path")?,
        manifest_sha256: value.get("manifest_sha256")?.as_str()?.to_string(),
        top: match value.get("top")? {
            serde_json::Value::Null => None,
            serde_json::Value::String(top) => Some(top.to_string()),
            _ => return None,
        },
        include_directories: json_path_array(value, "include_directories")?,
        defines: json_string_array_value(value, "defines")?,
        undefines: json_string_array_value(value, "undefines")?,
        library_directories: json_path_array(value, "library_directories")?,
        library_extensions: json_string_array_value(value, "library_extensions")?,
        roots: json_file_records(value, "roots")?,
        dependencies: json_file_records(value, "dependencies")?,
        compilation_units: json_compilation_units(value, "compilation_units")?,
        library_inventory: json_library_inventory(value, "library_inventory")?,
        ast_sha256: value.get("ast_sha256")?.as_str()?.to_string(),
    })
}

fn project_cache_record_matches(
    record: &SlangProjectCacheRecord,
    program: &str,
    runtime_version: &str,
    manifest_path: &Path,
    manifest: &SlangProjectManifest,
    roots: &[SlangProjectFileRecord],
    library_inventory: &[(PathBuf, Vec<PathBuf>)],
) -> bool {
    record.schema == SLANG_PROJECT_CACHE_SCHEMA
        && record.program == program
        && record.runtime_version == runtime_version
        && record.manifest_path == canonical_path(manifest_path)
        && project_file_record(manifest_path)
            .is_ok_and(|manifest| manifest.sha256 == record.manifest_sha256)
        && record.top == manifest.top
        && record.include_directories == canonical_paths(&manifest.include_directories)
        && record.defines == manifest.defines
        && record.undefines == manifest.undefines
        && record.library_directories == canonical_paths(&manifest.library_directories)
        && record.library_extensions == manifest.library_extensions
        && record.roots == roots
        && record.library_inventory == library_inventory
        && record.dependencies.iter().all(|dependency| {
            project_file_record(&dependency.path)
                .is_ok_and(|current| current.sha256 == dependency.sha256)
        })
}

fn load_project_cache_ast(path: &Path, record: &SlangProjectCacheRecord) -> Option<String> {
    let ast = std::fs::read_to_string(path).ok()?;
    (sha256_hex(ast.as_bytes()) == record.ast_sha256).then_some(ast)
}

fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "cannot create cache directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&temporary, contents)
        .map_err(|error| format!("cannot write cache file {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot install cache file {}: {error}", path.display()))
}

fn save_project_cache(
    directory: &Path,
    record: &SlangProjectCacheRecord,
    ast: &str,
) -> Result<(), String> {
    atomic_write(
        &directory.join("record.json"),
        &project_cache_record_json(record),
    )?;
    atomic_write(&directory.join("ast.json"), ast)
}

impl SlangProjectDependencyDatabase {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self {
                path: path.to_path_buf(),
                schema: SLANG_PROJECT_DATABASE_SCHEMA,
                projects: BTreeMap::new(),
            });
        }
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read project database {}: {error}", path.display()))?;
        let value = serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|error| format!("invalid project database {}: {error}", path.display()))?;
        let schema = value
            .get("schema")
            .and_then(serde_json::Value::as_u64)
            .and_then(|schema| u32::try_from(schema).ok())
            .ok_or_else(|| format!("project database {} has no valid schema", path.display()))?;
        if schema != SLANG_PROJECT_DATABASE_SCHEMA {
            return Err(format!(
                "unsupported project database schema {schema} in {}; expected {SLANG_PROJECT_DATABASE_SCHEMA}",
                path.display()
            ));
        }
        let entries = value
            .get("projects")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("project database {} has no projects array", path.display()))?;
        let mut projects = BTreeMap::new();
        for entry in entries {
            let record = parse_project_cache_record(entry).ok_or_else(|| {
                format!(
                    "project database {} contains an invalid project record",
                    path.display()
                )
            })?;
            if record.schema != SLANG_PROJECT_CACHE_SCHEMA {
                return Err(format!(
                    "project database {} contains project cache schema {}; expected {SLANG_PROJECT_CACHE_SCHEMA}",
                    path.display(),
                    record.schema
                ));
            }
            projects.insert(record.manifest_path.clone(), record);
        }
        Ok(Self {
            path: path.to_path_buf(),
            schema,
            projects,
        })
    }

    pub fn record_project_cache(&mut self, cache_directory: &Path) -> Result<(), String> {
        let record =
            load_project_cache_record(&cache_directory.join("record.json")).ok_or_else(|| {
                format!(
                    "project cache {} has no valid record.json",
                    cache_directory.display()
                )
            })?;
        if record.schema != SLANG_PROJECT_CACHE_SCHEMA {
            return Err(format!(
                "project cache schema {} does not match supported schema {SLANG_PROJECT_CACHE_SCHEMA}",
                record.schema
            ));
        }
        let ast = std::fs::read_to_string(cache_directory.join("ast.json")).map_err(|error| {
            format!(
                "cannot read project AST {}: {error}",
                cache_directory.join("ast.json").display()
            )
        })?;
        if sha256_hex(ast.as_bytes()) != record.ast_sha256 {
            return Err(format!(
                "project AST checksum mismatch in {}",
                cache_directory.display()
            ));
        }
        self.projects.insert(record.manifest_path.clone(), record);
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        let projects = self
            .projects
            .values()
            .map(|record| {
                serde_json::from_str::<serde_json::Value>(&project_cache_record_json(record))
                    .map_err(|error| format!("cannot serialize project record: {error}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let text = serde_json::json!({
            "schema": self.schema,
            "projects": projects,
        })
        .to_string();
        atomic_write(&self.path, &text)
    }

    pub fn project_manifests(&self) -> Vec<PathBuf> {
        self.projects.keys().cloned().collect()
    }

    pub fn partial_recompilation_plan(&self) -> SlangPartialRecompilationPlan {
        let mut clean_projects = Vec::new();
        let mut dirty_projects = Vec::new();
        let mut dirty_compilation_units = BTreeSet::new();
        let mut changed_files = BTreeSet::new();
        let mut missing_files = BTreeSet::new();
        for record in self.projects.values() {
            let mut dirty = false;
            let mut project_changed_files = Vec::new();
            let mut project_missing_files = Vec::new();
            for file in record.roots.iter().chain(record.dependencies.iter()) {
                match project_file_record(&file.path) {
                    Ok(current) => {
                        if current.sha256 != file.sha256 {
                            changed_files.insert(file.path.clone());
                            project_changed_files.push(file.path.clone());
                            dirty = true;
                        }
                    }
                    Err(_) => {
                        missing_files.insert(file.path.clone());
                        project_missing_files.push(file.path.clone());
                        dirty = true;
                    }
                }
            }
            let current_inventory = project_library_inventory(&record.library_directories);
            let mut inventory_changed = false;
            if current_inventory != record.library_inventory {
                dirty = true;
                inventory_changed = true;
            }
            let mut manifest_changed = false;
            if project_file_record(&record.manifest_path)
                .map(|manifest| manifest.sha256 != record.manifest_sha256)
                .unwrap_or(true)
            {
                changed_files.insert(record.manifest_path.clone());
                project_changed_files.push(record.manifest_path.clone());
                dirty = true;
                manifest_changed = true;
            }

            let mut record_dirty_units = BTreeSet::new();
            for unit in &record.compilation_units {
                if project_changed_files.contains(&unit.source_path)
                    || project_missing_files.contains(&unit.source_path)
                {
                    record_dirty_units.insert(unit.clone());
                }
            }
            if (inventory_changed || manifest_changed)
                && record_dirty_units.len() < record.compilation_units.len()
            {
                record_dirty_units.extend(record.compilation_units.iter().cloned());
            }
            dirty_compilation_units.extend(record_dirty_units.into_iter().map(|unit| {
                SlangCompilationUnit {
                    project_manifest: record.manifest_path.clone(),
                    name: unit.name,
                    kind: unit.kind,
                    source_path: unit.source_path,
                }
            }));
            if dirty {
                dirty_projects.push(record.manifest_path.clone());
            } else {
                clean_projects.push(record.manifest_path.clone());
            }
        }
        SlangPartialRecompilationPlan {
            clean_projects,
            dirty_projects,
            dirty_compilation_units: dirty_compilation_units.into_iter().collect(),
            changed_files: changed_files.into_iter().collect(),
            missing_files: missing_files.into_iter().collect(),
        }
    }

    pub fn reverse_dependencies(&self) -> Vec<SlangReverseDependency> {
        let mut dependencies = BTreeMap::new();
        for record in self.projects.values() {
            for file in record.roots.iter().chain(record.dependencies.iter()) {
                dependencies
                    .entry(file.path.clone())
                    .or_insert_with(Vec::new)
                    .push(record.manifest_path.clone());
            }
        }
        dependencies
            .into_iter()
            .map(|(file, projects)| SlangReverseDependency { file, projects })
            .collect()
    }
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
        let library_plan_key = if options.uses_libraries() {
            self.library_plan_cache_key(files, top, scopes, options)
        } else {
            None
        };
        let planned_files = library_plan_key
            .as_ref()
            .map(|key| {
                self.ast_cache
                    .lock()
                    .map(|mut cache| cache.library_plan(key))
                    .map_err(|_| self.cache_poisoned_failure())
            })
            .transpose()?
            .flatten();
        if library_plan_key.is_some() && planned_files.is_none() {
            self.ast_cache
                .lock()
                .map_err(|_| self.cache_poisoned_failure())?
                .record_miss();
        }
        let cache_key = if let Some(files) = planned_files {
            self.explicit_file_cache_key(&files, top, scopes, options)
        } else {
            self.named_file_cache_key(files, top, scopes, options)
        };
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

        let (json, parsed_files) = self.ast_json_and_parsed_files(
            files,
            top,
            scopes,
            options,
            library_plan_key.is_some(),
        )?;
        if let Some(plan_key) = library_plan_key {
            if !parsed_files.is_empty() {
                let mut selected_files = files.to_vec();
                selected_files.extend(parsed_files);
                if let Some(key) =
                    self.explicit_file_cache_key(&selected_files, top, scopes, options)
                {
                    if let Ok(mut cache) = self.ast_cache.lock() {
                        cache.insert_library_plan(plan_key, selected_files);
                        cache.insert(key, Arc::new(json.clone()));
                    }
                }
            }
        } else if let Some(key) = cache_key {
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
        self.ast_json_and_parsed_files(files, top, scopes, options, false)
            .map(|(json, _)| json)
    }

    fn ast_json_and_parsed_files(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
        options: &PreprocessOptions,
        show_parsed_files: bool,
    ) -> Result<(String, Vec<PathBuf>), SlangFailure> {
        let mut command = self.command();
        if let Some(top) = top {
            command.arg("--top").arg(top);
        }
        command.arg("--ast-json").arg("-");
        command.arg("--ast-json-detailed-types");
        command.arg("--ast-json-source-info");
        if show_parsed_files {
            command.arg("--show-parsed-files");
        }
        for scope in scopes {
            command.arg("--ast-json-scope").arg(scope);
        }
        add_preprocessor_options(&mut command, options);
        command.args(files);
        let output = self.run_capturing(command)?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let start = text.find('{').unwrap_or(0);
        let files = if show_parsed_files {
            parsed_file_paths(&text[..start])
        } else {
            Vec::new()
        };
        Ok((text[start..].to_string(), files))
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

    fn explicit_file_cache_key(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
        options: &PreprocessOptions,
    ) -> Option<SlangAstCacheKey> {
        if options
            .defines
            .iter()
            .chain(options.undefines.iter())
            .any(|value| value.contains("`include"))
        {
            return None;
        }
        let mut unique_files = BTreeMap::new();
        for path in files {
            let content = std::fs::read_to_string(path).ok()?;
            unique_files.insert(canonical_path(path), content);
        }
        let contents = unique_files.into_iter().collect::<Vec<_>>();
        let dependencies = include_dependencies(&contents, &options.include_directories)?;
        let mut named_files = contents
            .into_iter()
            .map(|(path, content)| SlangNamedSourceCacheKey { path, content })
            .collect::<Vec<_>>();
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

    fn library_plan_cache_key(
        &self,
        files: &[PathBuf],
        top: Option<&str>,
        scopes: &[String],
        options: &PreprocessOptions,
    ) -> Option<SlangLibraryPlanKey> {
        let mut roots = Vec::with_capacity(files.len());
        for path in files {
            roots.push(SlangNamedSourceCacheKey {
                path: path.clone(),
                content: std::fs::read_to_string(path).ok()?,
            });
        }
        let mut directory_inventory = Vec::with_capacity(options.library_directories.len());
        for directory in &options.library_directories {
            let mut entries = Vec::new();
            if let Ok(iterator) = std::fs::read_dir(directory) {
                for entry in iterator.flatten() {
                    entries.push(entry.path());
                }
            }
            entries.sort();
            directory_inventory.push((directory.clone(), entries));
        }
        Some(SlangLibraryPlanKey {
            program: self.program.to_string_lossy().into_owned(),
            roots,
            top: top.map(str::to_string),
            include_directories: options.include_directories.clone(),
            defines: options.defines.clone(),
            undefines: options.undefines.clone(),
            library_directories: options.library_directories.clone(),
            library_extensions: options.library_extensions.clone(),
            scopes: scopes.to_vec(),
            directory_inventory,
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
    /// search options are forwarded unchanged. Library-aware AST requests record
    /// slang's selected-file plan and include selected library contents in the
    /// cache key.
    pub fn compile_project_manifest(&self, manifest_path: &Path) -> Result<Design, String> {
        self.compile_project_manifest_with_cache(manifest_path, None)
            .map(|cached| cached.design)
    }

    /// Compiles a project manifest and reuses a persistent per-project AST cache.
    ///
    /// The cache directory contains an atomic `record.json` / `ast.json` pair.
    /// A hit requires the exact manifest options, source contents, selected
    /// dependency contents, library inventory, slang program/version, AST hash,
    /// and cache schema to match. Interface-array projects that need additional
    /// scoped AST requests deliberately bypass this persistent cache.
    pub fn compile_project_manifest_with_cache(
        &self,
        manifest_path: &Path,
        cache_directory: Option<&Path>,
    ) -> Result<SlangCachedDesign, String> {
        let manifest = SlangProjectManifest::parse_file(manifest_path)?;
        let options = manifest.preprocess_options();
        let Some(cache_directory) = cache_directory else {
            let design =
                self.compile_files(&manifest.sources, manifest.top.as_deref(), &options)?;
            return Ok(SlangCachedDesign {
                design,
                cache_hit: false,
            });
        };
        let roots = project_file_records(&manifest.sources)?;
        let library_inventory = project_library_inventory(&options.library_directories);
        let runtime_version = PINNED_SLANG_VERSION.to_string();
        let program = self.program.to_string_lossy().into_owned();
        {
            if let Some(record) = load_project_cache_record(&cache_directory.join("record.json")) {
                if project_cache_record_matches(
                    &record,
                    &program,
                    &runtime_version,
                    manifest_path,
                    &manifest,
                    &roots,
                    &library_inventory,
                ) {
                    if let Some(ast) =
                        load_project_cache_ast(&cache_directory.join("ast.json"), &record)
                    {
                        if project_compilation_units(&ast).as_ref()
                            == Some(&record.compilation_units)
                        {
                            if let Ok(design) = self.lower_ast_json(&ast) {
                                return Ok(SlangCachedDesign {
                                    design,
                                    cache_hit: true,
                                });
                            }
                        }
                    }
                }
            }
        }

        let (ast, selected_files) = self
            .ast_json_and_parsed_files(
                &manifest.sources,
                manifest.top.as_deref(),
                &[],
                &options,
                true,
            )
            .map_err(|error| error.to_string())?;
        let root = parse_ast_root(&ast)?;
        let interface_plan = plan_interface_array_port_scopes(&root)?;
        if interface_array_port_plan_has_members(&interface_plan) {
            let design = self.lower_ast_json_with_interface_array_scopes(
                &ast,
                &manifest.sources,
                manifest.top.as_deref(),
                &options,
            )?;
            return Ok(SlangCachedDesign {
                design,
                cache_hit: false,
            });
        }

        let design = self.lower_ast_json(&ast)?;
        {
            let root_paths = manifest
                .sources
                .iter()
                .map(|path| canonical_path(path))
                .collect::<BTreeSet<_>>();
            let dependencies = selected_files
                .iter()
                .filter(|path| !root_paths.contains(*path))
                .map(|path| project_file_record(path))
                .collect::<Result<Vec<_>, String>>()?;
            let Some(compilation_units) = project_compilation_units(&ast) else {
                return Ok(SlangCachedDesign {
                    design,
                    cache_hit: false,
                });
            };
            let record = SlangProjectCacheRecord {
                schema: SLANG_PROJECT_CACHE_SCHEMA,
                program,
                runtime_version,
                manifest_path: canonical_path(manifest_path),
                manifest_sha256: project_file_record(manifest_path)?.sha256,
                top: manifest.top.clone(),
                include_directories: canonical_paths(&manifest.include_directories),
                defines: manifest.defines.clone(),
                undefines: manifest.undefines.clone(),
                library_directories: canonical_paths(&manifest.library_directories),
                library_extensions: manifest.library_extensions.clone(),
                roots,
                dependencies,
                compilation_units,
                library_inventory,
                ast_sha256: sha256_hex(ast.as_bytes()),
            };
            save_project_cache(cache_directory, &record, &ast)?;
        }
        Ok(SlangCachedDesign {
            design,
            cache_hit: false,
        })
    }

    /// Compiles into a deterministic project cache slot and updates a shared database.
    ///
    /// The shared database stores fingerprints and reverse dependencies only;
    /// AST bytes stay in the project's own cache directory. This allows many
    /// project manifests to share one partial-recompilation index without
    /// duplicating their ASTs.
    pub fn compile_project_manifest_with_shared_cache(
        &self,
        manifest_path: &Path,
        cache_root: &Path,
        database_path: &Path,
    ) -> Result<SlangCachedDesign, String> {
        let manifest_key = sha256_hex(canonical_path(manifest_path).as_os_str().as_encoded_bytes());
        let cache_directory = cache_root.join(format!("project-{manifest_key}"));
        let cached =
            self.compile_project_manifest_with_cache(manifest_path, Some(&cache_directory))?;
        let mut database = SlangProjectDependencyDatabase::load(database_path)?;
        database.record_project_cache(&cache_directory)?;
        database.save()?;
        Ok(cached)
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

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut message = data.to_vec();
    let bit_length = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());

    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in message.chunks_exact(64) {
        let mut schedule = [0_u32; 64];
        for (index, word) in chunk.chunks_exact(4).enumerate() {
            schedule[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temporary1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary1);
            d = c;
            c = b;
            b = a;
            a = temporary1.wrapping_add(temporary2);
        }
        for (word, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(value);
        }
    }

    state
        .iter()
        .flat_map(|word| u32::to_be_bytes(*word))
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn parsed_file_paths(prefix: &str) -> Vec<PathBuf> {
    prefix
        .lines()
        .filter_map(|line| {
            if !line.starts_with("Parsing ") || !line.ends_with("'.") {
                return None;
            }
            let start = line.find("file '")? + "file '".len();
            let end = line.len().saturating_sub(2);
            let path = Path::new(&line[start..end]);
            (start < end && !path.as_os_str().is_empty()).then(|| canonical_path(path))
        })
        .collect()
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
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

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
    fn parsed_file_paths_extract_design_library_and_include_files() {
        let paths = parsed_file_paths(
            "Parsing design file '/tmp/top.sv'.\n\
             Parsing include file '/tmp/inc/a.svh'.\n\
             Back to file '/tmp/top.sv'.\n\
             Parsing library file '/tmp/lib/child.sv'.\n",
        );
        assert_eq!(
            paths,
            vec![
                Path::new("/tmp/top.sv"),
                Path::new("/tmp/inc/a.svh"),
                Path::new("/tmp/lib/child.sv"),
            ]
        );
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
