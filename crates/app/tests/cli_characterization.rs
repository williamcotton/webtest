use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

fn run(current_dir: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_webtest"))
        .current_dir(current_dir)
        .args(arguments)
        .output()
        .expect("run webtest")
}

fn fixture(source: &str) -> TempDir {
    let directory = tempfile::tempdir().expect("temporary project");
    fs::create_dir(directory.path().join("tests")).expect("test directory");
    fs::write(
        directory.path().join("webtest.toml"),
        "[project]\ntest_roots = [\"tests\"]\n",
    )
    .expect("project configuration");
    fs::write(directory.path().join("tests/example.webtest"), source).expect("test source");
    directory
}

#[test]
fn help_surface_is_byte_stable() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["--help"],
            "Statically analyzable web application tests\n\nUsage: webtest <COMMAND>\n\nCommands:\n  init      Create a minimal WebTest project, application schema, example, and agent skill\n  check     Parse and statically check WebTest files\n  fmt       Rewrite WebTest files using the canonical formatter\n  build     Analyze WebTest files and emit a versioned execution plan\n  test      Execute WebTest files in Chrome\n  inspect   Inspect the semantic interaction and assertion surface of one page\n  describe  Describe the installed language and project-visible provider surface\n  browser   Install and inspect managed Chrome for Testing versions\n  lsp       Run the language server over stdio\n  dap       Run the debug adapter protocol server over stdio\n  help      Print this message or the help of the given subcommand(s)\n\nOptions:\n  -h, --help     Print help\n  -V, --version  Print version\n",
        ),
        (
            &["init", "--help"],
            "Create a minimal WebTest project, application schema, example, and agent skill\n\nUsage: webtest init [PATH]\n\nArguments:\n  [PATH]  Directory to initialize [default: .]\n\nOptions:\n  -h, --help  Print help\n",
        ),
        (
            &["check", "--help"],
            "Parse and statically check WebTest files\n\nUsage: webtest check [OPTIONS] [PATHS]...\n\nArguments:\n  [PATHS]...  \n\nOptions:\n      --reporter <REPORTER>  [default: human] [possible values: human, concise, json]\n  -h, --help                 Print help\n",
        ),
        (
            &["fmt", "--help"],
            "Rewrite WebTest files using the canonical formatter\n\nUsage: webtest fmt [OPTIONS] [PATHS]...\n\nArguments:\n  [PATHS]...  \n\nOptions:\n      --check  Report files that differ without rewriting them\n  -h, --help   Print help\n",
        ),
        (
            &["build", "--help"],
            "Analyze WebTest files and emit a versioned execution plan\n\nUsage: webtest build --emit <EMIT> [PATHS]...\n\nArguments:\n  [PATHS]...  \n\nOptions:\n      --emit <EMIT>  Destination for the serialized plan envelope\n  -h, --help         Print help\n",
        ),
        (
            &["test", "--help"],
            "Execute WebTest files in Chrome\n\nUsage: webtest test [OPTIONS] [PATHS]...\n\nArguments:\n  [PATHS]...  \n\nOptions:\n      --chrome-path <CHROME_PATH>  \n      --headed                     Show the Chrome window while tests run\n      --reporter <REPORTER>        [default: human] [possible values: human, concise, json, junit, events]\n  -h, --help                       Print help\n",
        ),
        (
            &["inspect", "--help"],
            "Inspect the semantic interaction and assertion surface of one page\n\nUsage: webtest inspect [OPTIONS] [URL]\n\nArguments:\n  [URL]  \n\nOptions:\n      --chrome-path <CHROME_PATH>  \n      --headed                     Show the Chrome window while inspecting\n      --reporter <REPORTER>        [default: human] [possible values: human, json]\n  -h, --help                       Print help\n",
        ),
        (
            &["describe", "--help"],
            "Describe the installed language and project-visible provider surface\n\nUsage: webtest describe [OPTIONS] [QUERY]\n\nArguments:\n  [QUERY]  \n\nOptions:\n      --search <SEARCH>      \n      --project <PROJECT>    \n      --reporter <REPORTER>  [default: human] [possible values: human, json]\n  -h, --help                 Print help\n",
        ),
        (
            &["browser", "--help"],
            "Install and inspect managed Chrome for Testing versions\n\nUsage: webtest browser <COMMAND>\n\nCommands:\n  install  Download and atomically install the tested Chrome for Testing version\n  list     List valid managed installations\n  path     Print the Chrome executable selected by normal resolution precedence\n  clean    Remove validated WebTest-owned managed installations\n  help     Print this message or the help of the given subcommand(s)\n\nOptions:\n  -h, --help  Print help\n",
        ),
        (
            &["lsp", "--help"],
            "Run the language server over stdio\n\nUsage: webtest lsp [OPTIONS]\n\nOptions:\n      --chrome-path <CHROME_PATH>  \n  -h, --help                       Print help\n",
        ),
        (
            &["dap", "--help"],
            "Run the debug adapter protocol server over stdio\n\nUsage: webtest dap [OPTIONS]\n\nOptions:\n      --chrome-path <CHROME_PATH>  \n      --project <PROJECT>          File or directory used to discover the nearest WebTest project\n      --headless                   Hide Chrome while debugging. Debug sessions are headed by default\n  -h, --help                       Print help\n",
        ),
        (
            &["browser", "install", "--help"],
            "Download and atomically install the tested Chrome for Testing version\n\nUsage: webtest browser install\n\nOptions:\n      --version <VERSION>  \n  -h, --help               Print help\n",
        ),
        (
            &["browser", "list", "--help"],
            "List valid managed installations\n\nUsage: webtest browser list\n\nOptions:\n  -h, --help  Print help\n",
        ),
        (
            &["browser", "path", "--help"],
            "Print the Chrome executable selected by normal resolution precedence\n\nUsage: webtest browser path\n\nOptions:\n  -h, --help  Print help\n",
        ),
        (
            &["browser", "clean", "--help"],
            "Remove validated WebTest-owned managed installations\n\nUsage: webtest browser clean\n\nOptions:\n      --version <VERSION>  \n  -h, --help               Print help\n",
        ),
    ];

    for (arguments, expected) in cases {
        let output = run(Path::new(env!("CARGO_MANIFEST_DIR")), arguments);
        assert_eq!(output.status.code(), Some(0), "{arguments:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}");
        assert_eq!(String::from_utf8(output.stdout).expect("UTF-8"), *expected);
    }
}

#[test]
fn clap_errors_use_stderr_and_exit_two() {
    let output = run(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        &["describe", "locator.role", "--search", "role"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn check_reporters_preserve_output_and_exit_class() {
    let directory = fixture(
        "test \"works\" {\n    server {\n        let value = 1\n        expect value == 1\n    }\n}\n",
    );
    let human = run(directory.path(), &["check"]);
    assert_eq!(human.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(human.stdout).expect("human output"),
        "checked 1 file; 0 diagnostics\n"
    );
    assert!(human.stderr.is_empty());

    let concise = run(directory.path(), &["check", "--reporter", "concise"]);
    assert_eq!(concise.status.code(), Some(0));
    let concise = String::from_utf8(concise.stdout).expect("concise output");
    assert!(concise.contains("files=1 diagnostics=0 tests=0"));
    assert!(concise.ends_with("infrastructure=0 internal=0 exit=0\n"));

    let json = run(directory.path(), &["check", "--reporter", "json"]);
    assert_eq!(json.status.code(), Some(0));
    assert!(json.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).expect("JSON report");
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["command"], "check");
    assert_eq!(json["exit_class"], "success");
    assert_eq!(json["files"][0]["path"], "tests/example.webtest");
    assert_eq!(json["files"][0]["diagnostics"], serde_json::json!([]));
}

#[test]
fn human_test_reporter_streams_preparation_and_each_test_status_once() {
    let directory = fixture(
        "test \"works\" {\n    server {\n        let value = 1\n        expect value == 1\n    }\n}\n",
    );
    let output = run(directory.path(), &["test"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("human output"),
        concat!(
            "checking 1 test file ... done\n",
            "running 1 test in 1 file\n",
            "test \"works\" ... ok\n",
            "1 passed; 0 failed; 0 timed out; 0 cancelled; 0 skipped; 0 aborted; 0 infrastructure errors; 0 internal errors\n",
        )
    );
}

#[test]
fn human_test_reporter_closes_browser_startup_when_resolution_fails() {
    let directory = fixture("test \"browser\" { browser { open \"about:blank\" } }\n");
    let output = run(
        directory.path(),
        &["test", "--chrome-path", "missing-chrome"],
    );
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("human output");
    assert!(stdout.contains("starting Chrome for tests/example.webtest (headless) ... FAILED\n"));
    assert!(stdout.contains("infrastructure error[runtime.browser_launch]"));
}

#[test]
fn static_errors_suppress_test_execution_without_resolving_chrome() {
    let directory =
        fixture("test \"broken\" {\n    server {\n        expect missing == 1\n    }\n}\n");
    let output = run(directory.path(), &["test", "--reporter", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["exit_class"], "test_failure");
    assert_eq!(report["files"][0]["tests"], serde_json::json!([]));
    assert!(
        report["files"][0]["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| !diagnostics.is_empty())
    );
}

#[test]
fn formatter_check_is_non_mutating_and_silent_when_clean() {
    let source = "test \"works\"{server{let value=1\nexpect value==1}}\n";
    let directory = fixture(source);
    let path = directory.path().join("tests/example.webtest");

    let check = run(directory.path(), &["fmt", "--check"]);
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&path).expect("source"), source);
    assert!(String::from_utf8_lossy(&check.stdout).contains("would be reformatted"));

    let format = run(directory.path(), &["fmt"]);
    assert_eq!(format.status.code(), Some(0));
    assert_ne!(fs::read_to_string(&path).expect("source"), source);

    let clean = run(directory.path(), &["fmt", "--check"]);
    assert_eq!(clean.status.code(), Some(0));
    assert!(clean.stdout.is_empty());
    assert!(clean.stderr.is_empty());
}

#[test]
fn build_is_deterministic_and_description_json_remains_machine_clean() {
    let directory = fixture(
        "test \"works\" {\n    server {\n        let value = 1\n        expect value == 1\n    }\n}\n",
    );
    let first = directory.path().join("first.json");
    let second = directory.path().join("second.json");
    for emit in [&first, &second] {
        let output = Command::new(env!("CARGO_BIN_EXE_webtest"))
            .current_dir(directory.path())
            .args(["build", "--emit"])
            .arg(emit)
            .output()
            .expect("build plan");
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
    }
    assert_eq!(
        fs::read(&first).expect("first"),
        fs::read(&second).expect("second")
    );
    let plan: serde_json::Value =
        serde_json::from_slice(&fs::read(first).expect("plan")).expect("plan JSON");
    assert_eq!(plan["format_version"], 1);
    assert_eq!(plan["tests"][0]["id"], 0);
    assert_eq!(plan["tests"][0]["steps"][0]["id"], 0);

    for arguments in [
        &["describe", "--reporter", "json"][..],
        &["describe", "locator.role", "--reporter", "json"][..],
        &["describe", "--search", "role", "--reporter", "json"][..],
    ] {
        let output = run(directory.path(), arguments);
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("description JSON");
        assert!(value.is_object());
    }
}
