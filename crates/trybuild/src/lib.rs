use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

const MAX_FIXTURE_BYTES: u64 = 128 * 1024;
const MAX_DIAGNOSTIC_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TEMP_ATTEMPTS: u64 = 32;
const DELAYED_PRIVACY_PROBE: &str = r#"

#[allow(dead_code)]
trait __TrybuildUnavailableDefaultProbe {
    fn default() -> Self;
}
impl<T> __TrybuildUnavailableDefaultProbe for T {
    fn default() -> Self {
        panic!("trybuild delayed privacy probe")
    }
}

#[allow(dead_code)]
trait __TrybuildUnavailableCloneProbe {
    fn clone(&self) -> Self;
}
impl<T> __TrybuildUnavailableCloneProbe for T {
    fn clone(&self) -> Self {
        panic!("trybuild delayed privacy probe")
    }
}

#[allow(dead_code)]
trait __TrybuildUnavailableNewProbe {
    fn new() -> Self;
}
impl<T> __TrybuildUnavailableNewProbe for T {
    fn new() -> Self {
        panic!("trybuild delayed privacy probe")
    }
}
"#;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub struct TestCases {
    caller_file: &'static str,
}

impl TestCases {
    #[track_caller]
    pub fn new() -> Self {
        Self {
            caller_file: std::panic::Location::caller().file(),
        }
    }

    #[track_caller]
    pub fn compile_fail(&self, fixture: impl AsRef<Path>) {
        let fixture_argument = fixture.as_ref();
        let fixture_display = normalized_relative_path(fixture_argument);
        let package_root = resolve_package_root(fixture_argument, self.caller_file)
            .unwrap_or_else(|error| panic!("trybuild_fixture_root_invalid: {error}"));
        let fixture_path = resolve_fixture(&package_root, fixture_argument)
            .unwrap_or_else(|error| panic!("trybuild_fixture_invalid: {error}"));
        let expected_path = fixture_path.with_extension("stderr");
        validate_input_file(&fixture_path)
            .unwrap_or_else(|error| panic!("trybuild_fixture_invalid: {error}"));
        validate_input_file(&expected_path)
            .unwrap_or_else(|error| panic!("trybuild_expected_diagnostic_invalid: {error}"));

        let workspace_root = workspace_root(&package_root)
            .unwrap_or_else(|error| panic!("trybuild_workspace_invalid: {error}"));
        let canonical_target = canonical_target_dir(&workspace_root)
            .unwrap_or_else(|error| panic!("trybuild_target_invalid: {error}"));
        let temporary = TemporaryCase::create(&canonical_target)
            .unwrap_or_else(|error| panic!("trybuild_temporary_crate_unavailable: {error}"));
        temporary
            .materialize(&workspace_root, &package_root, &fixture_path)
            .unwrap_or_else(|error| panic!("trybuild_temporary_crate_invalid: {error}"));

        let output = temporary
            .compile()
            .unwrap_or_else(|error| panic!("trybuild_compiler_unavailable: {error}"));
        if output.success {
            panic!(
                "trybuild_expected_compile_failure_but_succeeded: {}",
                fixture_display
            );
        }

        let mut actual = diagnostics_from_json(
            &output.stdout,
            &temporary.source_path(),
            &fixture_display,
            None,
        )
        .unwrap_or_else(|error| panic!("trybuild_diagnostic_invalid: {error}"));
        if diagnostic_has_code(&output.stdout, "E0599")
            && !diagnostic_has_code(&output.stdout, "E0451")
        {
            let privacy_output = temporary
                .compile_delayed_privacy_probe()
                .unwrap_or_else(|error| panic!("trybuild_compiler_unavailable: {error}"));
            if !privacy_output.success {
                if let Ok(privacy_diagnostics) = diagnostics_from_json(
                    &privacy_output.stdout,
                    &temporary.source_path(),
                    &fixture_display,
                    Some("E0451"),
                ) {
                    actual.push_str(&privacy_diagnostics);
                }
            }
        }
        let expected = read_bounded_utf8(&expected_path, MAX_DIAGNOSTIC_BYTES)
            .unwrap_or_else(|error| panic!("trybuild_expected_diagnostic_invalid: {error}"));
        if normalize_diagnostic(&actual) != normalize_diagnostic(&expected) {
            panic!(
                "trybuild_diagnostic_mismatch: {}\nexpected:\n{}\nactual:\n{}",
                fixture_display,
                normalize_diagnostic(&expected),
                normalize_diagnostic(&actual),
            );
        }
    }
}

struct TemporaryCase {
    root: PathBuf,
}

impl TemporaryCase {
    fn create(canonical_target: &Path) -> Result<Self, String> {
        fs::create_dir_all(canonical_target)
            .map_err(|_| "canonical target cannot be created".to_string())?;
        let metadata = fs::symlink_metadata(canonical_target)
            .map_err(|_| "canonical target cannot be inspected".to_string())?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("canonical target must be a real directory".to_string());
        }

        let parent = canonical_target.join("trybuild-local");
        fs::create_dir_all(&parent)
            .map_err(|_| "temporary parent cannot be created".to_string())?;
        for _ in 0..MAX_TEMP_ATTEMPTS {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!("case-{}-{id}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { root: candidate }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("temporary case cannot be created".to_string()),
            }
        }
        Err("temporary case allocation exhausted".to_string())
    }

    fn materialize(
        &self,
        workspace_root: &Path,
        dependency_root: &Path,
        fixture: &Path,
    ) -> Result<(), String> {
        let source_dir = self.root.join("src");
        fs::create_dir(&source_dir)
            .map_err(|_| "temporary source directory cannot be created".to_string())?;
        let dependency = toml_string(&dependency_root.to_string_lossy());
        let mut manifest = format!(
            "[package]\nname = \"trybuild-case\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nlicoup-native = {{ path = {dependency} }}\n\n[workspace]\nresolver = \"3\"\n"
        );
        manifest.push_str(&workspace_patch(workspace_root)?);
        fs::write(self.root.join("Cargo.toml"), manifest)
            .map_err(|_| "temporary manifest cannot be written".to_string())?;
        fs::copy(
            workspace_root.join("Cargo.lock"),
            self.root.join("Cargo.lock"),
        )
        .map_err(|_| "workspace lock cannot be copied".to_string())?;
        fs::copy(fixture, source_dir.join("main.rs"))
            .map_err(|_| "temporary fixture cannot be copied".to_string())?;
        Ok(())
    }

    fn source_path(&self) -> PathBuf {
        self.root.join("src/main.rs")
    }

    fn compile(&self) -> Result<CompilerOutput, String> {
        self.run_compiler("primary")
    }

    fn compile_delayed_privacy_probe(&self) -> Result<CompilerOutput, String> {
        let source_path = self.source_path();
        let mut source = read_bounded_utf8(&source_path, MAX_FIXTURE_BYTES)?;
        source.push_str(DELAYED_PRIVACY_PROBE);
        fs::write(&source_path, source)
            .map_err(|_| "privacy probe source cannot be written".to_string())?;
        self.run_compiler("privacy-probe")
    }

    fn run_compiler(&self, artifact_label: &str) -> Result<CompilerOutput, String> {
        let stdout_path = self.root.join(format!("compiler-{artifact_label}.stdout"));
        let stderr_path = self.root.join(format!("compiler-{artifact_label}.stderr"));
        let stdout = File::create(&stdout_path)
            .map_err(|_| "compiler stdout cannot be opened".to_string())?;
        let stderr = File::create(&stderr_path)
            .map_err(|_| "compiler stderr cannot be opened".to_string())?;
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let build_target = self.root.join("target");
        let status = Command::new(cargo)
            .arg("check")
            .arg("--offline")
            .arg("--quiet")
            .arg("--color=never")
            .arg("--manifest-path")
            .arg(self.root.join("Cargo.toml"))
            .arg("--message-format=json")
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_TARGET_DIR", &build_target)
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .status()
            .map_err(|_| "cargo check could not be started".to_string())?;
        let stdout = read_bounded_utf8(&stdout_path, MAX_DIAGNOSTIC_BYTES)?;
        let stderr = read_bounded_utf8(&stderr_path, MAX_DIAGNOSTIC_BYTES)?;
        if !status.success() && stdout.trim().is_empty() {
            return Err(format!(
                "cargo failed without compiler diagnostics: {}",
                stable_cargo_failure(&stderr)
            ));
        }
        Ok(CompilerOutput {
            success: status.success(),
            stdout,
        })
    }
}

impl Drop for TemporaryCase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct CompilerOutput {
    success: bool,
    stdout: String,
}

fn diagnostics_from_json(
    output: &str,
    temporary_source: &Path,
    fixture_display: &str,
    required_code: Option<&str>,
) -> Result<String, String> {
    let mut diagnostics = String::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let message: Value =
            serde_json::from_str(line).map_err(|_| "cargo emitted malformed JSON".to_string())?;
        if message.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let target_name = message
            .get("target")
            .and_then(|target| target.get("name"))
            .and_then(Value::as_str);
        let diagnostic = message
            .get("message")
            .ok_or_else(|| "compiler message omitted diagnostic".to_string())?;
        if !matches!(target_name, Some("trybuild-case" | "trybuild_case"))
            || diagnostic.get("level").and_then(Value::as_str) != Some("error")
        {
            continue;
        }
        let diagnostic_code = diagnostic
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(Value::as_str);
        if required_code.is_some_and(|required| diagnostic_code != Some(required)) {
            continue;
        }
        let rendered = diagnostic
            .get("rendered")
            .and_then(Value::as_str)
            .ok_or_else(|| "compiler diagnostic omitted rendered text".to_string())?;
        let temporary_source = temporary_source.to_string_lossy();
        diagnostics.push_str(
            &rendered
                .replace(temporary_source.as_ref(), fixture_display)
                .replace("src/main.rs", fixture_display),
        );
    }
    if diagnostics.trim().is_empty() {
        return Err("cargo emitted no target error diagnostics".to_string());
    }
    Ok(diagnostics)
}

fn normalize_diagnostic(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n");
    let mut diagnostics = error_diagnostic_blocks(&normalized)
        .into_iter()
        .map(|block| normalize_error_block(&block))
        .collect::<Vec<_>>();
    diagnostics.sort_by_key(|diagnostic| diagnostic_location(diagnostic));
    diagnostics.join("\n\n").trim_end().to_string()
}

fn error_diagnostic_blocks(value: &str) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let mut current = String::new();
    for line in value.lines() {
        if line.starts_with("error[") && !current.trim().is_empty() {
            diagnostics.push(current.trim_end().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        diagnostics.push(current.trim_end().to_string());
    }
    diagnostics
}

fn normalize_error_block(block: &str) -> String {
    const TRAIT_HELP: &str =
        "= help: items from traits can only be used if the trait is implemented and in scope";
    let mut lines = Vec::new();
    let mut trait_help_removed = false;
    for line in block.lines() {
        if line.trim_start().starts_with(TRAIT_HELP) {
            trait_help_removed = true;
            break;
        }
        lines.push(line);
    }
    if trait_help_removed {
        while lines
            .last()
            .is_some_and(|line| line.trim().is_empty() || line.trim() == "|")
        {
            lines.pop();
        }
    }
    let without_trait_help = lines.join("\n").trim_end().to_string();
    if without_trait_help.starts_with("error[E0451]:") {
        normalize_private_field_diagnostic(&without_trait_help)
    } else {
        without_trait_help
    }
}

fn normalize_private_field_diagnostic(diagnostic: &str) -> String {
    let location = diagnostic
        .lines()
        .find(|line| line.trim_start().starts_with("--> "));
    let (line_number, _) = diagnostic_location(diagnostic);
    let line_number = line_number.to_string();
    let source_line = diagnostic.lines().find(|line| {
        line.trim_start()
            .strip_prefix(&line_number)
            .is_some_and(|suffix| suffix.starts_with(" |"))
    });
    let Some((header, location, source_line)) = diagnostic
        .lines()
        .next()
        .zip(location)
        .zip(source_line)
        .map(|((header, location), source_line)| (header, location.trim_start(), source_line))
    else {
        return diagnostic.to_string();
    };
    format!("{header}\n {location}\n |\n{}", source_line.trim_start())
}

fn diagnostic_location(diagnostic: &str) -> (u64, u64) {
    diagnostic
        .lines()
        .find_map(|line| {
            let location = line.trim_start().strip_prefix("--> ")?;
            let (path_and_line, column) = location.rsplit_once(':')?;
            let (_, line) = path_and_line.rsplit_once(':')?;
            Some((line.parse().ok()?, column.parse().ok()?))
        })
        .unwrap_or((u64::MAX, u64::MAX))
}

fn diagnostic_has_code(output: &str, expected_code: &str) -> bool {
    output.lines().any(|line| {
        serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|message| message.get("message").cloned())
            .and_then(|diagnostic| diagnostic.get("code").cloned())
            .and_then(|code| code.get("code").cloned())
            .and_then(|code| code.as_str().map(str::to_owned))
            .is_some_and(|code| code == expected_code)
    })
}

fn stable_cargo_failure(stderr: &str) -> &'static str {
    if stderr.contains("offline") || stderr.contains("download") {
        "offline_dependency_unavailable"
    } else if stderr.contains("lock") {
        "cargo_lock_unavailable"
    } else {
        "cargo_invocation_failed"
    }
}

fn resolve_package_root(fixture: &Path, caller_file: &str) -> Result<PathBuf, String> {
    let current = std::env::current_dir().map_err(|_| "working directory unavailable")?;
    if current.join(fixture).is_file() {
        return current
            .canonicalize()
            .map_err(|_| "package root cannot be resolved".to_string());
    }

    let caller = Path::new(caller_file);
    let caller_path = if caller.is_absolute() {
        caller.to_path_buf()
    } else {
        current.join(caller)
    };
    for ancestor in caller_path.ancestors().skip(1) {
        if ancestor.join("Cargo.toml").is_file() && ancestor.join(fixture).is_file() {
            return ancestor
                .canonicalize()
                .map_err(|_| "package root cannot be resolved".to_string());
        }
    }
    Err("fixture does not resolve beneath the invoking package".to_string())
}

fn resolve_fixture(package_root: &Path, fixture: &Path) -> Result<PathBuf, String> {
    if fixture.is_absolute() {
        return Err("absolute fixture paths are not accepted".to_string());
    }
    if fixture
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("fixture path cannot escape the package".to_string());
    }
    let resolved = package_root
        .join(fixture)
        .canonicalize()
        .map_err(|_| "fixture cannot be resolved".to_string())?;
    if !resolved.starts_with(package_root) {
        return Err("fixture path escaped the package".to_string());
    }
    Ok(resolved)
}

fn workspace_root(package_root: &Path) -> Result<PathBuf, String> {
    for ancestor in package_root.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        let is_workspace = fs::read_to_string(&manifest)
            .ok()
            .and_then(|content| content.parse::<toml::Value>().ok())
            .and_then(|value| value.get("workspace").cloned())
            .is_some();
        if manifest.is_file() && is_workspace {
            return ancestor
                .canonicalize()
                .map_err(|_| "workspace root cannot be resolved".to_string());
        }
    }
    Err("workspace root is unavailable".to_string())
}

fn canonical_target_dir(workspace_root: &Path) -> Result<PathBuf, String> {
    let candidate = if let Some(configured) = std::env::var_os("CARGO_TARGET_DIR") {
        let configured = PathBuf::from(configured);
        if configured.is_absolute() {
            configured
        } else {
            std::env::current_dir()
                .map_err(|_| "working directory unavailable")?
                .join(configured)
        }
    } else {
        workspace_root.join("target")
    };
    fs::create_dir_all(&candidate).map_err(|_| "canonical target cannot be created".to_string())?;
    let resolved = candidate
        .canonicalize()
        .map_err(|_| "canonical target cannot be resolved".to_string())?;
    if !resolved.starts_with(workspace_root) {
        return Err("canonical target must stay inside the workspace".to_string());
    }
    Ok(resolved)
}

fn workspace_patch(workspace_root: &Path) -> Result<String, String> {
    let manifest = read_bounded_utf8(&workspace_root.join("Cargo.toml"), MAX_FIXTURE_BYTES)?;
    let parsed: toml::Value =
        toml::from_str(&manifest).map_err(|_| "workspace manifest is invalid".to_string())?;
    let Some(patch) = parsed.get("patch") else {
        return Ok(String::new());
    };
    let mut projection = toml::map::Map::new();
    projection.insert("patch".to_string(), patch.clone());
    let rendered = toml::to_string(&toml::Value::Table(projection))
        .map_err(|_| "workspace patch cannot be projected".to_string())?;
    Ok(format!("\n{rendered}"))
}

fn validate_input_file(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "input file cannot be inspected".to_string())?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err("input must be a regular file".to_string());
    }
    if metadata.len() > MAX_FIXTURE_BYTES {
        return Err("input exceeds byte limit".to_string());
    }
    Ok(())
}

fn read_bounded_utf8(path: &Path, limit: u64) -> Result<String, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "bounded file cannot be inspected".to_string())?;
    if !metadata.file_type().is_file() || metadata.len() > limit {
        return Err("bounded file exceeds policy".to_string());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|_| "bounded file cannot be read".to_string())?;
    String::from_utf8(bytes).map_err(|_| "bounded file is not UTF-8".to_string())
}

fn normalized_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn toml_string(value: &str) -> String {
    let escaped = value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            character => vec![character],
        })
        .collect::<String>();
    format!("\"{escaped}\"")
}
