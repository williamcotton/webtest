use super::catalog::{base_construct, constraint, example, parameter, provider_failures};
use super::*;

pub(super) fn constructs() -> Vec<ConstructDescription> {
    vec![
        provider_overview(),
        manifest_reference(),
        protocol_reference(),
        implementation_reference(),
    ]
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
            "For a new or custom bridge, inspect `app.schema`, then `app.protocol`, then `app.pseudocode`; inspect the exact project operation separately after loading its manifest.",
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
        "app.schema".into(),
        "app.protocol".into(),
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
        "app.pseudocode".into(),
    ];
    value.availability.runtime_requires = vec!["native_app_bridge".into()];
    value
}

fn implementation_reference() -> ConstructDescription {
    let mut value = base_construct(
        "app.pseudocode",
        "Application Bridge implementation outline",
        "implementation_reference",
        "Protocol 1 state machine pseudocode",
        "Outline a custom bridge without replacing the normative Protocol 1 schemas, limits, or state semantics.",
    );
    value.search_terms = vec![
        "custom bridge".into(),
        "host implementation".into(),
        "pseudocode".into(),
        "SDK".into(),
    ];
    value.guidance = vec![
        guidance(
            "prefer_bridge_sdk",
            "Prefer a maintained bridge SDK. A custom implementation must still implement bounded framing, authentication, schema verification, correlation, value validation, deadlines, and shutdown.",
        ),
        guidance(
            "implementation_source_of_truth",
            "Keep one in-process manifest value for SDK export and the live schema response so the checked-in offline manifest and runtime description cannot drift silently.",
        ),
        guidance(
            "application_readiness",
            "For a runner-managed web server, configure app.health so WebTest performs bounded HTTP readiness polling after the bridge connects and before the protocol handshake proceeds.",
        ),
    ];
    value.examples = vec![
        example(
            "SDK integration outline",
            r#"manifest = load_or_define_manifest()
bridge = sdk_bridge(manifest)
bridge.register_each_application_handler()
if explicit_test_boot_mode:
    bridge.connect_from_environment()  // blocks until bounded shutdown"#,
            "pseudocode",
            "application_test_boot",
        ),
        example(
            "webtest init echo handler",
            r#"call(id, "echo", arguments, deadline_ms) =>
    require(arguments.message is a string)
    send({type: result, id, value: arguments.message})"#,
            "pseudocode",
            "protocol_state_machine",
        ),
        example(
            "no-SDK Protocol 1 loop",
            r#"endpoint = WEBTEST_BRIDGE if present else stdio
token = require(WEBTEST_TOKEN)
if endpoint starts with "tcp://":
    host, port = parse_uri(endpoint)
    require(host is loopback)
    io = connect_tcp(host, port)
else:
    io = connect_supported_local_endpoint_or_stdio(endpoint)
send({type: hello, protocol_versions: [1], sdk, sdk_version, token,
      capabilities: {cancel: false, events: false}})
hello_ok = receive_bounded_frame()
require(hello_ok.protocol == 1)
limit = min(local_limit, hello_ok.max_message_bytes)

loop:
    message = receive_bounded_frame(limit)
    describe(id) => send({type: schema, id, protocol: 1,
                          schema_hash: manifest.schema_hash,
                          functions: manifest.functions})
    call(id, function, arguments, deadline_ms) =>
        validate arguments against manifest
        result = run the registered handler within deadline_ms
        validate result against manifest
        send({type: result, id, value: result}),
        or send({type: error, id, code, message, retryable, data})
    ping(id) => send({type: pong, id})
    shutdown(id) => drain bounded work; send({type: shutdown_ok, id}); close"#,
            "pseudocode",
            "protocol_state_machine",
        ),
    ];
    value.related = vec!["app.protocol".into(), "app.schema".into()];
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
