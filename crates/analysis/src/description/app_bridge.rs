use super::catalog::{base_construct, constraint, example, parameter, provider_failures};
use super::*;

pub(super) fn constructs() -> Vec<ConstructDescription> {
    vec![
        provider_overview(),
        configuration_reference(),
        bridge_lifecycle_reference(),
        bridge_example_reference(),
        manifest_reference(),
        protocol_reference(),
        diagnostics_reference(),
        runtime_configuration_reference(),
    ]
}

fn configuration_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.configuration",
        "Application and provider configuration",
        "configuration_reference",
        "[server.app] plus adapter-specific [app] or [server.app.*] settings",
        "Configure runner-owned application lifecycle separately from the optional typed application-provider adapter.",
    );
    value.search_terms = vec![
        "webtest.toml app command".into(),
        "server.app command".into(),
        "bridge stdio http adapter".into(),
        "working directory".into(),
        "inspect start application app lifecycle".into(),
    ];
    value.constraints = vec![
        constraint(
            "app_configuration_ownership",
            "configuration",
            "sections",
            "`[server.app]` owns provider adapter, transport, schema, limits, and compatibility-adapter settings. `[app]` owns an optionally runner-managed application process, its arguments, working directory, environment, ownership, and health check.",
        ),
        constraint(
            "app_configuration_inspect_lifecycle",
            "inspection",
            "URL mode",
            "`webtest inspect` with an omitted or relative URL uses project mode: it starts `[app]` when configured, waits for its health check, inspects the resolved browser URL, and tears the application down. An explicit absolute HTTP(S) URL is standalone and does not launch `[app]`.",
        ),
        constraint(
            "app_configuration_no_duplicate_commands",
            "configuration",
            "command",
            "Socket bridges use `[app].command`; stdio bridges and command adapters use `[server.app].command`. Do not duplicate one process command in both locations unless the adapter intentionally starts two distinct processes.",
        ),
        constraint(
            "app_configuration_adapter_requirements",
            "configuration",
            "required settings",
            "Every adapter requires `server.app.schema`. A runner-managed socket bridge requires `[app].command`; a stdio bridge or command adapter requires `server.app.command`; an HTTP adapter requires `server.app.http.base_url` and at least one explicit operation.",
        ),
    ];
    value.guidance = vec![
        guidance(
            "app_configuration_process_consumers",
            "`webtest test` and project-mode `webtest inspect` both use `[app]`. A project with browser-only tests or inspection may configure `[app]` without `[server.app]`; add `[server.app]` only when server blocks call typed `app.*` operations.",
        ),
        guidance(
            "app_configuration_bridge_difference",
            "For `adapter = \"bridge\"` with `auto`, `unix`, `named_pipe`, or `tcp`, WebTest creates the local endpoint and launches or waits for the application described by `[app]`. With `transport = \"stdio\"`, WebTest launches `server.app.command` as the dedicated protocol peer and reserves its stdout for frames; `[app]`, when present, describes a separate web application process and health check.",
        ),
        guidance(
            "app_configuration_command_vs_stdio",
            "The `command` compatibility adapter starts `server.app.command` once per `app.*` call, writes one JSON object containing function, arguments, deadline_ms, and schema_hash to stdin, reads one bounded terminal JSON response from stdout, and exits. A stdio bridge is one persistent Protocol 1 process for the run: it performs hello/schema negotiation, handles many correlated calls, and participates in shutdown.",
        ),
        guidance(
            "app_configuration_timeouts",
            "Configure bridge connection and protocol readiness with `server.app.startup_timeout`, and bounded bridge shutdown and child cleanup with `server.app.shutdown_timeout`; neither key belongs under `[app]`. `[app.health].timeout` is separate and bounds only the configured HTTP health check.",
        ),
        guidance(
            "app_configuration_validation",
            "Unknown keys are warnings. Invalid adapter/transport combinations, missing adapter-required settings, absolute or parent-traversing project paths, and malformed URLs are configuration errors before analysis or execution.",
        ),
    ];
    value.examples = vec![
        example(
            "runner-managed socket bridge",
            "[app]\ncommand = \"node\"\nargs = [\"server.js\"]\nworking_directory = \".\"\n\n[server.app]\nadapter = \"bridge\"\ntransport = \"auto\"\nschema = \".webtest/app-schema.json\"\nstartup_timeout = \"10s\"\nshutdown_timeout = \"2s\"",
            "config",
            "webtest.toml",
        ),
        example(
            "dedicated stdio bridge",
            "[server.app]\nadapter = \"bridge\"\ntransport = \"stdio\"\nschema = \".webtest/app-schema.json\"\ncommand = \"node\"\nargs = [\"bridge.js\"]",
            "config",
            "webtest.toml",
        ),
        example(
            "command compatibility adapter",
            "[server.app]\nadapter = \"command\"\nschema = \".webtest/app-schema.json\"\ncommand = \"bin/app-provider\"\nargs = []",
            "config",
            "webtest.toml",
        ),
        example(
            "HTTP compatibility adapter",
            "[server.app]\nadapter = \"http\"\nschema = \".webtest/app-schema.json\"\n\n[server.app.http]\nbase_url = \"http://127.0.0.1:3000\"\n\n[server.app.http.operations]\ncreate_user = { method = \"POST\", path = \"/__webtest/create_user\" }",
            "config",
            "webtest.toml",
        ),
    ];
    value.related = vec![
        "app.bridge".into(),
        "app.schema".into(),
        "runtime.configuration".into(),
    ];
    value
}

fn bridge_lifecycle_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.bridge",
        "Application bridge lifecycle",
        "lifecycle_reference",
        "runner start → transport connect → hello → describe → calls → shutdown",
        "Follow the complete runner-owned lifecycle for a Protocol 1 application bridge.",
    );
    value.search_terms = vec![
        "bridge lifecycle".into(),
        "startup teardown".into(),
        "hello shutdown".into(),
    ];
    value.constraints = vec![
        constraint(
            "bridge_lifecycle_order",
            "runtime",
            "state machine",
            "The application sends `hello`; the runner sends `hello_ok` and then `describe`; the application returns a correlated `schema`; calls and shutdown each require a terminal response carrying the request's exact ID.",
        ),
        constraint(
            "bridge_stream_ownership",
            "transport",
            "stdout",
            "For stdio, stdin/stdout are the protocol stream and application logs must use stderr. Socket bridges may log normally, but must send frames only on the selected bridge connection.",
        ),
    ];
    value.guidance = vec![
        guidance(
            "bridge_lifecycle_sequence",
            "WebTest resolves configuration, creates the socket listener or stdio pipes, starts the configured process, and establishes transport. The application sends authenticated `hello`; WebTest verifies it, replies `hello_ok`, requests `describe`, verifies the returned schema, dispatches correlated calls, sends `shutdown`, waits for correlated `shutdown_ok`, then reaps runner-owned children.",
        ),
        guidance(
            "bridge_lifecycle_environment",
            "Socket bridges receive `WEBTEST_BRIDGE`, `WEBTEST_TOKEN`, and `WEBTEST_PROTOCOL=1`. Stdio bridges receive `WEBTEST_TOKEN` and `WEBTEST_PROTOCOL=1`; their endpoint is stdin/stdout, so `WEBTEST_BRIDGE` is absent.",
        ),
    ];
    value.examples = vec![
        example(
            "socket lifecycle",
            "create local listener\nspawn [app].command with WEBTEST_BRIDGE, WEBTEST_TOKEN, WEBTEST_PROTOCOL=1\naccept connection\nhello → hello_ok → describe → schema\ncall(id) → result(id) or error(id)\nshutdown(id) → shutdown_ok(id)\nreap child",
            "sequence",
            "runner_and_application",
        ),
        example(
            "stdio lifecycle",
            "spawn server.app.command with stdin/stdout pipes and WEBTEST_TOKEN, WEBTEST_PROTOCOL=1\nhello → hello_ok → describe → schema\ncall(id) → result(id) or error(id)\nshutdown(id) → shutdown_ok(id)\nreap bridge process",
            "sequence",
            "runner_and_application",
        ),
    ];
    value.related = vec![
        "app.configuration".into(),
        "app.protocol".into(),
        "app.bridge.example".into(),
        "app.diagnostics".into(),
    ];
    value.availability.runtime_requires = vec!["native_app_bridge".into()];
    value
}

fn bridge_example_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.bridge.example",
        "Custom application bridge example",
        "implementation_reference",
        "correlated Protocol 1 bridge loop pseudocode",
        "Implement the smallest custom bridge loop while preserving authentication, bounds, validation, and exact request-ID correlation.",
    );
    value.search_terms = vec![
        "bridge pseudocode".into(),
        "request id correlation".into(),
        "shutdown_ok id".into(),
    ];
    value.constraints = vec![constraint(
        "bridge_exact_response_id",
        "protocol",
        "response id",
        "Copy the ID from each `describe`, `call`, `ping`, and `shutdown` request into its corresponding `schema`, `result`/`error`, `pong`, or `shutdown_ok` response. Never hardcode or generate response IDs.",
    )];
    value.guidance = vec![guidance(
        "bridge_example_scope",
        "This is non-normative pseudocode. `app.protocol` remains authoritative for frame fields, limits, validation, cancellation, events, and state semantics.",
    )];
    value.examples = vec![
        example(
            "complete no-SDK loop",
            r#"manifest = load_manifest()
endpoint = environment.get("WEBTEST_BRIDGE")
if endpoint is absent:
    io = stdio  # only for configured transport = "stdio"
else if endpoint starts with "unix:":
    # WebTest emits exactly unix:/absolute/path/to/app.sock, not unix:///path.
    socket_path = endpoint after the "unix:" prefix
    require(socket_path starts with "/")
    io = connect_unix_socket(socket_path)
else if endpoint starts with "tcp://":
    # WebTest emits tcp://127.0.0.1:<random-port>.
    host, port = parse_tcp_uri(endpoint)
    require(host is loopback and port is an integer)
    io = connect_tcp(host, port)
else if endpoint starts with "pipe:":
    # On Windows, everything after "pipe:" is the named-pipe path.
    io = connect_named_pipe(endpoint after the "pipe:" prefix)
else:
    fail("unsupported WEBTEST_BRIDGE endpoint format")

send hello(token = WEBTEST_TOKEN, protocol_versions = [1])
hello_ok = receive_bounded_frame()
require(hello_ok.protocol == 1)

message = receive_bounded_frame()
require(message.type == describe)
send schema(id = message.id, protocol = 1,
            schema_hash = canonical_hash(manifest.functions),
            functions = manifest.functions)

loop:
    message = receive_bounded_frame()
    if message.type == call:
        validate(message.arguments, manifest.functions[message.function].params)
        outcome = invoke(message.function, message.arguments, message.deadline_ms)
        validate(outcome, manifest.functions[message.function].returns)
        send result(id = message.id, value = outcome)
        # on failure: send error(id = message.id, ...)
    if message.type == ping:
        send pong(id = message.id)
    if message.type == shutdown:
        send shutdown_ok(id = message.id)
        close io
        break"#,
            "pseudocode",
            "protocol_state_machine",
        ),
        example(
            "correlation rule",
            "receive call(id = request_id, ...)\nsend result(id = request_id, ...)\nreceive shutdown(id = shutdown_id)\nsend shutdown_ok(id = shutdown_id)",
            "pseudocode",
            "protocol_state_machine",
        ),
    ];
    value.related = vec![
        "app.bridge".into(),
        "app.protocol".into(),
        "app.schema".into(),
    ];
    value
}

fn provider_overview() -> ConstructDescription {
    let mut value = base_construct(
        "provider.app",
        "app",
        "provider",
        "app.<function>(<named arguments>?)",
        "Call a project-defined, statically typed server operation through the application provider.",
    );
    value.syntax_forms = vec![SyntaxForm {
        id: "call".into(),
        elements: vec![
            SyntaxElement::Literal {
                value: "app.".into(),
            },
            SyntaxElement::Slot {
                parameter: Some("function".into()),
                parameter_group: None,
            },
            SyntaxElement::Literal { value: "(".into() },
            SyntaxElement::Repeat {
                separator: ", ".into(),
                elements: vec![SyntaxElement::Slot {
                    parameter: None,
                    parameter_group: Some("named_arguments".into()),
                }],
            },
            SyntaxElement::Literal { value: ")".into() },
        ],
    }];
    value.parameters = vec![parameter(
        "function",
        Type::String,
        true,
        None,
        false,
        "operation_name",
        "Identifier",
    )];
    value.requires_capabilities = vec![Capability::Server];
    value.allowed_contexts = vec!["scope.server".into()];
    value.effects = vec!["provider_call".into(), "application_integration".into()];
    value.failure_modes = provider_failures("app");
    value.search_terms = vec![
        "application bridge".into(),
        "fixture".into(),
        "project provider".into(),
        "test data".into(),
    ];
    value.constraints = vec![
        constraint(
            "app_offline_schema",
            "analysis",
            "function and arguments",
            "Function names, parameters, defaults, secrets, retry safety, and result types come from the configured offline manifest.",
        ),
        constraint(
            "app_live_schema_identity",
            "runtime",
            "live schema",
            "A persistent bridge must describe functions whose derived canonical schema hash matches the offline schema compiled into the plan.",
        ),
    ];
    value.guidance = vec![
        guidance(
            "app_bridge_discovery",
            "For a new or custom bridge, inspect `app.configuration`, `runtime.configuration`, `app.bridge`, `app.schema`, `app.protocol`, `app.bridge.example`, and `app.diagnostics`; inspect the exact project operation separately after loading its manifest.",
        ),
        guidance(
            "app_operation_lookup",
            "With a project manifest loaded, query an exact operation such as `webtest describe app.create_user`; the canonical result ID is `provider.app.create_user`.",
        ),
        guidance(
            "app_server_context",
            "Use `app.*` only inside `server` blocks. Transferable results may be referenced by later browser blocks.",
        ),
    ];
    value.examples = vec![
        app_call_example(
            "typed application call",
            "let user = app.create_user(email: \"alice@example.com\")",
        ),
        app_call_example(
            "application result assertion",
            "let user = app.create_user(email: \"alice@example.com\", admin: true)\nexpect user.admin == true",
        ),
    ];
    value.related = vec![
        "app.configuration".into(),
        "app.bridge".into(),
        "app.schema".into(),
        "app.protocol".into(),
        "app.bridge.example".into(),
        "app.diagnostics".into(),
        "scope.server".into(),
    ];
    value.availability.runtime_requires = vec!["configured_app_provider".into()];
    value.availability.configuration_prerequisites = vec![
        "server.app.schema plus the selected bridge, command, or HTTP adapter configuration".into(),
    ];
    value
}

fn manifest_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.schema",
        "app schema",
        "schema",
        "Protocol 1 application manifest JSON configured by server.app.schema",
        "Define the offline application-provider operations used by analysis, editor services, plan compilation, and live schema verification.",
    );
    value.allowed_contexts = vec!["webtest.toml:server.app.schema".into()];
    value.search_terms = vec![
        "app-schema.json".into(),
        "offline manifest".into(),
        "schema hash".into(),
        "BLAKE3".into(),
        "function schema".into(),
    ];
    value.constraints = vec![
        constraint(
            "app_manifest_shape",
            "configuration",
            "manifest",
            "Protocol 1 requires manifest_version, protocol, provider, sdk, sdk_version, schema_hash, and a functions object.",
        ),
        constraint(
            "app_function_parameters",
            "configuration",
            "functions.<name>.params",
            "Function parameters must use an object schema; defaults are allowed only on optional parameters.",
        ),
        constraint(
            "app_schema_hash",
            "configuration",
            "schema_hash",
            "The schema identity is BLAKE3 over canonical JSON for functions; documentation and all semantic schema metadata participate.",
        ),
    ];
    value.guidance = vec![
        guidance(
            "app_manifest_types",
            "Manifest type tags are `null`, `boolean`, `integer`, `float`, `string`, `array`, `optional`, `object`, and `alias`. Arrays use `items`; optionals use `item`; objects use `fields`; aliases use `name` and `base`.",
        ),
        guidance(
            "app_manifest_hash_generation",
            "The offline schema may be edited directly. WebTest derives its in-memory schema identity from `functions`, so a stale declared `schema_hash` does not block check, editor analysis, planning, or execution, and WebTest does not rewrite the file. SDK exporters should still write the canonical hash for portable Protocol 1 manifests.",
        ),
        guidance(
            "app_manifest_location",
            "The conventional path is `.webtest/app-schema.json`, but `server.app.schema` may select another project-relative path.",
        ),
    ];
    value.examples = vec![
        example(
            "valid Protocol 1 manifest",
            include_str!("../../../../protocol/examples/app-schema.json"),
            "app_schema_json",
            "webtest.toml:server.app.schema",
        ),
        example(
            "runner-managed bridge configuration",
            "[app]\ncommand = \"node\"\nargs = [\"server.js\"]\n\n[server.app]\nadapter = \"bridge\"\ntransport = \"auto\"\nschema = \".webtest/app-schema.json\"",
            "config",
            "webtest.toml",
        ),
    ];
    value.related = vec!["provider.app".into(), "app.protocol".into()];
    value
}

fn protocol_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.protocol",
        "Application Bridge Protocol 1",
        "protocol",
        "one UTF-8 JSON object followed by LF per frame",
        "Authenticate a local application bridge, verify its live schema, dispatch correlated typed calls, and shut it down cleanly.",
    );
    value.search_terms = vec![
        "wire protocol".into(),
        "JSONL".into(),
        "hello".into(),
        "describe".into(),
        "schema drift".into(),
        "call deadline".into(),
        "loopback TCP".into(),
        "WEBTEST_BRIDGE".into(),
    ];
    value.constraints = vec![
        constraint(
            "protocol_framing",
            "transport",
            "frame",
            "Each bounded frame is one UTF-8 JSON object followed by LF. Protocol streams carry frames only; with stdio, stdout carries frames and logs use stderr.",
        ),
        constraint(
            "protocol_authentication",
            "handshake",
            "hello.token",
            "The bridge sends hello first and authenticates with the per-run WEBTEST_TOKEN before any other message is accepted.",
        ),
        constraint(
            "protocol_schema_verification",
            "handshake",
            "schema.schema_hash",
            "After hello_ok, the runner sends describe, derives the canonical hash from the returned functions, and rejects functions whose identity differs from the offline plan. The declared hash is not trusted and may be stale while a manifest is being edited.",
        ),
    ];
    value.guidance = vec![
        guidance(
            "protocol_sequence",
            "The normal sequence is `hello` → `hello_ok` → `describe` → `schema`, followed by correlated `call`/`result` or `error` frames, then `shutdown` → `shutdown_ok`.",
        ),
        guidance(
            "protocol_environment",
            "Runner-managed socket bridges receive WEBTEST_BRIDGE, WEBTEST_TOKEN, and WEBTEST_PROTOCOL=1. Persistent stdio bridges receive the token and protocol variables and use stdin/stdout; WEBTEST=1 is not injected unless the project adds it under app.environment.",
        ),
        guidance(
            "protocol_transport_selection",
            "The `webtest init` scaffold selects `transport = \"auto\"`: Unix-domain sockets on Unix, named pipes on Windows, and loopback TCP when local IPC is unavailable. A custom bridge may explicitly select `tcp` for a portable `tcp://127.0.0.1:<random-port>` endpoint. WebTest creates the listener before spawning the configured application command.",
        ),
        guidance(
            "protocol_managed_command",
            "Configure `[app].command` as one executable and `[app].args` as separate arguments. WebTest launches it directly without a shell, sets the bridge environment, and owns bounded shutdown and child reaping.",
        ),
        guidance(
            "protocol_transports",
            "The runner supports Unix sockets, Windows named pipes, loopback TCP, and persistent stdio. Custom bridges should prefer an SDK and must reject non-local socket endpoints.",
        ),
        guidance(
            "protocol_retry_facts",
            "Function retry_safe and error retryable are independent metadata; Protocol 1 and the sequential runner do not automatically retry a call.",
        ),
    ];
    value.examples = vec![
        example(
            "valid handshake frames",
            r#"{"type":"hello","protocol_versions":[1],"sdk":"custom","sdk_version":"0.1.0","token":"<per-run token>","capabilities":{"cancel":false,"events":false}}
{"type":"hello_ok","protocol":1,"run_id":"<run id>","max_message_bytes":1048576}
{"type":"describe","id":1}
{"type":"schema","id":1,"protocol":1,"schema_hash":"blake3:b1254e79ab8984797e49f26190f9fa181239cb0d4c0d279f4d627b7d101e1e2a","functions":{"create_user":{"documentation":"Create a user directly in the test application's in-memory store.","retry_safe":false,"params":{"type":"object","fields":{"admin":{"type":"boolean","documentation":"Grant administrative access.","optional":true,"secret":false,"default":false},"email":{"type":"string","documentation":"Unique sign-in email.","optional":false,"secret":false}}},"returns":{"type":"object","fields":{"admin":{"type":"boolean","optional":false,"secret":false},"email":{"type":"string","optional":false,"secret":false},"id":{"type":"integer","optional":false,"secret":false}}}}}}"#,
            "jsonl",
            "protocol_stream",
        ),
        example(
            "valid call terminal frames",
            r#"{"type":"call","id":42,"function":"create_user","arguments":{"email":"alice@example.com"},"deadline_ms":10000}
{"type":"result","id":42,"value":{"id":123,"email":"alice@example.com","admin":false}}
{"type":"call","id":43,"function":"create_user","arguments":{"email":"alice@example.com"},"deadline_ms":10000}
{"type":"error","id":43,"code":"user.exists","message":"user already exists","retryable":false,"data":{}}"#,
            "jsonl",
            "protocol_stream",
        ),
    ];
    value.related = vec![
        "provider.app".into(),
        "app.schema".into(),
        "app.bridge".into(),
        "app.bridge.example".into(),
    ];
    value.availability.runtime_requires = vec!["native_app_bridge".into()];
    value
}

fn diagnostics_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.diagnostics",
        "Application bridge diagnostics",
        "diagnostic_reference",
        "application bridge failure code → likely cause → next inspection",
        "Diagnose bridge startup, handshake, correlation, schema, and child-process failures using the structured provider failure and the process state it implies.",
    );
    value.search_terms = vec![
        "bridge did not connect".into(),
        "closed before hello".into(),
        "unknown response id".into(),
        "schema mismatch".into(),
        "child process exited".into(),
        "broken pipe teardown".into(),
        "timeout debugging".into(),
    ];
    value.failure_modes = provider_failures("app");
    value.guidance = vec![
        guidance(
            "diagnose_bridge_readiness_timeout",
            "`app_bridge_handshake` with `bridge_readiness_timeout` means the runner created a local endpoint but no bridge connected before `server.app.startup_timeout`. Confirm `runtime.configuration`, then inspect whether `[app].command` started, received `WEBTEST_BRIDGE`, and entered its explicit test boot path.",
        ),
        guidance(
            "diagnose_eof_before_hello",
            "`app_bridge_handshake` with `eof_before_hello` means transport connected and then closed before the first frame. Inspect child exit status and stderr; for stdio, remove every log or banner from stdout and send one JSON `hello` frame followed by LF.",
        ),
        guidance(
            "diagnose_unknown_response_id",
            "`app_bridge_protocol` with `unknown_response_id` means a terminal response reused, invented, or hardcoded an ID. Log request type and ID on stderr and copy each request ID unchanged into its one terminal response, including `shutdown_ok`.",
        ),
        guidance(
            "diagnose_schema_drift",
            "`app_schema_drift` means the canonical live `functions` differ from the offline manifest compiled into the plan. Diff the live schema response against `server.app.schema`; generate both from one manifest value and restart the bridge.",
        ),
        guidance(
            "diagnose_bridge_process",
            "`app_bridge_process` means a runner-owned process exited before readiness. Inspect captured bounded stderr and the exit status, then run the resolved command and arguments in the resolved working directory with the same explicit test boot path.",
        ),
        guidance(
            "diagnose_secondary_broken_pipe",
            "A broken pipe reported during shutdown or teardown often means the bridge process already crashed or closed its transport. Treat it as a secondary failure: inspect the earliest reported failure, child exit status, and captured stderr before debugging shutdown framing.",
        ),
        guidance(
            "diagnose_hello_or_describe_timeout",
            "`hello_timeout` means transport exists but the application did not send `hello`; `describe_timeout` means authentication completed but no correlated `schema` arrived. These states distinguish process/transport startup from protocol progress.",
        ),
    ];
    value.examples = vec![
        example(
            "readiness triage",
            "failure: app_bridge_handshake / bridge_readiness_timeout\nprocess state: runner listener exists; bridge has not connected\ninspect: runtime.configuration, child stderr, WEBTEST_BRIDGE handling, test boot path",
            "diagnostic_playbook",
            "runtime_failure",
        ),
        example(
            "teardown correlation triage",
            "failure: app_bridge_protocol / unknown_response_id\nprocess state: protocol loop is active; response ID has no matching request\ninspect: shutdown request ID and shutdown_ok ID; never hardcode either",
            "diagnostic_playbook",
            "runtime_failure",
        ),
        example(
            "secondary broken pipe triage",
            "failure: broken pipe during shutdown or teardown\nprocess state: bridge peer may already have exited or closed transport\ninspect: earliest failure, child exit status, captured stderr; diagnose shutdown framing only if no earlier cause exists",
            "diagnostic_playbook",
            "runtime_failure",
        ),
    ];
    value.related = vec![
        "runtime.configuration".into(),
        "app.configuration".into(),
        "app.bridge".into(),
        "app.bridge.example".into(),
        "app.protocol".into(),
    ];
    value
}

fn runtime_configuration_reference() -> ConstructDescription {
    let mut value = base_construct(
        "runtime.configuration",
        "Resolved runtime configuration",
        "runtime_inspection",
        "webtest describe runtime.configuration [--project <path>] --reporter json",
        "Inspect the project configuration that WebTest resolved for application lifecycle, application-provider, and browser/server startup without exposing environment values or unredacted secret-like arguments.",
    );
    value.search_terms = vec![
        "resolved adapter transport command arguments".into(),
        "working directory schema path".into(),
        "browser base URL server base URL".into(),
        "per-test deadline provider call timeout".into(),
        "configuration debugging".into(),
        "inspect project startup owned health".into(),
    ];
    value.constraints = vec![
        constraint(
            "runtime_configuration_project",
            "discovery",
            "project",
            "Run from a project or pass the describe command's project path option. Without a discovered `webtest.toml`, the topic explains the fields but has no `resolved_configuration` object.",
        ),
        constraint(
            "runtime_configuration_redaction",
            "presentation",
            "secrets",
            "Environment entries and bridge tokens are never reported. Arguments following secret-like flags and inline secret-like assignments are replaced with `<redacted>`.",
        ),
    ];
    value.guidance = vec![
        guidance(
            "runtime_configuration_fields",
            "The machine-readable `resolved_configuration` object reports selected adapter and transport, resolved command and arguments, working directory, schema path, application ownership and whether health is configured, browser and server base URLs, the per-test deadline, and the distinct provider-call default. Absent optional configuration is represented as null or an empty argument list; timeout values are integer milliseconds.",
        ),
        guidance(
            "runtime_configuration_inspect_startup",
            "Project-mode `webtest inspect` uses the reported application command, ownership, health configuration, working directory, and browser base URL. Passing an absolute HTTP(S) URL selects standalone inspection and skips the project application lifecycle.",
        ),
        guidance(
            "runtime_configuration_transport",
            "`selected_transport` is the configured selection. For `auto`, the concrete runtime transport is host-dependent and may fall back from local IPC to loopback TCP.",
        ),
    ];
    value.examples = vec![
        example(
            "inspect current project",
            "webtest describe runtime.configuration --reporter json",
            "cli_invocation",
            "project_directory",
        ),
        example(
            "inspect selected project",
            "webtest describe runtime.configuration --project path/to/project --reporter json",
            "cli_invocation",
            "shell",
        ),
    ];
    value.related = vec!["app.configuration".into(), "app.diagnostics".into()];
    value
}

fn guidance(code: &str, summary: &str) -> GuidanceDescription {
    GuidanceDescription {
        code: code.into(),
        summary: summary.into(),
    }
}

fn app_call_example(name: &str, source: &str) -> SourceExample {
    let mut value = example(name, source, "statement_fragment", "scope.server");
    value.prerequisites = vec![
        "the configured app schema defines create_user with the shown parameters and result fields"
            .into(),
    ];
    value
}
