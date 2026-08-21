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
    pub navigation: Duration,
    pub test: Duration,
}

#[derive(Clone, Debug)]
pub struct ArtifactSection {
    pub directory: PathBuf,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            project: ProjectSection::default(),
            browser: BrowserSection {
                headless: true,
                channel: BrowserChannel::Managed,
                path: None,
            },
            timeouts: TimeoutSection {
                browser_command: Duration::from_secs(10),
                navigation: Duration::from_secs(30),
                test: Duration::from_secs(60),
            },
            artifacts: ArtifactSection {
                directory: PathBuf::from(".webtest/artifacts"),
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
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize)]
struct RawTimeouts {
    browser_command: Option<String>,
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
    for test_root in &raw.project.test_roots {
        validate_project_relative(path, "project.test_roots", test_root)?;
    }
    if let Some(directory) = &raw.artifacts.directory {
        validate_project_relative(path, "artifacts.directory", directory)?;
    }
    let timeout = |key: &str, value: Option<String>, default: Duration| {
        value.map_or(Ok(default), |value| {
            parse_duration(&value).ok_or_else(|| ProjectError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!("timeouts.{key} has invalid duration `{value}`"),
            })
        })
    };
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
            },
            timeouts: TimeoutSection {
                browser_command: timeout(
                    "browser_command",
                    raw.timeouts.browser_command,
                    defaults.timeouts.browser_command,
                )?,
                navigation: timeout(
                    "navigation",
                    raw.timeouts.navigation,
                    defaults.timeouts.navigation,
                )?,
                test: timeout("test", raw.timeouts.test, defaults.timeouts.test)?,
            },
            artifacts: ArtifactSection {
                directory: raw
                    .artifacts
                    .directory
                    .unwrap_or(defaults.artifacts.directory),
            },
        },
        warnings,
    ))
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
