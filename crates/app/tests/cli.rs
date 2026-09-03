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
fn describe_bootstraps_exact_alias_category_and_search_without_source_files() {
    let directory = tempfile::tempdir().expect("temp directory");
    let index = webtest(directory.path())
        .args(["describe", "--reporter", "json"])
        .output()
        .expect("describe index");
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stderr)
    );
    let index: serde_json::Value = serde_json::from_slice(&index.stdout).expect("index JSON");
    assert_eq!(index["kind"], "description_index");
    assert_eq!(index["description_schema_version"], 1);
    assert!(
        index["categories"]["locators"]
            .as_array()
            .expect("locators")
            .contains(&serde_json::json!("locator.role"))
    );
    assert_eq!(
        index["categories"]["app_bridge"],
        serde_json::json!([
            "provider.app",
            "app.configuration",
            "app.bridge",
            "app.bridge.example",
            "app.schema",
            "app.protocol",
            "app.diagnostics",
            "runtime.configuration"
        ])
    );
    assert!(index["categories"].get("cli_commands").is_none());
    assert!(index["categories"].get("configuration").is_none());

    let role = webtest(directory.path())
        .args(["describe", "role", "--reporter", "json"])
        .output()
        .expect("describe role");
    assert!(role.status.success());
    let role: serde_json::Value = serde_json::from_slice(&role.stdout).expect("role JSON");
    assert_eq!(role["id"], "locator.role");
    assert_eq!(role["syntax_forms"][0]["elements"][1]["parameter"], "role");
    assert_eq!(role["allowed_contexts"][0], "scope.browser");
    assert_eq!(role["examples"].as_array().expect("examples").len(), 2);

    let short_provider_operation = webtest(directory.path())
        .args(["describe", "http.post", "--reporter", "json"])
        .output()
        .expect("describe short provider operation");
    assert!(short_provider_operation.status.success());
    let short_provider_operation: serde_json::Value =
        serde_json::from_slice(&short_provider_operation.stdout).expect("provider operation JSON");
    assert_eq!(short_provider_operation["id"], "provider.http.post");

    let category = webtest(directory.path())
        .args(["describe", "provider.http", "--reporter", "json"])
        .output()
        .expect("provider category");
    let category: serde_json::Value =
        serde_json::from_slice(&category.stdout).expect("category JSON");
    assert!(
        category["children"]
            .as_array()
            .expect("children")
            .contains(&serde_json::json!("provider.http.get"))
    );

    let plural_category = webtest(directory.path())
        .args(["describe", "locators", "--reporter", "json"])
        .output()
        .expect("plural category alias");
    assert!(plural_category.status.success());
    let plural_category: serde_json::Value =
        serde_json::from_slice(&plural_category.stdout).expect("plural category JSON");
    assert_eq!(plural_category["id"], "locator");

    let app_schema = webtest(directory.path())
        .args(["describe", "app-schema.json", "--reporter", "json"])
        .output()
        .expect("app manifest alias");
    assert!(app_schema.status.success());
    let app_schema: serde_json::Value =
        serde_json::from_slice(&app_schema.stdout).expect("app schema JSON");
    assert_eq!(app_schema["id"], "app.schema");

    let search = webtest(directory.path())
        .args([
            "describe",
            "--search",
            "activate button pointer",
            "--reporter",
            "json",
        ])
        .output()
        .expect("description search");
    let search: serde_json::Value = serde_json::from_slice(&search.stdout).expect("search JSON");
    assert_eq!(search["results"][0]["id"], "browser.click");

    let lifecycle_search = webtest(directory.path())
        .args([
            "describe",
            "--search",
            "inspect start application lifecycle",
            "--reporter",
            "json",
        ])
        .output()
        .expect("application lifecycle search");
    let lifecycle_search: serde_json::Value =
        serde_json::from_slice(&lifecycle_search.stdout).expect("lifecycle search JSON");
    assert_eq!(lifecycle_search["results"][0]["id"], "app.configuration");

    let unknown = webtest(directory.path())
        .args(["describe", "locator.rol", "--reporter", "json"])
        .output()
        .expect("unknown description");
    assert_eq!(unknown.status.code(), Some(2));
    let unknown: serde_json::Value = serde_json::from_slice(&unknown.stdout).expect("unknown JSON");
    assert_eq!(unknown["code"], "description_unknown_query");
    assert_eq!(unknown["repair_hints"][0]["kind"], "name_candidate");

    let human_unknown = webtest(directory.path())
        .args(["describe", "locator.rol"])
        .output()
        .expect("human unknown description");
    assert_eq!(human_unknown.status.code(), Some(2));
    let human_unknown = String::from_utf8(human_unknown.stdout).expect("human output");
    assert!(human_unknown.contains("suggestion: locator.role"));
    assert!(human_unknown.contains("reference: locator.role"));

    write(
        &directory.path().join("webtest.toml"),
        "[app]\ncommand = \"node\"\nargs = [\"server.js\", \"--token\", \"do-not-report\"]\nworking_directory = \".\"\n\n[server.app]\nadapter = \"bridge\"\ntransport = \"tcp\"\nschema = \"app-schema.json\"\n\n[browser]\nbase_url = \"http://127.0.0.1:3000\"\n\n[server]\nbase_url = \"http://127.0.0.1:3001\"\n",
    );
    write(
        &directory.path().join("app-schema.json"),
        include_str!("../../../protocol/examples/app-schema.json"),
    );
    let project_operation = webtest(directory.path())
        .args(["describe", "app.create_user", "--reporter", "json"])
        .output()
        .expect("project app operation");
    assert!(
        project_operation.status.success(),
        "{}",
        String::from_utf8_lossy(&project_operation.stderr)
    );
    let project_operation: serde_json::Value =
        serde_json::from_slice(&project_operation.stdout).expect("project operation JSON");
    assert_eq!(project_operation["id"], "provider.app.create_user");
    assert_eq!(project_operation["examples"], serde_json::json!([]));
    assert!(
        project_operation["guidance"]
            .as_array()
            .expect("guidance")
            .iter()
            .any(|guidance| guidance["code"] == "project_examples_not_declared")
    );

    let runtime_configuration = webtest(directory.path())
        .args(["describe", "runtime.configuration", "--reporter", "json"])
        .output()
        .expect("resolved runtime configuration");
    assert!(runtime_configuration.status.success());
    let runtime_configuration: serde_json::Value =
        serde_json::from_slice(&runtime_configuration.stdout).expect("runtime configuration JSON");
    let resolved = &runtime_configuration["resolved_configuration"];
    assert_eq!(resolved["selected_adapter"], "bridge");
    assert_eq!(resolved["selected_transport"], "tcp");
    assert_eq!(resolved["resolved_command"], "node");
    assert_eq!(
        resolved["resolved_arguments"],
        serde_json::json!(["server.js", "--token", "<redacted>"])
    );
    assert_eq!(resolved["browser_base_url"], "http://127.0.0.1:3000");
    assert_eq!(resolved["server_base_url"], "http://127.0.0.1:3001");
    assert_eq!(resolved["application_owned"], true);
    assert_eq!(resolved["application_health_configured"], false);
    assert!(!runtime_configuration.to_string().contains("do-not-report"));
}

#[test]
fn init_creates_a_checkable_idempotent_application_bridge_scaffold() {
    let directory = tempfile::tempdir().expect("temp directory");
    let project = directory.path().join("demo");
    let initialized = webtest(directory.path())
        .args(["init", "demo"])
        .output()
        .expect("initialize project");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let initialized_output = String::from_utf8(initialized.stdout).expect("init output");
    assert!(initialized_output.contains("created webtest.toml"));
    assert!(initialized_output.contains(".agents/skills/webtest/SKILL.md"));
    assert!(initialized_output.contains("webtest describe app.protocol"));

    assert!(project.join("webtest.toml").is_file());
    assert!(project.join(".webtest/app-schema.json").is_file());
    assert!(project.join("tests/example.webtest").is_file());
    let skill_path = project.join(".agents/skills/webtest/SKILL.md");
    let skill = fs::read_to_string(&skill_path).expect("installed skill");
    assert_eq!(
        skill,
        include_str!("../../../.agents/skills/webtest/SKILL.md")
    );
    assert!(skill.contains("webtest init ."));
    assert!(skill.contains("test \"application bridge responds\""));
    assert!(skill.contains("[server.app]"));
    assert!(skill.contains("webtest describe app.schema"));
    assert!(skill.contains("webtest describe app.configuration"));
    assert!(skill.contains("webtest describe runtime.configuration"));
    assert!(skill.contains("webtest describe app.bridge"));
    assert!(skill.contains("webtest describe app.protocol"));
    assert!(skill.contains("webtest describe app.bridge.example"));
    assert!(skill.contains("webtest describe app.diagnostics"));
    assert!(skill.contains("webtest describe app.echo"));
    assert!(skill.contains("Do not start the configured application separately"));
    assert!(skill.contains("webtest inspect [<url>] --reporter json"));
    assert!(!skill.contains("target/debug/webtest"));
    assert!(!skill.contains("cargo run"));

    let manifest = webtest_app_bridge::AppManifest::read(&project.join(".webtest/app-schema.json"))
        .expect("valid generated manifest");
    assert!(manifest.functions.contains_key("echo"));
    assert_eq!(manifest.sdk, "webtest-init");

    #[cfg(unix)]
    assert_eq!(
        fs::read_link(project.join(".claude/skills/webtest")).expect("Claude skill link"),
        Path::new("../../.agents/skills/webtest")
    );
    #[cfg(not(unix))]
    assert!(project.join(".claude/skills/webtest/SKILL.md").is_file());

    let check = webtest(&project)
        .args(["check", "--reporter", "json"])
        .output()
        .expect("check generated project");
    assert!(
        check.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let formatted = webtest(&project)
        .args(["fmt", "--check"])
        .output()
        .expect("check generated formatting");
    assert!(
        formatted.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&formatted.stdout),
        String::from_utf8_lossy(&formatted.stderr)
    );

    let described = webtest(&project)
        .args(["describe", "app.echo", "--reporter", "json"])
        .output()
        .expect("describe generated operation");
    assert!(
        described.status.success(),
        "{}",
        String::from_utf8_lossy(&described.stderr)
    );
    let described: serde_json::Value =
        serde_json::from_slice(&described.stdout).expect("description JSON");
    assert_eq!(described["id"], "provider.app.echo");

    let repeated = webtest(&project)
        .args(["init"])
        .output()
        .expect("repeat initialization");
    assert!(repeated.status.success());
    let repeated = String::from_utf8(repeated.stdout).expect("repeated output");
    assert!(repeated.contains("already initialized"));
    assert!(repeated.contains("unchanged webtest.toml"));
}

#[test]
fn check_and_describe_accept_a_directly_edited_manifest_with_a_stale_hash() {
    let directory = tempfile::tempdir().expect("temp directory");
    let project = directory.path().join("demo");
    let initialized = webtest(directory.path())
        .args(["init", "demo"])
        .output()
        .expect("initialize project");
    assert!(initialized.status.success());

    let schema_path = project.join(".webtest/app-schema.json");
    let mut manifest =
        webtest_app_bridge::AppManifest::read(&schema_path).expect("generated manifest is strict");
    let echo = manifest.functions.remove("echo").expect("echo function");
    manifest.functions.insert("renamed_echo".into(), echo);
    fs::write(
        &schema_path,
        serde_json::to_vec_pretty(&manifest).expect("edited manifest JSON"),
    )
    .expect("write directly edited manifest");
    assert!(webtest_app_bridge::AppManifest::read(&schema_path).is_err());
    write(
        &project.join("tests/example.webtest"),
        "test \"application bridge responds\" { server { let echoed = app.renamed_echo(message: \"hello\") expect echoed == \"hello\" } }\n",
    );

    let check = webtest(&project)
        .args(["check", "--reporter", "json"])
        .output()
        .expect("check edited manifest");
    assert!(
        check.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let described = webtest(&project)
        .args(["describe", "app.renamed_echo", "--reporter", "json"])
        .output()
        .expect("describe directly edited operation");
    assert!(
        described.status.success(),
        "{}",
        String::from_utf8_lossy(&described.stderr)
    );
    let described: serde_json::Value =
        serde_json::from_slice(&described.stdout).expect("description JSON");
    assert_eq!(described["id"], "provider.app.renamed_echo");
}

#[test]
fn init_refuses_conflicts_without_creating_partial_scaffolding() {
    let directory = tempfile::tempdir().expect("temp directory");
    let configuration = directory.path().join("webtest.toml");
    write(&configuration, "# application-owned configuration\n");

    let initialized = webtest(directory.path())
        .args(["init"])
        .output()
        .expect("initialize conflicting project");
    assert_eq!(initialized.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&initialized.stderr).contains("webtest.toml"));
    assert_eq!(
        fs::read_to_string(configuration).expect("preserved configuration"),
        "# application-owned configuration\n"
    );
    assert!(!directory.path().join(".webtest/app-schema.json").exists());
    assert!(!directory.path().join("tests/example.webtest").exists());
    assert!(
        !directory
            .path()
            .join(".agents/skills/webtest/SKILL.md")
            .exists()
    );
    assert!(fs::symlink_metadata(directory.path().join(".claude/skills/webtest")).is_err());
}

#[test]
fn init_preflights_agent_directory_conflicts_before_writing_project_files() {
    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join(".claude"),
        "application-owned file\n",
    );

    let initialized = webtest(directory.path())
        .args(["init"])
        .output()
        .expect("initialize with conflicting agent directory");
    assert_eq!(initialized.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&initialized.stderr).contains(".claude/skills/webtest"));
    assert!(!directory.path().join("webtest.toml").exists());
    assert!(!directory.path().join(".webtest/app-schema.json").exists());
    assert!(!directory.path().join("tests/example.webtest").exists());
}

#[test]
fn check_json_contains_versioned_source_identity_semantic_details_and_repairs() {
    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("machine.webtest"),
        r#"test "machine" {
    server {
        let user: { email: String } = { email: "alice@example.test" }
        let typo = user.emial
    }
}
"#,
    );
    let output = webtest(directory.path())
        .args(["check", "machine.webtest", "--reporter", "json"])
        .output()
        .expect("machine check");
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("check JSON");
    let diagnostic = report["files"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "semantic.unknown_member")
        .expect("unknown member");
    assert_eq!(diagnostic["diagnostic_schema_version"], 1);
    assert_eq!(diagnostic["repair_hint_schema_version"], 1);
    assert_eq!(diagnostic["semantic_details"]["requested"], "emial");
    assert_eq!(diagnostic["repair_hints"][0]["replacement"], "email");
    assert_eq!(diagnostic["source"]["path"], "machine.webtest");
    assert_eq!(
        diagnostic["source"]["source_revision"],
        report["files"][0]["source_revision"]
    );
    assert!(
        diagnostic["source"]["byte_range"]["end"].as_u64().unwrap()
            > diagnostic["source"]["byte_range"]["start"]
                .as_u64()
                .unwrap()
    );
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
    assert_eq!(report["schema_version"], 3);
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
        "test \"valid\" { browser { open \"about:blank\" } }\n",
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
    assert!(report["files"][0]["execution_error"].is_null());
    assert!(
        report["files"][0]["tests"][0]["failure"]["message"]
            .as_str()
            .expect("message")
            .contains("does not exist")
    );
    assert_eq!(
        report["files"][0]["tests"][0]["failure_class"],
        "infrastructure"
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
fn build_emits_a_versioned_typed_plan_deterministically() {
    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("typed.webtest"),
        r#"test "typed" {
    server {
        let response = http.get("http://example.test/user")
        expect response.status == 200
    }
}
"#,
    );
    let first = directory.path().join("first.json");
    let second = directory.path().join("second.json");
    for output in [&first, &second] {
        let result = webtest(directory.path())
            .args(["build", "typed.webtest", "--emit"])
            .arg(output)
            .output()
            .expect("build plan");
        assert_eq!(
            result.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let first_bytes = fs::read(&first).expect("first plan");
    assert_eq!(first_bytes, fs::read(&second).expect("second plan"));
    let plan: serde_json::Value = serde_json::from_slice(&first_bytes).expect("plan JSON");
    assert_eq!(plan["format_version"], 3);
    assert_eq!(plan["required_host_capabilities"][0], "server");
    assert_eq!(
        plan["tests"][0]["required_host_capabilities"],
        serde_json::json!(["server", "test"])
    );
    assert_eq!(
        plan["tests"][0]["steps"][0]["operation"]["kind"],
        "server_provider_call"
    );
    assert_eq!(
        plan["tests"][0]["steps"][1]["operation"]["operation"]["actual"]["value"]["missing_is_null"],
        false
    );

    write(
        &directory.path().join("webtest.toml"),
        "[server]\nbase_url = \"http://127.0.0.1:4000\"\n",
    );
    let configured = directory.path().join("configured.json");
    assert!(
        webtest(directory.path())
            .args(["build", "typed.webtest", "--emit"])
            .arg(&configured)
            .status()
            .expect("configured build")
            .success()
    );
    let configured: serde_json::Value =
        serde_json::from_slice(&fs::read(configured).expect("configured plan"))
            .expect("configured plan JSON");
    assert_ne!(plan["project_identity"], configured["project_identity"]);
}

#[test]
fn build_refuses_to_serialize_literal_secrets() {
    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("secret.webtest"),
        r#"test "secret" {
    server {
        let credential = "do-not-emit"
        http.post("http://example.test/login", json: { password: credential })
    }
}
"#,
    );
    let output_path = directory.path().join("plan.json");
    let result = webtest(directory.path())
        .args(["build", "secret.webtest", "--emit"])
        .arg(&output_path)
        .output()
        .expect("secret build");
    assert_eq!(result.status.code(), Some(2));
    assert!(!output_path.exists());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("literal secret"), "{stderr}");
    assert!(!stderr.contains("do-not-emit"), "{stderr}");
}

#[test]
fn server_only_http_decode_and_assertion_runs_without_chrome() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let fixture = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("fixture request");
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let body = r#"{"id":7,"email":"alice@example.test"}"#;
        write!(
            stream,
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("fixture response");
    });

    let directory = tempfile::tempdir().expect("temp directory");
    write(
        &directory.path().join("server.webtest"),
        &format!(
            r#"test "server" {{
    server {{
        let response = http.post("http://{address}/users", json: {{ email: "alice@example.test" }})
        expect response.status == 201
        let user: {{ id: Int, email: String }} = response.json
        expect user.id == 7
    }}
}}
"#,
        ),
    );
    let result = webtest(directory.path())
        .args(["test", "server.webtest", "--reporter", "json"])
        .env(
            "WEBTEST_CHROME_PATH",
            directory.path().join("missing-chrome"),
        )
        .output()
        .expect("server-only run");
    fixture.join().expect("fixture thread");
    assert_eq!(
        result.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).expect("run report");
    assert_eq!(report["summary"]["passed"], 1);
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
        "test \"valid\" { browser { open \"about:blank\" } }\n",
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
