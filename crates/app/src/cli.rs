use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use crate::report::Reporter;

#[derive(Debug, Parser)]
#[command(
    name = "webtest",
    version,
    about = "Statically analyzable web application tests"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Create a minimal WebTest project, application schema, example, and agent skill.
    Init {
        /// Directory to initialize.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Parse and statically check WebTest files.
    Check {
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = CheckReporter::Human)]
        reporter: CheckReporter,
    },
    /// Rewrite WebTest files using the canonical formatter.
    Fmt {
        paths: Vec<PathBuf>,
        /// Report files that differ without rewriting them.
        #[arg(long)]
        check: bool,
    },
    /// Analyze WebTest files and emit a versioned execution plan.
    Build {
        paths: Vec<PathBuf>,
        /// Destination for the serialized plan envelope.
        #[arg(long)]
        emit: PathBuf,
    },
    /// Execute WebTest files in Chrome.
    Test {
        paths: Vec<PathBuf>,
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// Show the Chrome window while tests run.
        #[arg(long)]
        headed: bool,
        #[arg(long, value_enum, default_value_t = TestReporter::Human)]
        reporter: TestReporter,
    },
    /// Inspect the semantic interaction and assertion surface of one page.
    Inspect {
        url: Option<String>,
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// Show the Chrome window while inspecting.
        #[arg(long)]
        headed: bool,
        #[arg(long, value_enum, default_value_t = ReferenceReporter::Human)]
        reporter: ReferenceReporter,
    },
    /// Describe the installed language and project-visible provider surface.
    Describe {
        #[arg(conflicts_with = "search")]
        query: Option<String>,
        #[arg(long, conflicts_with = "query")]
        search: Option<String>,
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = ReferenceReporter::Human)]
        reporter: ReferenceReporter,
    },
    /// Install and inspect managed Chrome for Testing versions.
    Browser {
        #[command(subcommand)]
        command: BrowserCommand,
    },
    /// Run the language server over stdio.
    Lsp {
        #[arg(long)]
        chrome_path: Option<PathBuf>,
    },
    /// Run the debug adapter protocol server over stdio.
    Dap {
        #[arg(long)]
        chrome_path: Option<PathBuf>,
        /// File or directory used to discover the nearest WebTest project.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Hide Chrome while debugging. Debug sessions are headed by default.
        #[arg(long)]
        headless: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BrowserCommand {
    /// Download and atomically install the tested Chrome for Testing version.
    Install {
        #[arg(long)]
        version: Option<String>,
    },
    /// List valid managed installations.
    List,
    /// Print the Chrome executable selected by normal resolution precedence.
    Path,
    /// Remove validated WebTest-owned managed installations.
    Clean {
        #[arg(long)]
        version: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CheckReporter {
    Human,
    Concise,
    Json,
}

impl From<CheckReporter> for Reporter {
    fn from(value: CheckReporter) -> Self {
        match value {
            CheckReporter::Human => Self::Human,
            CheckReporter::Concise => Self::Concise,
            CheckReporter::Json => Self::Json,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum TestReporter {
    Human,
    Concise,
    Json,
    Junit,
    Events,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReferenceReporter {
    Human,
    Json,
}

impl From<TestReporter> for Reporter {
    fn from(value: TestReporter) -> Self {
        match value {
            TestReporter::Human => Self::Human,
            TestReporter::Concise => Self::Concise,
            TestReporter::Json => Self::Json,
            TestReporter::Junit => Self::Junit,
            TestReporter::Events => Self::Events,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn every_top_level_command_parses() {
        let cases: &[&[&str]] = &[
            &["webtest", "init"],
            &["webtest", "check"],
            &["webtest", "fmt"],
            &["webtest", "build", "--emit", "plan.json"],
            &["webtest", "test"],
            &["webtest", "inspect", "https://example.test"],
            &["webtest", "describe"],
            &["webtest", "browser", "list"],
            &["webtest", "lsp"],
            &["webtest", "dap"],
        ];
        for arguments in cases {
            Cli::try_parse_from(*arguments).expect("command should parse");
        }
    }

    #[test]
    fn browser_subcommands_parse() {
        for arguments in [
            &["webtest", "browser", "install"][..],
            &["webtest", "browser", "list"][..],
            &["webtest", "browser", "path"][..],
            &["webtest", "browser", "clean"][..],
        ] {
            Cli::try_parse_from(arguments).expect("browser command should parse");
        }
    }

    #[test]
    fn commands_accept_multiple_paths_and_reporters() {
        let cli = Cli::try_parse_from([
            "webtest",
            "test",
            "a.webtest",
            "tests",
            "--headed",
            "--reporter",
            "junit",
        ])
        .expect("parse test command");
        assert!(matches!(
            cli.command,
            Command::Test {
                paths,
                headed: true,
                reporter: TestReporter::Junit,
                ..
            } if paths.len() == 2
        ));
    }

    #[test]
    fn reporter_defaults_and_conversions_are_stable() {
        let check = Cli::try_parse_from(["webtest", "check"]).expect("check");
        assert!(matches!(
            check.command,
            Command::Check {
                reporter: CheckReporter::Human,
                ..
            }
        ));
        let test = Cli::try_parse_from(["webtest", "test"]).expect("test");
        assert!(matches!(
            test.command,
            Command::Test {
                reporter: TestReporter::Human,
                ..
            }
        ));
        assert_eq!(Reporter::from(CheckReporter::Concise), Reporter::Concise);
        assert_eq!(Reporter::from(TestReporter::Events), Reporter::Events);
    }

    #[test]
    fn describe_query_and_search_conflict() {
        let error =
            Cli::try_parse_from(["webtest", "describe", "locator.role", "--search", "role"])
                .expect_err("query and search must conflict");
        assert!(error.use_stderr());
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn dap_is_headed_by_default_and_accepts_headless_mode() {
        let headed = Cli::try_parse_from(["webtest", "dap"]).expect("parse headed DAP command");
        assert!(matches!(
            headed.command,
            Command::Dap {
                headless: false,
                ..
            }
        ));
        let headless =
            Cli::try_parse_from(["webtest", "dap", "--headless"]).expect("parse headless DAP");
        assert!(matches!(
            headless.command,
            Command::Dap { headless: true, .. }
        ));
    }

    #[test]
    fn help_version_and_parse_error_exit_behavior_is_stable() {
        for arguments in [["webtest", "--help"], ["webtest", "--version"]] {
            let error = Cli::try_parse_from(arguments).expect_err("display request");
            assert!(!error.use_stderr());
            assert_eq!(error.exit_code(), 0);
        }
        let error = Cli::try_parse_from(["webtest", "unknown"]).expect_err("parse error");
        assert!(error.use_stderr());
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn root_help_keeps_the_complete_command_surface() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.starts_with(
            "Statically analyzable web application tests\n\nUsage: webtest <COMMAND>"
        ));
        for command in [
            "init", "check", "fmt", "build", "test", "inspect", "describe", "browser", "lsp", "dap",
        ] {
            assert!(help.contains(&format!("  {command}")), "missing {command}");
        }
    }
}
