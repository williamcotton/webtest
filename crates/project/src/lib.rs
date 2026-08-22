//! Project configuration, root selection, and deterministic WebTest file discovery.

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use thiserror::Error;

const CONFIG_NAME: &str = "webtest.toml";

#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub config_path: Option<PathBuf>,
    pub config: ProjectConfig,
    pub warnings: Vec<ConfigWarning>,
    pub files: Vec<DiscoveredFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub path: PathBuf,
    pub display_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigWarning {
    pub key: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    pub browser: BrowserSection,
    pub timeouts: TimeoutSection,
    pub artifacts: ArtifactSection,
    pub evidence: EvidenceSection,
    pub server: ServerSection,
    pub redaction: RedactionSection,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectSection {
    pub name: Option<String>,
    pub test_roots: Vec<PathBuf>,
    pub exclude: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BrowserSection {
    pub headless: bool,
    pub channel: BrowserChannel,
    pub path: Option<PathBuf>,
    pub base_url: Option<String>,
    pub viewport: ViewportSection,
    pub test_id_attribute: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportSection {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowserChannel {
    #[default]
    Managed,
    System,
}

#[derive(Clone, Debug)]
pub struct TimeoutSection {
    pub browser_command: Duration,
    pub action: Duration,
    pub assertion: Duration,
    pub navigation: Duration,
    pub test: Duration,
}

#[derive(Clone, Debug)]
pub struct ArtifactSection {
    pub directory: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceMode {
    Off,
    OnFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceSection {
    pub screenshot: EvidenceMode,
    pub dom_snapshot: EvidenceMode,
    pub max_dom_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ServerSection {
    pub base_url: Option<String>,
    pub http: ServerHttpSection,
    pub process: ServerProcessSection,
    pub fs: ServerFsSection,
}

#[derive(Clone, Debug)]
pub struct ServerHttpSection {
    pub follow_redirects: bool,
    pub max_response_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ServerProcessSection {
    pub allowed_working_roots: Vec<PathBuf>,
    pub max_output_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct ServerFsSection {
    pub read_roots: Vec<PathBuf>,
    pub write_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RedactionSection {
    pub headers: Vec<String>,
    pub json_fields: Vec<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            project: ProjectSection::default(),
            browser: BrowserSection {
                headless: true,
                channel: BrowserChannel::Managed,
                path: None,
                base_url: None,
                viewport: ViewportSection {
                    width: 1280,
                    height: 720,
                },
                test_id_attribute: "data-testid".into(),
            },
            timeouts: TimeoutSection {
                browser_command: Duration::from_secs(10),
                action: Duration::from_secs(5),
                assertion: Duration::from_secs(5),
                navigation: Duration::from_secs(30),
                test: Duration::from_secs(60),
            },
            artifacts: ArtifactSection {
                directory: PathBuf::from(".webtest/artifacts"),
            },
            evidence: EvidenceSection {
                screenshot: EvidenceMode::OnFailure,
                dom_snapshot: EvidenceMode::OnFailure,
                max_dom_bytes: 1_048_576,
            },
            server: ServerSection {
                base_url: None,
                http: ServerHttpSection {
                    follow_redirects: true,
                    max_response_bytes: 8_388_608,
                },
                process: ServerProcessSection {
                    allowed_working_roots: vec![PathBuf::from(".")],
                    max_output_bytes: 1_048_576,
                },
                fs: ServerFsSection {
                    read_roots: vec![PathBuf::from("fixtures")],
                    write_root: PathBuf::from(".webtest/tmp"),
                },
            },
            redaction: RedactionSection {
                headers: vec!["authorization".into(), "cookie".into(), "set-cookie".into()],
                json_fields: vec!["password".into(), "token".into(), "secret".into()],
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("could not determine the current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    #[error("input `{path}` does not exist")]
    MissingInput { path: PathBuf },
    #[error("could not inspect `{path}`: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("inputs belong to different WebTest projects: `{first}` and `{second}`")]
    MultipleProjects { first: PathBuf, second: PathBuf },
    #[error("could not read configuration `{path}`: {source}")]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration `{path}`: {message}")]
    InvalidConfig { path: PathBuf, message: String },
    #[error("invalid exclude pattern `{pattern}`: {message}")]
    InvalidExclude { pattern: String, message: String },
    #[error("could not read directory `{path}`: {source}")]
    ReadDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not canonicalize `{path}`: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    project: RawProject,
    #[serde(default)]
    browser: RawBrowser,
    #[serde(default)]
    timeouts: RawTimeouts,
    #[serde(default)]
    artifacts: RawArtifacts,
    #[serde(default)]
    evidence: RawEvidence,
    #[serde(default)]
    server: RawServer,
    #[serde(default)]
    redaction: RawRedaction,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawProject {
    name: Option<String>,
    #[serde(default)]
    test_roots: Vec<PathBuf>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawBrowser {
    headless: Option<bool>,
    channel: Option<String>,
    path: Option<PathBuf>,
    base_url: Option<String>,
    viewport: Option<RawViewport>,
    test_id_attribute: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Deserialize)]
struct RawViewport {
    width: u32,
    height: u32,
}

#[derive(Debug, Default, Deserialize)]
struct RawTimeouts {
    browser_command: Option<String>,
    action: Option<String>,
    assertion: Option<String>,
    navigation: Option<String>,
    test: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawArtifacts {
    directory: Option<PathBuf>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawEvidence {
    screenshot: Option<String>,
    dom_snapshot: Option<String>,
    max_dom_bytes: Option<usize>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawServer {
    base_url: Option<String>,
    #[serde(default)]
    http: RawServerHttp,
    #[serde(default)]
    process: RawServerProcess,
    #[serde(default)]
    fs: RawServerFs,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawServerHttp {
    follow_redirects: Option<bool>,
    max_response_bytes: Option<usize>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawServerProcess {
    #[serde(default)]
    allowed_working_roots: Vec<PathBuf>,
    max_output_bytes: Option<usize>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawServerFs {
    #[serde(default)]
    read_roots: Vec<PathBuf>,
    write_root: Option<PathBuf>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawRedaction {
    #[serde(default)]
    headers: Vec<String>,
    #[serde(default)]
    json_fields: Vec<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

pub fn discover(inputs: &[PathBuf]) -> Result<Project, ProjectError> {
    let cwd = std::env::current_dir().map_err(ProjectError::CurrentDirectory)?;
    discover_from(&cwd, inputs)
}

pub fn discover_from(cwd: &Path, inputs: &[PathBuf]) -> Result<Project, ProjectError> {
    let cwd = canonicalize_existing(cwd)?;
    let explicit = resolve_explicit_inputs(&cwd, inputs)?;
    let roots_to_check: Vec<PathBuf> = if explicit.is_empty() {
        vec![cwd.clone()]
    } else {
        explicit.clone()
    };

    let mut configured_roots = BTreeSet::new();
    for input in &roots_to_check {
        let start = if input.is_dir() {
            input.as_path()
        } else {
            input.parent().unwrap_or(input)
        };
        if let Some(config) = find_config(start) {
            configured_roots.insert(config.parent().unwrap_or(start).to_path_buf());
        }
    }
    if configured_roots.len() > 1 {
        let mut roots = configured_roots.into_iter();
        let first = roots.next().ok_or_else(|| ProjectError::InvalidConfig {
            path: cwd.join(CONFIG_NAME),
            message: "project root resolution lost its first root".into(),
        })?;
        let second = roots.next().ok_or_else(|| ProjectError::InvalidConfig {
            path: cwd.join(CONFIG_NAME),
            message: "project root resolution lost its second root".into(),
        })?;
        return Err(ProjectError::MultipleProjects { first, second });
    }

    let root = configured_roots.into_iter().next().unwrap_or(cwd);
    let config_path = root.join(CONFIG_NAME);
    let (config, warnings) = if config_path.is_file() {
        let text = fs::read_to_string(&config_path).map_err(|source| ProjectError::ReadConfig {
            path: config_path.clone(),
            source,
        })?;
        let (config, warnings) = parse_config(&config_path, &text)?;
        (config, warnings)
    } else {
        (ProjectConfig::default(), Vec::new())
    };
    let config_path = config_path.is_file().then_some(config_path);
    let matcher = exclude_matcher(&config.project.exclude)?;
    let search_inputs = if explicit.is_empty() {
        if config.project.test_roots.is_empty() {
            vec![root.clone()]
        } else {
            config
                .project
                .test_roots
                .iter()
                .map(|path| root.join(path))
                .collect()
        }
    } else {
        explicit
    };
    let files = discover_files(&root, &search_inputs, &matcher)?;

    Ok(Project {
        root,
        config_path,
        config,
        warnings,
        files,
    })
}

fn resolve_explicit_inputs(cwd: &Path, inputs: &[PathBuf]) -> Result<Vec<PathBuf>, ProjectError> {
    inputs
        .iter()
        .map(|input| {
            let path = if input.is_absolute() {
                input.clone()
            } else {
                cwd.join(input)
            };
            if !path.exists() {
                return Err(ProjectError::MissingInput { path });
            }
            canonicalize_existing(&path)
        })
        .collect()
}

fn find_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|directory| directory.join(CONFIG_NAME))
        .find(|candidate| candidate.is_file())
}

fn parse_config(
    path: &Path,
    text: &str,
) -> Result<(ProjectConfig, Vec<ConfigWarning>), ProjectError> {
    let raw: RawConfig = toml::from_str(text).map_err(|error| ProjectError::InvalidConfig {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut warnings = Vec::new();
    collect_unknown(&mut warnings, "", &raw.extra);
    collect_unknown(&mut warnings, "project", &raw.project.extra);
    collect_unknown(&mut warnings, "browser", &raw.browser.extra);
    collect_unknown(&mut warnings, "timeouts", &raw.timeouts.extra);
    collect_unknown(&mut warnings, "artifacts", &raw.artifacts.extra);
    collect_unknown(&mut warnings, "evidence", &raw.evidence.extra);
    collect_unknown(&mut warnings, "server", &raw.server.extra);
    collect_unknown(&mut warnings, "server.http", &raw.server.http.extra);
    collect_unknown(&mut warnings, "server.process", &raw.server.process.extra);
    collect_unknown(&mut warnings, "server.fs", &raw.server.fs.extra);
    collect_unknown(&mut warnings, "redaction", &raw.redaction.extra);

    let channel = match raw.browser.channel.as_deref().unwrap_or("managed") {
        "managed" => BrowserChannel::Managed,
        "system" => BrowserChannel::System,
        value => {
            return Err(ProjectError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("browser.channel must be `managed` or `system`, got `{value}`"),
            });
        }
    };
    if channel == BrowserChannel::System && raw.browser.path.is_some() {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "browser.path cannot be combined with browser.channel = `system`".into(),
        });
    }
    let defaults = ProjectConfig::default();
    if let Some(base_url) = &raw.browser.base_url
        && !is_absolute_url(base_url)
    {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "browser.base_url must be an absolute URL".into(),
        });
    }
    if let Some(base_url) = &raw.server.base_url
        && !is_absolute_url(base_url)
    {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "server.base_url must be an absolute URL".into(),
        });
    }
    if raw.server.http.max_response_bytes == Some(0) {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "server.http.max_response_bytes must be positive".into(),
        });
    }
    if raw.server.process.max_output_bytes == Some(0) {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "server.process.max_output_bytes must be positive".into(),
        });
    }
    if raw
        .browser
        .test_id_attribute
        .as_deref()
        .is_some_and(str::is_empty)
    {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "browser.test_id_attribute must not be empty".into(),
        });
    }
    if let Some(viewport) = &raw.browser.viewport
        && (viewport.width == 0
            || viewport.height == 0
            || viewport.width > 16_384
            || viewport.height > 16_384)
    {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "browser.viewport dimensions must be between 1 and 16384".into(),
        });
    }
    for test_root in &raw.project.test_roots {
        validate_project_relative(path, "project.test_roots", test_root)?;
    }
    if let Some(directory) = &raw.artifacts.directory {
        validate_project_relative(path, "artifacts.directory", directory)?;
    }
    for root in &raw.server.process.allowed_working_roots {
        validate_project_relative(path, "server.process.allowed_working_roots", root)?;
    }
    for root in &raw.server.fs.read_roots {
        validate_project_relative(path, "server.fs.read_roots", root)?;
    }
    if let Some(root) = &raw.server.fs.write_root {
        validate_project_relative(path, "server.fs.write_root", root)?;
    }
    let timeout = |key: &str, value: Option<String>, default: Duration| {
        value.map_or(Ok(default), |value| {
            parse_duration(&value).ok_or_else(|| ProjectError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("timeouts.{key} has invalid duration `{value}`"),
            })
        })
    };
    let resolved_timeouts = TimeoutSection {
        browser_command: timeout(
            "browser_command",
            raw.timeouts.browser_command,
            defaults.timeouts.browser_command,
        )?,
        action: timeout("action", raw.timeouts.action, defaults.timeouts.action)?,
        assertion: timeout(
            "assertion",
            raw.timeouts.assertion,
            defaults.timeouts.assertion,
        )?,
        navigation: timeout(
            "navigation",
            raw.timeouts.navigation,
            defaults.timeouts.navigation,
        )?,
        test: timeout("test", raw.timeouts.test, defaults.timeouts.test)?,
    };
    for (name, value) in [
        ("action", resolved_timeouts.action),
        ("assertion", resolved_timeouts.assertion),
        ("navigation", resolved_timeouts.navigation),
    ] {
        if value > resolved_timeouts.test {
            return Err(ProjectError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("timeouts.{name} must not exceed timeouts.test"),
            });
        }
    }
    let evidence_mode = |key: &str, value: Option<String>, default| match value.as_deref() {
        None => Ok(default),
        Some("off") => Ok(EvidenceMode::Off),
        Some("on-failure") => Ok(EvidenceMode::OnFailure),
        Some(value) => Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: format!("evidence.{key} must be `off` or `on-failure`, got `{value}`"),
        }),
    };
    if raw.evidence.max_dom_bytes == Some(0) {
        return Err(ProjectError::InvalidConfig {
            path: path.to_path_buf(),
            message: "evidence.max_dom_bytes must be positive".into(),
        });
    }
    Ok((
        ProjectConfig {
            project: ProjectSection {
                name: raw.project.name,
                test_roots: raw.project.test_roots,
                exclude: raw.project.exclude,
            },
            browser: BrowserSection {
                headless: raw.browser.headless.unwrap_or(defaults.browser.headless),
                channel,
                path: raw.browser.path,
                base_url: raw.browser.base_url,
                viewport: raw
                    .browser
                    .viewport
                    .map_or(defaults.browser.viewport, |viewport| ViewportSection {
                        width: viewport.width,
                        height: viewport.height,
                    }),
                test_id_attribute: raw
                    .browser
                    .test_id_attribute
                    .unwrap_or(defaults.browser.test_id_attribute),
            },
            timeouts: resolved_timeouts,
            artifacts: ArtifactSection {
                directory: raw
                    .artifacts
                    .directory
                    .unwrap_or(defaults.artifacts.directory),
            },
            evidence: EvidenceSection {
                screenshot: evidence_mode(
                    "screenshot",
                    raw.evidence.screenshot,
                    defaults.evidence.screenshot,
                )?,
                dom_snapshot: evidence_mode(
                    "dom_snapshot",
                    raw.evidence.dom_snapshot,
                    defaults.evidence.dom_snapshot,
                )?,
                max_dom_bytes: raw
                    .evidence
                    .max_dom_bytes
                    .unwrap_or(defaults.evidence.max_dom_bytes),
            },
            server: ServerSection {
                base_url: raw.server.base_url,
                http: ServerHttpSection {
                    follow_redirects: raw
                        .server
                        .http
                        .follow_redirects
                        .unwrap_or(defaults.server.http.follow_redirects),
                    max_response_bytes: raw
                        .server
                        .http
                        .max_response_bytes
                        .unwrap_or(defaults.server.http.max_response_bytes),
                },
                process: ServerProcessSection {
                    allowed_working_roots: if raw.server.process.allowed_working_roots.is_empty() {
                        defaults.server.process.allowed_working_roots
                    } else {
                        raw.server.process.allowed_working_roots
                    },
                    max_output_bytes: raw
                        .server
                        .process
                        .max_output_bytes
                        .unwrap_or(defaults.server.process.max_output_bytes),
                },
                fs: ServerFsSection {
                    read_roots: if raw.server.fs.read_roots.is_empty() {
                        defaults.server.fs.read_roots
                    } else {
                        raw.server.fs.read_roots
                    },
                    write_root: raw
                        .server
                        .fs
                        .write_root
                        .unwrap_or(defaults.server.fs.write_root),
                },
            },
            redaction: RedactionSection {
                headers: if raw.redaction.headers.is_empty() {
                    defaults.redaction.headers
                } else {
                    raw.redaction.headers
                },
                json_fields: if raw.redaction.json_fields.is_empty() {
                    defaults.redaction.json_fields
                } else {
                    raw.redaction.json_fields
                },
            },
        },
        warnings,
    ))
}

fn is_absolute_url(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && scheme.chars().next().is_some_and(char::is_alphabetic)
            && scheme.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            })
            && !rest.is_empty()
    })
}

fn validate_project_relative(
    config_path: &Path,
    key: &str,
    value: &Path,
) -> Result<(), ProjectError> {
    if value.is_absolute()
        || value
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return Err(ProjectError::InvalidConfig {
            path: config_path.to_path_buf(),
            message: format!(
                "{key} path `{}` must stay within the project",
                value.display()
            ),
        });
    }
    Ok(())
}

fn collect_unknown(warnings: &mut Vec<ConfigWarning>, section: &str, values: &toml::Table) {
    for key in values.keys() {
        let key = if section.is_empty() {
            key.clone()
        } else {
            format!("{section}.{key}")
        };
        warnings.push(ConfigWarning {
            message: format!("unknown configuration key `{key}`"),
            key,
        });
    }
}

fn parse_duration(value: &str) -> Option<Duration> {
    let (number, multiplier) = if let Some(number) = value.strip_suffix("ms") {
        (number, 1u64)
    } else if let Some(number) = value.strip_suffix('s') {
        (number, 1_000)
    } else if let Some(number) = value.strip_suffix('m') {
        (number, 60_000)
    } else {
        return None;
    };
    let number = number.parse::<u64>().ok()?;
    (number > 0).then(|| Duration::from_millis(number.saturating_mul(multiplier)))
}

fn exclude_matcher(patterns: &[String]) -> Result<GlobSet, ProjectError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|error| ProjectError::InvalidExclude {
            pattern: pattern.clone(),
            message: error.to_string(),
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| ProjectError::InvalidExclude {
            pattern: "<set>".into(),
            message: error.to_string(),
        })
}

fn discover_files(
    root: &Path,
    inputs: &[PathBuf],
    excludes: &GlobSet,
) -> Result<Vec<DiscoveredFile>, ProjectError> {
    let mut canonical = HashSet::new();
    let mut files = Vec::new();
    for input in inputs {
        if !input.exists() {
            return Err(ProjectError::MissingInput {
                path: input.clone(),
            });
        }
        if input.is_file() {
            add_file(root, input, &mut canonical, &mut files)?;
        } else if input.is_dir() {
            walk_directory(root, input, excludes, &mut canonical, &mut files)?;
        } else {
            return Err(ProjectError::MissingInput {
                path: input.clone(),
            });
        }
    }
    files.sort_by(|left, right| {
        normalized(&left.display_path).cmp(&normalized(&right.display_path))
    });
    Ok(files)
}

fn walk_directory(
    root: &Path,
    directory: &Path,
    excludes: &GlobSet,
    canonical: &mut HashSet<PathBuf>,
    files: &mut Vec<DiscoveredFile>,
) -> Result<(), ProjectError> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map_err(|source| ProjectError::ReadDirectory {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<_, _>>()
        .map_err(|source| ProjectError::ReadDirectory {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let file_type = entry.file_type().map_err(|source| ProjectError::Inspect {
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() && name.to_string_lossy().starts_with('.') {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if excludes.is_match(normalized(relative)) {
            continue;
        }
        if file_type.is_dir() {
            walk_directory(root, &path, excludes, canonical, files)?;
        } else if file_type.is_file() && path.extension().is_some_and(|value| value == "webtest") {
            add_file(root, &path, canonical, files)?;
        }
    }
    Ok(())
}

fn add_file(
    root: &Path,
    path: &Path,
    canonical: &mut HashSet<PathBuf>,
    files: &mut Vec<DiscoveredFile>,
) -> Result<(), ProjectError> {
    if path.extension().is_none_or(|value| value != "webtest") {
        return Ok(());
    }
    let path = canonicalize_existing(path)?;
    if !canonical.insert(path.clone()) {
        return Ok(());
    }
    let display_path = path
        .strip_prefix(root)
        .map_or_else(|_| path.clone(), Path::to_path_buf);
    files.push(DiscoveredFile { path, display_path });
    Ok(())
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf, ProjectError> {
    fs::canonicalize(path).map_err(|source| ProjectError::Canonicalize {
        path: path.to_path_buf(),
        source,
    })
}

fn normalized(path: &Path) -> String {
    let path = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        path.into_owned()
    } else {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn discovers_configured_roots_excludes_hidden_and_sorts() {
        let directory = tempfile::tempdir().expect("temp directory");
        write(
            &directory.path().join("webtest.toml"),
            "[project]\ntest_roots = [\"tests\"]\nexclude = [\"tests/generated/**\"]\n",
        );
        write(&directory.path().join("tests/z.webtest"), "test \"z\" {}");
        write(&directory.path().join("tests/a.webtest"), "test \"a\" {}");
        write(
            &directory.path().join("tests/generated/no.webtest"),
            "test \"no\" {}",
        );
        write(
            &directory.path().join("tests/.hidden/no.webtest"),
            "test \"no\" {}",
        );

        let project = discover_from(directory.path(), &[]).expect("discover project");
        let paths: Vec<_> = project
            .files
            .iter()
            .map(|file| normalized(&file.display_path))
            .collect();
        assert_eq!(paths, ["tests/a.webtest", "tests/z.webtest"]);
    }

    #[test]
    fn nearest_config_wins_and_mixed_roots_fail() {
        let directory = tempfile::tempdir().expect("temp directory");
        write(&directory.path().join("webtest.toml"), "");
        write(
            &directory.path().join("nested/webtest.toml"),
            "[project]\nname = \"nested\"\n",
        );
        write(&directory.path().join("root.webtest"), "test \"root\" {}");
        write(
            &directory.path().join("nested/test.webtest"),
            "test \"nested\" {}",
        );

        let nested = discover_from(&directory.path().join("nested"), &[]).expect("nested");
        assert_eq!(nested.config.project.name.as_deref(), Some("nested"));
        let error = discover_from(
            directory.path(),
            &[
                PathBuf::from("root.webtest"),
                PathBuf::from("nested/test.webtest"),
            ],
        )
        .expect_err("mixed roots");
        assert!(matches!(error, ProjectError::MultipleProjects { .. }));
    }

    #[test]
    fn parses_durations_unknown_keys_and_rejects_contradictions() {
        let path = Path::new("webtest.toml");
        let (config, warnings) = parse_config(
            path,
            "mystery = true\n[timeouts]\nbrowser_command = \"250ms\"\n[test]\nfuture = true\n",
        )
        .expect("config");
        assert_eq!(config.timeouts.browser_command, Duration::from_millis(250));
        assert_eq!(warnings[0].key, "mystery");
        assert_eq!(warnings[1].key, "test");

        let error = parse_config(path, "[browser]\nchannel = \"system\"\npath = \"chrome\"\n")
            .expect_err("contradictory browser config");
        assert!(error.to_string().contains("cannot be combined"));

        let error = parse_config(path, "[project]\ntest_roots = [\"../outside\"]\n")
            .expect_err("escaping test root");
        assert!(error.to_string().contains("must stay within the project"));

        let error = parse_config(path, "[artifacts]\ndirectory = \"/tmp/results\"\n")
            .expect_err("absolute artifact directory");
        assert!(error.to_string().contains("must stay within the project"));
    }

    #[test]
    fn invalid_toml_and_durations_are_configuration_errors() {
        let path = Path::new("webtest.toml");
        assert!(matches!(
            parse_config(path, "[project\n"),
            Err(ProjectError::InvalidConfig { .. })
        ));
        assert!(matches!(
            parse_config(path, "[timeouts]\ntest = \"0s\"\n"),
            Err(ProjectError::InvalidConfig { .. })
        ));
    }

    #[test]
    fn parses_typed_server_provider_and_redaction_configuration() {
        let path = Path::new("webtest.toml");
        let (config, warnings) = parse_config(
            path,
            r#"
[server]
base_url = "http://127.0.0.1:3000"

[server.http]
follow_redirects = false
max_response_bytes = 4096

[server.process]
allowed_working_roots = ["bin"]
max_output_bytes = 2048

[server.fs]
read_roots = ["fixtures", "data"]
write_root = ".webtest/generated"

[redaction]
headers = ["authorization"]
json_fields = ["credential"]
"#,
        )
        .expect("server configuration");
        assert!(warnings.is_empty());
        assert_eq!(
            config.server.base_url.as_deref(),
            Some("http://127.0.0.1:3000")
        );
        assert!(!config.server.http.follow_redirects);
        assert_eq!(config.server.http.max_response_bytes, 4096);
        assert_eq!(
            config.server.process.allowed_working_roots,
            [PathBuf::from("bin")]
        );
        assert_eq!(
            config.server.fs.write_root,
            PathBuf::from(".webtest/generated")
        );
        assert_eq!(config.redaction.json_fields, ["credential"]);

        assert!(parse_config(path, "[server]\nbase_url = \"/relative\"\n").is_err());
        assert!(parse_config(path, "[server.fs]\nwrite_root = \"../outside\"\n").is_err());
    }

    #[test]
    fn parses_browser_timeouts_and_evidence() {
        let path = Path::new("webtest.toml");
        let (config, warnings) = parse_config(
            path,
            r#"
[browser]
base_url = "http://127.0.0.1:3000"
viewport = { width = 1440, height = 900 }
test_id_attribute = "data-test"

[timeouts]
action = "2s"
assertion = "3s"
navigation = "10s"
test = "20s"

[evidence]
screenshot = "on-failure"
dom_snapshot = "off"
max_dom_bytes = 4096
"#,
        )
        .expect("browser config");
        assert!(warnings.is_empty());
        assert_eq!(
            config.browser.base_url.as_deref(),
            Some("http://127.0.0.1:3000")
        );
        assert_eq!(
            config.browser.viewport,
            ViewportSection {
                width: 1440,
                height: 900
            }
        );
        assert_eq!(config.browser.test_id_attribute, "data-test");
        assert_eq!(config.timeouts.action, Duration::from_secs(2));
        assert_eq!(config.timeouts.assertion, Duration::from_secs(3));
        assert_eq!(config.evidence.dom_snapshot, EvidenceMode::Off);
        assert_eq!(config.evidence.max_dom_bytes, 4096);

        assert!(parse_config(path, "[browser]\nbase_url = \"/relative\"\n").is_err());
        assert!(parse_config(path, "[browser]\nviewport = { width = 0, height = 10 }\n").is_err());
        assert!(parse_config(path, "[timeouts]\naction = \"61s\"\ntest = \"60s\"\n").is_err());
    }

    #[test]
    fn unicode_paths_are_preserved_and_ordered_deterministically() {
        let directory = tempfile::tempdir().expect("temp directory");
        write(&directory.path().join("tests/é.webtest"), "test \"é\" {}");
        write(&directory.path().join("tests/β.webtest"), "test \"β\" {}");
        let project = discover_from(directory.path(), &[PathBuf::from("tests")])
            .expect("discover Unicode paths");
        let paths = project
            .files
            .iter()
            .map(|file| normalized(&file.display_path))
            .collect::<Vec<_>>();
        assert_eq!(paths, ["tests/é.webtest", "tests/β.webtest"]);
    }

    #[test]
    fn explicit_duplicate_paths_are_deduplicated() {
        let directory = tempfile::tempdir().expect("temp directory");
        write(&directory.path().join("same.webtest"), "test \"same\" {}");
        let project = discover_from(
            directory.path(),
            &[PathBuf::from("same.webtest"), PathBuf::from("same.webtest")],
        )
        .expect("discover");
        assert_eq!(project.files.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn directory_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temp directory");
        write(
            &directory.path().join("outside/test.webtest"),
            "test \"outside\" {}",
        );
        fs::create_dir_all(directory.path().join("tests")).expect("tests directory");
        symlink(
            directory.path().join("outside"),
            directory.path().join("tests/link"),
        )
        .expect("symlink");
        let project =
            discover_from(directory.path(), &[directory.path().join("tests")]).expect("discover");
        assert!(project.files.is_empty());
    }
}
