use std::{fs, path::Path, process::Command};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(path, contents).expect("write fixture");
}

fn webtest(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_webtest"));
    command.current_dir(directory);
    command.env_remove("WEBTEST_CHROME_PATH");
    command.env("WEBTEST_CACHE_DIR", directory.join("cache"));
    command
}

#[test]
fn check_without_paths_discovers_configured_tests_in_order() {
    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("webtest.toml"),
        "[project]\ntest_roots = [\"specs\"]\nexclude = [\"specs/generated/**\"]\n",
    );
    write(
        &directory.path().join("specs/β.webtest"),
        "test \"beta\" {}\n",
    );
    write(&directory.path().join("specs/a.webtest"), "test \"a\" {}\n");
    write(
        &directory.path().join("specs/generated/skip.webtest"),
        "test \"skip\" {}\n",
    );

    let output = webtest(directory.path())
        .args(["check", "--reporter", "json"])
        .output()
        .expect("run check");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["schema_version"], 1);
    let paths = report["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("path"))
        .collect::<Vec<_>>();
    assert_eq!(paths, ["specs/a.webtest", "specs/β.webtest"]);
}

#[test]
fn static_usage_and_browser_failures_have_distinct_exit_codes() {
    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("broken.webtest"),
        "test \"broken\" {\n",
    );
    let static_failure = webtest(directory.path())
        .args(["test", "broken.webtest", "--reporter", "json"])
        .output()
        .expect("run static failure");
    assert_eq!(static_failure.status.code(), Some(1));
    let report: serde_json::Value =
        serde_json::from_slice(&static_failure.stdout).expect("static JSON");
    assert_eq!(report["exit_class"], "test_failure");

    let usage = webtest(directory.path())
        .args(["check", "missing.webtest"])
        .output()
        .expect("run missing input");
    assert_eq!(usage.status.code(), Some(2));

    let unsupported_browser = webtest(directory.path())
        .args(["browser", "install", "--version", "0.0.0.0"])
        .output()
        .expect("unsupported browser version");
    assert_eq!(unsupported_browser.status.code(), Some(2));

    write(
        &directory.path().join("valid.webtest"),
        "test \"valid\" {}\n",
    );
    let infrastructure = webtest(directory.path())
        .args([
            "test",
            "valid.webtest",
            "--chrome-path",
            "missing-chrome",
            "--reporter",
            "json",
        ])
        .output()
        .expect("run browser failure");
    assert_eq!(infrastructure.status.code(), Some(3));
    let report: serde_json::Value =
        serde_json::from_slice(&infrastructure.stdout).expect("infrastructure JSON");
    assert_eq!(report["exit_class"], "infrastructure");
    assert!(
        report["files"][0]["infrastructure_error"]["message"]
            .as_str()
            .expect("message")
            .contains("does not exist")
    );
}

#[test]
fn fmt_check_is_non_mutating_and_rewrite_converges() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("format.webtest");
    write(
        &path,
        "test \"format\"{browser{open \"http://example.com\"}}\n",
    );
    let original = fs::read_to_string(&path).expect("original");

    let check = webtest(directory.path())
        .args(["fmt", "format.webtest", "--check"])
        .output()
        .expect("format check");
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&path).expect("unchanged"), original);

    assert!(
        webtest(directory.path())
            .args(["fmt", "format.webtest"])
            .status()
            .expect("format")
            .success()
    );
    let formatted = fs::read_to_string(&path).expect("formatted");
    assert_ne!(formatted, original);
    assert!(
        webtest(directory.path())
            .args(["fmt", "format.webtest", "--check"])
            .status()
            .expect("second check")
            .success()
    );
}

#[test]
fn browser_path_reports_environment_provenance() {
    let directory = tempfile::tempdir().expect("temp directory");
    let chrome = directory.path().join("chrome fixture");
    write(&chrome, "fixture");
    let output = webtest(directory.path())
        .env("WEBTEST_CHROME_PATH", &chrome)
        .args(["browser", "path"])
        .output()
        .expect("browser path");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        fs::canonicalize(&chrome)
            .expect("canonical Chrome fixture")
            .display()
            .to_string()
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("WEBTEST_CHROME_PATH"));
}

#[test]
fn browser_environment_path_overrides_project_configuration() {
    let directory = tempfile::tempdir().expect("temp directory");
    let configured = directory.path().join("configured-chrome");
    let environment = directory.path().join("environment-chrome");
    write(&configured, "configured");
    write(&environment, "environment");
    write(
        &directory.path().join("webtest.toml"),
        "[browser]\npath = \"configured-chrome\"\n",
    );

    let configured_output = webtest(directory.path())
        .args(["browser", "path"])
        .output()
        .expect("configured path");
    assert!(configured_output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&configured_output.stdout).trim(),
        fs::canonicalize(&configured)
            .expect("canonical configured path")
            .display()
            .to_string()
    );

    let environment_output = webtest(directory.path())
        .env("WEBTEST_CHROME_PATH", &environment)
        .args(["browser", "path"])
        .output()
        .expect("environment path");
    assert!(environment_output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&environment_output.stdout).trim(),
        fs::canonicalize(&environment)
            .expect("canonical environment path")
            .display()
            .to_string()
    );
}

#[cfg(unix)]
#[test]
fn mixed_outcomes_use_the_highest_severity_exit_class() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("broken.webtest"),
        "test \"broken\" {\n",
    );
    write(
        &directory.path().join("valid.webtest"),
        "test \"valid\" {}\n",
    );
    let fake_chrome = directory.path().join("fake-chrome");
    write(&fake_chrome, "#!/bin/sh\nexit 9\n");
    let mut permissions = fs::metadata(&fake_chrome)
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_chrome, permissions).expect("make fake executable");

    let output = webtest(directory.path())
        .arg("test")
        .arg("broken.webtest")
        .arg("valid.webtest")
        .arg("--chrome-path")
        .arg(&fake_chrome)
        .args(["--reporter", "json"])
        .output()
        .expect("run mixed outcomes");
    assert_eq!(output.status.code(), Some(3));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("mixed JSON");
    assert_eq!(report["exit_class"], "infrastructure");
    assert_eq!(report["files"][0]["exit_class"], "test_failure");
    assert_eq!(report["files"][1]["exit_class"], "infrastructure");
}
