//! Installed module/helper/fixture language facts. Project declarations are a query.
use super::catalog::{base_construct, constraint, example, parameter};
use super::*;
fn workspace_example(name: &str, source: &str, support: &str) -> SourceExample {
    let mut value = example(name, source, "workspace_sources", "source_file");
    value
        .inputs
        .insert("support.webtest".into(), support.into());
    value
}
pub(super) fn constructs() -> Vec<ConstructDescription> {
    let helper = "export fn normalize(value: String) -> String = lower(value)";
    let fixture = "export fixture seed() -> Int scope suite shareable { setup { provide 7 } teardown { expect 1 == 1 } }";
    let imports = vec![
        workspace_example(
            "named import",
            "import { normalize } from \"./support.webtest\" test \"name\" { let value = normalize(\"É\") expect value == \"é\" }",
            helper,
        ),
        workspace_example(
            "import alias",
            "import { seed as number } from \"./support\" test \"fixture\" { let value = use number() expect value == 7 }",
            fixture,
        ),
    ];
    let mut import = base_construct(
        "language.import",
        "import",
        "declaration",
        "import { <name> [as <alias>] } from <StringLiteral>",
        "Resolve named exports from an explicit workspace module without running application code.",
    );
    import.allowed_contexts = vec!["source_file".into()];
    import.search_terms = vec![
        "module".into(),
        "alias".into(),
        "source roots".into(),
        "relative path".into(),
    ];
    import.failure_modes = [
        "semantic.missing_module",
        "semantic.non_exported_import",
        "semantic.missing_export",
        "semantic.import_alias_collision",
        "semantic.ambiguous_module",
        "semantic.module_case_collision",
        "semantic.import_cycle",
        "semantic.module_outside_source_roots",
    ]
    .map(str::to_owned)
    .to_vec();
    import.constraints = vec![constraint(
        "local_modules",
        "analysis",
        "path",
        "Named imports and aliases only. Paths are relative to the importing file or use project/ under the configured root. An omitted extension adds .webtest. Resolution stays inside source roots; canonical aliases identify one module. Import cycles are rejected with related locations.",
    )];
    import.examples = imports.clone();
    let mut export = base_construct(
        "language.export",
        "export",
        "declaration",
        "export (fn | fixture) <declaration>",
        "Make a typed helper or fixture available to named imports.",
    );
    export.allowed_contexts = vec!["source_file".into()];
    export.search_terms = vec!["public".into(), "module".into()];
    export.examples = imports;
    export.constraints = vec![constraint(
        "explicit_export",
        "analysis",
        "declaration",
        "Only helpers and fixtures are exported. Parameter and result types are declared; private declarations remain local to their module.",
    )];
    export.failure_modes = vec![
        "semantic.duplicate_declaration".into(),
        "syntax.expected_declaration_name".into(),
    ];
    let mut function = base_construct(
        "language.fn",
        "fn",
        "declaration",
        "[export] fn <name>(<name>: <Type>, ...) -> <Type> (= <expression> | [server | browser] { <statements> })",
        "Define a typed, acyclic helper that lowers through the shared execution plan.",
    );
    function.allowed_contexts = vec!["source_file".into()];
    function.search_terms = vec![
        "function".into(),
        "helper".into(),
        "parameter".into(),
        "return".into(),
    ];
    function.produces_value = Some(true);
    function.result_rule = Some("The declared result type. Calls evaluate arguments once in author order, then execute with isolated parameters and local bindings.".into());
    function.effects = vec!["Pure and statement helpers have explicit source-mapped scopes and debugger safe points. Lazy boolean operands remain lazy. Statement helpers require the capabilities inferred from their body.".into()];
    function.constraints = vec![constraint(
        "acyclic_calls",
        "analysis",
        "body",
        "No recursion, dynamic dispatch, or higher-order values. Statement helpers are direct binding values or statements. Native results cannot escape a shorter resource lifetime.",
    )];
    function.failure_modes = vec![
        "semantic.recursive_helper".into(),
        "semantic.helper_capability_mismatch".into(),
        "semantic.missing_helper_result".into(),
        "semantic.type_mismatch".into(),
        "Body failures retain their original typed codes and source locations.".into(),
    ];
    function.examples = vec![
        example(
            "pure helper",
            "fn normalize(value: String) -> String = lower(value) test \"name\" { let value = normalize(\"É\") expect value == \"é\" }",
            "source_file",
            "source_file",
        ),
        example(
            "statement helper",
            "fn verify(value: Int) -> Int { expect value > 0 provide value } test \"value\" { let value = verify(7) expect value == 7 }",
            "source_file",
            "source_file",
        ),
    ];
    let mut declaration = base_construct(
        "language.fixture",
        "fixture",
        "declaration",
        "[export] fixture <name>(<typed_parameters>?) -> <Type> [scope test|file|worker|suite] [shareable] { setup [server|browser] { ... provide <value> } [teardown [server|browser] { ... }] }",
        "Acquire a cached value with explicit setup, lifetime ownership, and awaited teardown.",
    );
    declaration.allowed_contexts = vec!["source_file".into()];
    declaration.search_terms = vec![
        "fixture".into(),
        "setup".into(),
        "teardown".into(),
        "lifetime".into(),
        "worker".into(),
        "suite".into(),
    ];
    declaration.effects = vec!["Test fixtures are owned by the test or nearest retry attempt. File fixtures belong to one executed file; worker fixtures belong to one scheduler worker; suite fixtures belong to the invocation. Concurrent consumers coordinate one setup per definition, owner, and canonical argument set.".into(), "Dependencies acquire before consumers and tear down in reverse successful acquisition order. Setup locals remain available in teardown. Cancellation joins partial setup and native cleanup. Teardown failures aggregate with primary results.".into()];
    declaration.constraints = vec![constraint(
        "fixture_ownership",
        "analysis",
        "lifetime",
        "Parameters must be transferable. Longer-lived fixtures cannot depend on shorter-lived ones. File, worker, and suite fixtures require explicit shareable and transferable results; native results are limited to test fixtures. Browser contexts remain owned by their fixture lifetime. Dependencies must be acyclic. Acquire dependencies during setup; teardown may use retained bindings but cannot acquire new fixtures.",
    )];
    declaration.failure_modes = vec![
        "semantic.fixture_dependency_cycle".into(),
        "semantic.fixture_acquisition_in_teardown".into(),
        "semantic.fixture_lifetime_dependency".into(),
        "semantic.fixture_not_shareable".into(),
        "semantic.type_mismatch".into(),
        "fixture_budget_exceeded".into(),
        "cleanup_fixture_failed".into(),
        "Setup and teardown retain typed assertion/provider/browser/timeout failures.".into(),
    ];
    declaration.examples = vec![
        example(
            "test lifetime",
            "fixture seed() -> Int { setup { let saved = 7 provide saved } teardown { expect saved == 7 } } test \"seed\" { let value = use seed() expect value == 7 }",
            "source_file",
            "source_file",
        ),
        example(
            "shared suite value",
            "fixture seed() -> Int scope suite shareable { setup { provide 7 } } test \"first\" { let value = use seed() } test \"second\" { let value = use seed() }",
            "source_file",
            "source_file",
        ),
    ];
    let mut acquisition = base_construct(
        "statement.use",
        "use",
        "statement",
        "let <name> = use <fixture>(<arguments>?)",
        "Acquire a resolved fixture, reusing the same cached terminal result within its owner.",
    );
    acquisition.allowed_contexts = vec![
        "flow_block".into(),
        "helper_body".into(),
        "fixture_setup".into(),
        "scope.server".into(),
        "scope.browser".into(),
    ];
    acquisition.produces_value = Some(true);
    acquisition.result_rule = Some("The fixture's declared result with its redaction metadata. Arguments are pure and evaluated once; use is a direct let value or expression statement.".into());
    acquisition.examples = declaration.examples.clone();
    acquisition.failure_modes = declaration.failure_modes.clone();
    acquisition.search_terms = vec!["acquire".into(), "fixture value".into()];
    let builtin = crate::callables::builtin("lower").expect("installed lower signature");
    let mut lower = base_construct(
        builtin.id,
        builtin.name,
        "builtin",
        "lower(value: String) -> String",
        builtin.documentation,
    );
    lower.parameters = vec![parameter(
        "value",
        Type::String,
        true,
        Some(0),
        true,
        "value",
        "expression",
    )];
    lower.return_type = Some(Type::String);
    lower.requires_capabilities = vec![Capability::Pure];
    lower.allowed_contexts = vec!["expression".into()];
    lower.produces_value = Some(true);
    lower.retry_safe = Some(true);
    lower.failure_modes = vec!["semantic.type_mismatch".into()];
    lower.search_terms = vec!["Unicode".into(), "lowercase".into()];
    lower.examples = vec![
        example(
            "Unicode lowercase",
            "test \"lowercase\" { let value = lower(\"É\") expect value == \"é\" }",
            "source_file",
            "source_file",
        ),
        example(
            "named argument",
            "test \"named\" { let value = lower(value: \"NAME\") expect value == \"name\" }",
            "source_file",
            "source_file",
        ),
    ];
    let mut metadata = base_construct(
        "project.metadata",
        "project metadata",
        "workspace",
        "project.declarations",
        "Describe exported helpers, fixtures, and authored tests through the shared bounded workspace query.",
    );
    metadata.allowed_contexts = vec!["static_workspace_query".into()];
    metadata.search_terms = vec![
        "project.declarations".into(),
        "test variants".into(),
        "semantic discovery".into(),
    ];
    metadata.effects = vec!["Reports declared types, documentation, capabilities, fixture lifetimes, source revisions, stable declaration IDs, and default variant IDs. Performs no application execution; limits preserve explicit truncation.".into()];
    metadata.examples = function.examples.clone();
    let mut values = vec![
        import,
        export,
        function,
        declaration,
        acquisition,
        lower,
        metadata,
    ];
    for value in &mut values {
        value.related = vec![
            "language.import".into(),
            "language.fn".into(),
            "language.fixture".into(),
            "statement.use".into(),
            "statement.provide".into(),
            "project.metadata".into(),
        ];
    }
    values
}
