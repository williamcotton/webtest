use super::*;

pub(super) fn core_constructs() -> BTreeMap<String, ConstructDescription> {
    let mut constructs = BTreeMap::new();
    for construct in [
        declaration_test(),
        scope_construct("server", Capability::Server),
        scope_construct("browser", Capability::Browser),
        let_construct(),
    ] {
        constructs.insert(construct.id.clone(), construct);
    }
    for construct in locator_constructs()
        .into_iter()
        .chain(browser_constructs())
        .chain(assertion_constructs())
        .chain(type_constructs())
        .chain(capability_constructs())
        .chain(super::app_bridge::constructs())
    {
        constructs.insert(construct.id.clone(), construct);
    }
    constructs
}

pub(super) fn base_construct(
    id: impl Into<String>,
    name: impl Into<String>,
    construct_kind: impl Into<String>,
    syntax: impl Into<String>,
    summary: impl Into<String>,
) -> ConstructDescription {
    let id = id.into();
    let syntax = syntax.into();
    ConstructDescription {
        kind: "construct_description".into(),
        description_schema_version: DESCRIPTION_SCHEMA_VERSION,
        language_version: language_version(),
        id,
        name: name.into(),
        construct_kind: construct_kind.into(),
        syntax_forms: vec![SyntaxForm {
            id: "default".into(),
            elements: vec![SyntaxElement::RuleReference {
                rule: syntax.clone(),
            }],
        }],
        syntax,
        summary: summary.into(),
        search_terms: Vec::new(),
        parameters: Vec::new(),
        return_type: None,
        retry_safe: None,
        produces_value: None,
        result_rule: None,
        requires_capabilities: Vec::new(),
        allowed_contexts: Vec::new(),
        effects: Vec::new(),
        failure_modes: Vec::new(),
        constraints: Vec::new(),
        guidance: Vec::new(),
        examples: Vec::new(),
        related: Vec::new(),
        availability: Availability {
            analysis: true,
            runtime_requires: Vec::new(),
            configuration_prerequisites: Vec::new(),
        },
        provenance: Provenance {
            kind: "core".into(),
            content_trust: "installed".into(),
            schema_identity: None,
        },
        truncation: Vec::new(),
    }
}

pub(super) fn parameter(
    name: &str,
    ty: Type,
    required: bool,
    position: Option<usize>,
    named: bool,
    syntax_role: &str,
    grammar_rule: &str,
) -> ParameterDescription {
    ParameterDescription {
        name: name.into(),
        ty,
        required,
        position,
        named,
        default: None,
        secret: false,
        documentation: String::new(),
        syntax_role: syntax_role.into(),
        grammar_rule: grammar_rule.into(),
    }
}

pub(super) fn constraint(
    code: &str,
    phase: &str,
    subject: &str,
    summary: &str,
) -> ConstraintDescription {
    ConstraintDescription {
        code: code.into(),
        phase: phase.into(),
        subject: subject.into(),
        summary: summary.into(),
        details: None,
    }
}

pub(super) fn example(name: &str, source: &str, source_kind: &str, context: &str) -> SourceExample {
    SourceExample {
        name: name.into(),
        source: source.into(),
        source_kind: source_kind.into(),
        enclosing_context: context.into(),
        prerequisites: Vec::new(),
    }
}

fn declaration_test() -> ConstructDescription {
    let mut value = base_construct(
        "declaration.test",
        "test",
        "declaration",
        "test <name> { <flow_statement>* }",
        "Declare a named sequential test flow.",
    );
    value.parameters = vec![
        parameter(
            "name",
            Type::String,
            true,
            Some(0),
            false,
            "declaration_name",
            "StringLiteral",
        ),
        parameter(
            "body",
            Type::Record(BTreeMap::new()),
            true,
            None,
            false,
            "body",
            "flow_block",
        ),
    ];
    value.produces_value = Some(false);
    value.requires_capabilities = vec![Capability::Test];
    value.allowed_contexts = vec!["source_file".into()];
    value.constraints = vec![constraint(
        "sequential_test_flow",
        "runtime",
        "body",
        "Statements execute in source order and stop after the first failed step.",
    )];
    value.examples = vec![
        example(
            "empty test",
            "test \"example\" {}",
            "declaration_fragment",
            "source_file",
        ),
        example(
            "browser test",
            "test \"home\" { browser { open \"http://example.test\" } }",
            "source_file",
            "source_file",
        ),
    ];
    value
}

fn scope_construct(name: &str, capability: Capability) -> ConstructDescription {
    let mut value = base_construct(
        format!("scope.{name}"),
        name,
        "scope",
        format!("{name} {{ <{name}_statement>* }}"),
        match capability {
            Capability::Server => {
                "Enter the server-provider capability for setup and fixture work."
            }
            Capability::Browser => {
                "Enter the browser capability for page operations and assertions."
            }
            _ => "Enter a capability scope.",
        },
    );
    value.parameters = vec![parameter(
        "body",
        Type::Record(BTreeMap::new()),
        true,
        None,
        false,
        "body",
        &format!("{name}_statement"),
    )];
    value.produces_value = Some(false);
    value.requires_capabilities = vec![capability];
    value.allowed_contexts = vec!["flow_block".into()];
    value.constraints = vec![constraint(
        "capability_scoped_statements",
        "analysis",
        "body",
        "Every statement in the body must be legal in this capability domain.",
    )];
    value.examples = match capability {
        Capability::Server => vec![
            example(
                "server block",
                "server { let value = 1 }",
                "block_fragment",
                "flow_block",
            ),
            example(
                "provider setup",
                "server { let response = http.get(\"http://example.test/api\") }",
                "block_fragment",
                "flow_block",
            ),
        ],
        _ => vec![
            example(
                "browser block",
                "browser { open \"http://example.test\" }",
                "block_fragment",
                "flow_block",
            ),
            example(
                "browser interaction",
                "browser { click role(\"button\", name: \"Save\") }",
                "block_fragment",
                "flow_block",
            ),
        ],
    };
    value
}

fn let_construct() -> ConstructDescription {
    let mut value = base_construct(
        "statement.let",
        "let",
        "statement",
        "let <name> [: <type>] = <value>",
        "Bind a typed value for later statements in the enclosing sequential flow.",
    );
    value.parameters = vec![
        parameter(
            "name",
            Type::String,
            true,
            Some(0),
            false,
            "bound_variable",
            "Identifier",
        ),
        parameter("type", Type::Json, false, None, false, "annotation", "Type"),
        parameter(
            "value",
            Type::Json,
            true,
            Some(1),
            false,
            "operand",
            "expression",
        ),
    ];
    value.produces_value = Some(false);
    value.allowed_contexts = vec![
        "flow_block".into(),
        "scope.server".into(),
        "scope.browser".into(),
    ];
    value.constraints = vec![constraint(
        "binding_visible_after_declaration",
        "analysis",
        "name",
        "The binding is visible only after this statement in the enclosing sequential flow.",
    )];
    value.examples = vec![
        example(
            "inferred binding",
            "let count = 1",
            "statement_fragment",
            "flow_block",
        ),
        example(
            "typed binding",
            "let email: String = \"alice@example.com\"",
            "statement_fragment",
            "scope.server",
        ),
    ];
    value
}

fn locator_constructs() -> Vec<ConstructDescription> {
    let specs = [
        (
            "id",
            "id(<id>)",
            "Locate an element by exact DOM ID.",
            vec!["DOM", "identifier"],
            "id(\"submit\")",
            "click id(\"submit\")",
        ),
        (
            "label",
            "label(<label>)",
            "Locate a form-oriented control by its exact normalized accessible name.",
            vec!["form", "field", "accessible"],
            "label(\"Email\")",
            "fill label(\"Email\") with \"alice@example.com\"",
        ),
        (
            "text",
            "text(<text>)",
            "Locate an element by exact normalized user-facing text.",
            vec!["copy", "content", "visible text"],
            "text(\"Welcome\")",
            "expect text(\"Welcome\").visible",
        ),
        (
            "placeholder",
            "placeholder(<placeholder>)",
            "Locate an input or textarea by exact placeholder text.",
            vec!["input", "hint", "form"],
            "placeholder(\"Search\")",
            "fill placeholder(\"Search\") with \"WebTest\"",
        ),
        (
            "test_id",
            "test_id(<id>)",
            "Locate an element by the configured exact test-ID attribute.",
            vec!["data-testid", "automation id"],
            "test_id(\"checkout-submit\")",
            "click test_id(\"checkout-submit\")",
        ),
        (
            "css",
            "css(<selector>)",
            "Locate elements with an explicit CSS selector escape hatch.",
            vec!["selector", "escape hatch"],
            "css(\"button.primary\")",
            "click css(\"button.primary\")",
        ),
        (
            "xpath",
            "xpath(<expression>)",
            "Locate elements with an explicit XPath escape hatch.",
            vec!["selector", "escape hatch"],
            "xpath(\"//button\")",
            "click xpath(\"//button\")",
        ),
    ];
    let mut values = specs
        .into_iter()
        .map(|(name, syntax, summary, terms, minimal, composed)| {
            let mut value =
                base_construct(format!("locator.{name}"), name, "locator", syntax, summary);
            value.parameters = vec![parameter(
                if name == "css" {
                    "selector"
                } else if name == "xpath" {
                    "expression"
                } else {
                    name
                },
                Type::String,
                true,
                Some(0),
                false,
                "argument",
                "StringLiteral",
            )];
            let parameter_name = value.parameters[0].name.clone();
            value.syntax_forms = vec![SyntaxForm {
                id: "call".into(),
                elements: vec![
                    SyntaxElement::Literal {
                        value: format!("{name}("),
                    },
                    SyntaxElement::Slot {
                        parameter: Some(parameter_name),
                        parameter_group: None,
                    },
                    SyntaxElement::Literal { value: ")".into() },
                ],
            }];
            value.return_type = Some(Type::Locator);
            value.requires_capabilities = vec![Capability::Browser];
            value.allowed_contexts = vec!["scope.browser".into()];
            value.search_terms = terms.into_iter().map(str::to_owned).collect();
            value.failure_modes = vec!["locator_not_found".into(), "locator_ambiguous".into()];
            if matches!(name, "css" | "xpath") {
                value.failure_modes.push("locator_invalid".into());
            }
            value.constraints = vec![constraint(
                "singular_consumer_requires_unique_match",
                "runtime",
                "locator result",
                "An operation that requires a present target rejects multiple matches; hidden and detached state checks may succeed with no match.",
            )];
            value.examples = vec![
                example(
                    "locator",
                    minimal,
                    "locator_fragment",
                    "browser.click.target",
                ),
                example(
                    "composed locator",
                    composed,
                    "statement_fragment",
                    "scope.browser",
                ),
            ];
            value.availability.runtime_requires = vec!["native_browser".into()];
            value
        })
        .collect::<Vec<_>>();
    let mut role = base_construct(
        "locator.role",
        "role",
        "locator",
        "role(<role>, name: <name>?)",
        "Locate an element by accessibility role and optional accessible name.",
    );
    role.syntax_forms = vec![SyntaxForm {
        id: "default".into(),
        elements: vec![
            SyntaxElement::Literal {
                value: "role(".into(),
            },
            SyntaxElement::Slot {
                parameter: Some("role".into()),
                parameter_group: None,
            },
            SyntaxElement::Optional {
                elements: vec![
                    SyntaxElement::Literal {
                        value: ", name: ".into(),
                    },
                    SyntaxElement::Slot {
                        parameter: Some("name".into()),
                        parameter_group: None,
                    },
                ],
            },
            SyntaxElement::Literal { value: ")".into() },
        ],
    }];
    role.parameters = vec![
        parameter(
            "role",
            Type::String,
            true,
            Some(0),
            false,
            "argument",
            "StringLiteral",
        ),
        parameter(
            "name",
            Type::String,
            false,
            None,
            true,
            "argument",
            "StringLiteral",
        ),
    ];
    role.return_type = Some(Type::Locator);
    role.requires_capabilities = vec![Capability::Browser];
    role.allowed_contexts = vec!["scope.browser".into()];
    role.search_terms = vec![
        "accessibility".into(),
        "accessible name".into(),
        "button".into(),
        "control".into(),
    ];
    role.failure_modes = vec!["locator_not_found".into(), "locator_ambiguous".into()];
    role.constraints = vec![
        constraint(
            "exact_accessible_match",
            "runtime",
            "role,name",
            "Role and name use exact case-sensitive matching after whitespace normalization.",
        ),
        constraint(
            "singular_consumer_requires_unique_match",
            "runtime",
            "locator result",
            "An operation that requires a present target rejects multiple matches; hidden and detached state checks may succeed with no match.",
        ),
    ];
    role.examples = vec![
        example(
            "role only",
            "role(\"button\")",
            "locator_fragment",
            "browser.click.target",
        ),
        example(
            "role and accessible name",
            "click role(\"button\", name: \"Sign in\")",
            "statement_fragment",
            "scope.browser",
        ),
    ];
    role.related = vec!["browser.click".into(), "assertion.locator_state".into()];
    role.availability.runtime_requires = vec!["native_browser".into()];
    values.insert(1, role);
    values
}

fn browser_constructs() -> Vec<ConstructDescription> {
    let specs = [
        (
            "open",
            "open <url>",
            "Navigate the current page to an absolute or configured relative URL.",
            vec![parameter(
                "url",
                Type::String,
                true,
                Some(0),
                false,
                "operand",
                "expression",
            )],
            "open \"http://example.test\"",
            "open \"/login\"",
            vec!["navigation"],
        ),
        (
            "evaluate",
            "evaluate <expression>",
            "Evaluate an explicit JavaScript string in the current page without producing a WebTest value.",
            vec![parameter(
                "expression",
                Type::String,
                true,
                Some(0),
                false,
                "operand",
                "StringLiteral",
            )],
            "evaluate \"document.body.dataset.ready = 'true'\"",
            "evaluate \"window.bootstrap && window.bootstrap()\"",
            vec!["javascript", "page setup"],
        ),
        (
            "click",
            "click <target>",
            "Wait for one visible, enabled, stable, unobscured element and activate it with pointer input.",
            vec![parameter(
                "target",
                Type::Locator,
                true,
                Some(0),
                false,
                "operand",
                "locator_expression",
            )],
            "click role(\"button\", name: \"Save\")",
            "click text(\"Continue\")",
            vec!["activate", "button", "pointer", "press control"],
        ),
        (
            "fill",
            "fill <target> with <value>",
            "Replace the value of one editable element.",
            vec![
                parameter(
                    "target",
                    Type::Locator,
                    true,
                    Some(0),
                    false,
                    "operand",
                    "locator_expression",
                ),
                parameter(
                    "value",
                    Type::String,
                    true,
                    Some(1),
                    false,
                    "operand",
                    "expression",
                ),
            ],
            "fill label(\"Email\") with \"alice@example.com\"",
            "fill placeholder(\"Search\") with \"WebTest\"",
            vec!["input", "replace", "form", "textbox"],
        ),
        (
            "type",
            "type <target> with <value>",
            "Append text to one editable element using browser text input.",
            vec![
                parameter(
                    "target",
                    Type::Locator,
                    true,
                    Some(0),
                    false,
                    "operand",
                    "locator_expression",
                ),
                parameter(
                    "value",
                    Type::String,
                    true,
                    Some(1),
                    false,
                    "operand",
                    "expression",
                ),
            ],
            "type label(\"Name\") with \"Alice\"",
            "type placeholder(\"Search\") with \" docs\"",
            vec!["input", "append", "keyboard", "textbox"],
        ),
        (
            "press",
            "press <target> key <key>",
            "Send a validated key or key chord after focusing one visible, enabled, stable element.",
            vec![
                parameter(
                    "target",
                    Type::Locator,
                    true,
                    Some(0),
                    false,
                    "operand",
                    "locator_expression",
                ),
                parameter(
                    "key",
                    Type::String,
                    true,
                    Some(1),
                    false,
                    "operand",
                    "expression",
                ),
            ],
            "press label(\"Search\") key \"Enter\"",
            "press role(\"textbox\", name: \"Query\") key \"Control+a\"",
            vec!["keyboard", "enter", "shortcut", "key chord"],
        ),
        (
            "check",
            "check <target>",
            "Ensure one checkbox or radio target is checked.",
            vec![parameter(
                "target",
                Type::Locator,
                true,
                Some(0),
                false,
                "operand",
                "locator_expression",
            )],
            "check label(\"Remember me\")",
            "check role(\"checkbox\", name: \"Terms\")",
            vec!["checkbox", "radio", "toggle"],
        ),
        (
            "uncheck",
            "uncheck <target>",
            "Ensure one checkbox target is unchecked.",
            vec![parameter(
                "target",
                Type::Locator,
                true,
                Some(0),
                false,
                "operand",
                "locator_expression",
            )],
            "uncheck label(\"Remember me\")",
            "uncheck role(\"checkbox\", name: \"Marketing\")",
            vec!["checkbox", "toggle", "clear"],
        ),
        (
            "select",
            "select <target> option <option>",
            "Select one exact option label or value in a select control.",
            vec![
                parameter(
                    "target",
                    Type::Locator,
                    true,
                    Some(0),
                    false,
                    "operand",
                    "locator_expression",
                ),
                parameter(
                    "option",
                    Type::String,
                    true,
                    Some(1),
                    false,
                    "operand",
                    "expression",
                ),
            ],
            "select label(\"Timezone\") option \"UTC\"",
            "select id(\"country\") option \"US\"",
            vec!["dropdown", "combobox", "option"],
        ),
        (
            "hover",
            "hover <target>",
            "Move pointer input to one visible unobscured element.",
            vec![parameter(
                "target",
                Type::Locator,
                true,
                Some(0),
                false,
                "operand",
                "locator_expression",
            )],
            "hover text(\"Account\")",
            "hover role(\"button\", name: \"Help\")",
            vec!["pointer", "mouseover", "tooltip"],
        ),
        (
            "wait.locator",
            "wait <target>.<state> [within <timeout>]",
            "Wait until a locator reaches a requested semantic state.",
            vec![
                parameter(
                    "target",
                    Type::Locator,
                    true,
                    Some(0),
                    false,
                    "operand",
                    "locator_expression",
                ),
                parameter(
                    "state",
                    Type::String,
                    true,
                    Some(1),
                    false,
                    "state",
                    "locator_state",
                ),
                parameter(
                    "timeout",
                    Type::Duration,
                    false,
                    None,
                    false,
                    "clause",
                    "Duration",
                ),
            ],
            "wait text(\"Ready\").visible",
            "wait id(\"spinner\").detached within 5s",
            vec!["wait", "state", "visible", "attached"],
        ),
        (
            "wait.url",
            "wait url(<url>) [within <timeout>]",
            "Wait until the current URL exactly matches an absolute or configured relative URL.",
            vec![
                parameter(
                    "url",
                    Type::String,
                    true,
                    Some(0),
                    false,
                    "operand",
                    "StringLiteral",
                ),
                parameter(
                    "timeout",
                    Type::Duration,
                    false,
                    None,
                    false,
                    "clause",
                    "Duration",
                ),
            ],
            "wait url(\"http://example.test/dashboard\")",
            "wait url(\"/dashboard\") within 5s",
            vec!["wait", "navigation", "address", "redirect"],
        ),
    ];
    specs
        .into_iter()
        .map(
            |(suffix, syntax, summary, parameters, first, second, search_terms)| {
                let id = format!("browser.{suffix}");
                let name = suffix.rsplit('.').next().unwrap_or(suffix);
                let mut value = base_construct(&id, name, "browser_operation", syntax, summary);
                value.parameters = parameters;
                value.syntax_forms = browser_syntax_forms(suffix);
                value.produces_value = Some(false);
                value.requires_capabilities = vec![Capability::Browser];
                value.allowed_contexts = vec!["scope.browser".into()];
                value.search_terms = search_terms.into_iter().map(str::to_owned).collect();
                value.effects = browser_effects(suffix);
                value.failure_modes = browser_failure_modes(suffix);
                value.constraints = browser_constraints(suffix);
                value.examples = vec![
                    example(
                        "minimal operation",
                        first,
                        "statement_fragment",
                        "scope.browser",
                    ),
                    example(
                        "composed operation",
                        second,
                        "statement_fragment",
                        "scope.browser",
                    ),
                ];
                if suffix == "open" || suffix == "wait.url" {
                    value.examples[1].prerequisites =
                        vec!["configured browser base URL for a relative URL".into()];
                }
                value.related = vec!["scope.browser".into()];
                value.availability.runtime_requires = vec!["native_browser".into()];
                value
            },
        )
        .collect()
}

fn browser_effects(operation: &str) -> Vec<String> {
    string_list(match operation {
        "open" => &["page_navigation"],
        "evaluate" => &["page_script_evaluation"],
        "click" => &["browser_pointer_input", "page_may_navigate"],
        "check" | "uncheck" => &["browser_pointer_input", "browser_form_input"],
        "hover" => &["browser_pointer_input"],
        "fill" | "type" | "press" => &["browser_pointer_input", "browser_keyboard_input"],
        "select" => &["browser_form_input"],
        "wait.locator" | "wait.url" => &["page_state_wait"],
        _ => &[],
    })
}

fn browser_failure_modes(operation: &str) -> Vec<String> {
    string_list(match operation {
        "open" => &[
            "navigation_failed",
            "navigation_timeout",
            "browser_command_timeout",
            "browser_disconnected",
        ],
        "evaluate" => &[
            "evaluation_failed",
            "browser_command_timeout",
            "browser_disconnected",
        ],
        "click" | "hover" => &[
            "locator_not_found",
            "locator_ambiguous",
            "locator_invalid",
            "element_detached",
            "element_not_visible",
            "element_unstable",
            "element_disabled",
            "element_obscured",
            "action_timeout",
            "browser_disconnected",
        ],
        "fill" | "type" => &[
            "locator_not_found",
            "locator_ambiguous",
            "locator_invalid",
            "element_detached",
            "element_not_visible",
            "element_unstable",
            "element_disabled",
            "element_not_editable",
            "action_timeout",
            "browser_disconnected",
        ],
        "press" => &[
            "locator_not_found",
            "locator_ambiguous",
            "locator_invalid",
            "element_detached",
            "element_not_visible",
            "element_unstable",
            "element_disabled",
            "invalid_key",
            "action_timeout",
            "browser_disconnected",
        ],
        "check" | "uncheck" => &[
            "locator_not_found",
            "locator_ambiguous",
            "locator_invalid",
            "element_detached",
            "element_not_visible",
            "element_unstable",
            "element_disabled",
            "element_obscured",
            "element_not_editable",
            "assertion_failed",
            "action_timeout",
            "browser_disconnected",
        ],
        "select" => &[
            "locator_not_found",
            "locator_ambiguous",
            "locator_invalid",
            "element_detached",
            "element_not_visible",
            "element_unstable",
            "element_disabled",
            "element_not_editable",
            "option_not_found",
            "option_ambiguous",
            "action_timeout",
            "browser_disconnected",
        ],
        "wait.locator" => &[
            "locator_not_found",
            "locator_ambiguous",
            "locator_invalid",
            "element_not_visible",
            "assertion_failed",
            "browser_disconnected",
        ],
        "wait.url" => &["url_mismatch", "evaluation_failed", "browser_disconnected"],
        _ => &[],
    })
}

fn browser_constraints(operation: &str) -> Vec<ConstraintDescription> {
    let mut constraints = match operation {
        "open" | "wait.url" => vec![constraint(
            "relative_url_requires_base_url",
            "configuration",
            "url",
            "A relative URL requires browser.base_url and uses the normal URL resolution policy.",
        )],
        "evaluate" => vec![constraint(
            "literal_javascript_source",
            "syntax",
            "expression",
            "The JavaScript source is an explicit string literal; evaluate does not interpolate WebTest expressions and produces no binding value.",
        )],
        "wait.locator" => vec![locator_state_constraint()],
        _ => vec![constraint(
            "unique_target_before_deadline",
            "runtime",
            "target",
            "The locator must resolve to exactly one visible, enabled, stable target before the operation deadline.",
        )],
    };
    match operation {
        "fill" | "type" => constraints.push(constraint(
            "editable_target",
            "runtime",
            "target",
            "The resolved target must accept text editing.",
        )),
        "press" => constraints.push(constraint(
            "validated_key_chord",
            "runtime",
            "key",
            "The key string must be one supported key or modifier chord.",
        )),
        "check" | "uncheck" => constraints.push(constraint(
            "checkable_target",
            "runtime",
            "target",
            "The resolved target must expose a checked state, and the final checked state is verified.",
        )),
        "select" => constraints.push(constraint(
            "select_option_target",
            "runtime",
            "target,option",
            "The target must be a select element and exactly one option must match by value or normalized label.",
        )),
        _ => {}
    }
    constraints
}

fn string_list(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn locator_state_constraint() -> ConstraintDescription {
    constraint(
        "locator_state_vocabulary",
        "syntax",
        "state",
        "The state is one of attached, detached, visible, hidden, enabled, disabled, checked, or unchecked.",
    )
}

fn browser_syntax_forms(operation: &str) -> Vec<SyntaxForm> {
    let slot = |parameter: &str| SyntaxElement::Slot {
        parameter: Some(parameter.into()),
        parameter_group: None,
    };
    let literal = |value: &str| SyntaxElement::Literal {
        value: value.into(),
    };
    let elements = match operation {
        "open" => vec![literal("open "), slot("url")],
        "evaluate" => vec![literal("evaluate "), slot("expression")],
        "click" | "check" | "uncheck" | "hover" => {
            vec![literal(&format!("{operation} ")), slot("target")]
        }
        "fill" | "type" => vec![
            literal(&format!("{operation} ")),
            slot("target"),
            literal(" with "),
            slot("value"),
        ],
        "press" => vec![
            literal("press "),
            slot("target"),
            literal(" key "),
            slot("key"),
        ],
        "select" => vec![
            literal("select "),
            slot("target"),
            literal(" option "),
            slot("option"),
        ],
        "wait.locator" => vec![
            literal("wait "),
            slot("target"),
            literal("."),
            slot("state"),
            SyntaxElement::Optional {
                elements: vec![literal(" within "), slot("timeout")],
            },
        ],
        "wait.url" => vec![
            literal("wait url("),
            slot("url"),
            literal(")"),
            SyntaxElement::Optional {
                elements: vec![literal(" within "), slot("timeout")],
            },
        ],
        _ => vec![SyntaxElement::RuleReference {
            rule: operation.into(),
        }],
    };
    vec![SyntaxForm {
        id: "default".into(),
        elements,
    }]
}

fn assertion_constructs() -> Vec<ConstructDescription> {
    let mut locator = base_construct(
        "assertion.locator_state",
        "locator_state",
        "assertion",
        "expect <target>.<state> [within <timeout>]",
        "Assert that a locator reaches one semantic state before the assertion deadline.",
    );
    locator.parameters = vec![
        parameter(
            "target",
            Type::Locator,
            true,
            Some(0),
            false,
            "operand",
            "locator_expression",
        ),
        parameter(
            "state",
            Type::String,
            true,
            Some(1),
            false,
            "state",
            "locator_state",
        ),
        parameter(
            "timeout",
            Type::Duration,
            false,
            None,
            false,
            "clause",
            "Duration",
        ),
    ];
    locator.syntax_forms = vec![SyntaxForm {
        id: "default".into(),
        elements: vec![
            SyntaxElement::Literal {
                value: "expect ".into(),
            },
            SyntaxElement::Slot {
                parameter: Some("target".into()),
                parameter_group: None,
            },
            SyntaxElement::Literal { value: ".".into() },
            SyntaxElement::Slot {
                parameter: Some("state".into()),
                parameter_group: None,
            },
            SyntaxElement::Optional {
                elements: vec![
                    SyntaxElement::Literal {
                        value: " within ".into(),
                    },
                    SyntaxElement::Slot {
                        parameter: Some("timeout".into()),
                        parameter_group: None,
                    },
                ],
            },
        ],
    }];
    locator.produces_value = Some(false);
    locator.requires_capabilities = vec![Capability::Browser, Capability::Test];
    locator.allowed_contexts = vec!["scope.browser".into()];
    locator.failure_modes = vec![
        "locator_not_found".into(),
        "locator_ambiguous".into(),
        "locator_invalid".into(),
        "element_not_visible".into(),
        "assertion_failed".into(),
        "browser_disconnected".into(),
    ];
    locator.constraints = vec![
        locator_state_constraint(),
        constraint(
            "state_before_deadline",
            "runtime",
            "target,state",
            "The target must satisfy the requested state before the assertion deadline.",
        ),
    ];
    locator.examples = vec![
        example(
            "visible assertion",
            "expect text(\"Welcome\").visible",
            "statement_fragment",
            "scope.browser",
        ),
        example(
            "detached assertion",
            "expect id(\"spinner\").detached within 5s",
            "statement_fragment",
            "scope.browser",
        ),
    ];
    locator.search_terms = vec!["assert element".into(), "visible".into(), "state".into()];
    locator.availability.runtime_requires = vec!["native_browser".into()];

    let mut url = base_construct(
        "assertion.url",
        "url",
        "assertion",
        "expect url(<url>) [within <timeout>]",
        "Assert that the current URL exactly matches an absolute or configured relative URL.",
    );
    url.parameters = vec![
        parameter(
            "url",
            Type::String,
            true,
            Some(0),
            false,
            "operand",
            "StringLiteral",
        ),
        parameter(
            "timeout",
            Type::Duration,
            false,
            None,
            false,
            "clause",
            "Duration",
        ),
    ];
    url.syntax_forms = vec![SyntaxForm {
        id: "default".into(),
        elements: vec![
            SyntaxElement::Literal {
                value: "expect url(".into(),
            },
            SyntaxElement::Slot {
                parameter: Some("url".into()),
                parameter_group: None,
            },
            SyntaxElement::Literal { value: ")".into() },
            SyntaxElement::Optional {
                elements: vec![
                    SyntaxElement::Literal {
                        value: " within ".into(),
                    },
                    SyntaxElement::Slot {
                        parameter: Some("timeout".into()),
                        parameter_group: None,
                    },
                ],
            },
        ],
    }];
    url.produces_value = Some(false);
    url.requires_capabilities = vec![Capability::Browser, Capability::Test];
    url.allowed_contexts = vec!["scope.browser".into()];
    url.failure_modes = vec!["url_mismatch".into(), "browser_disconnected".into()];
    url.constraints = vec![constraint(
        "relative_url_requires_base_url",
        "configuration",
        "url",
        "A relative URL requires browser.base_url.",
    )];
    url.examples = vec![
        example(
            "absolute URL",
            "expect url(\"http://example.test/dashboard\")",
            "statement_fragment",
            "scope.browser",
        ),
        example(
            "relative URL",
            "expect url(\"/dashboard\") within 5s",
            "statement_fragment",
            "scope.browser",
        ),
    ];
    url.examples[1].prerequisites = vec!["configured browser base URL".into()];
    url.search_terms = vec!["address".into(), "navigation".into(), "assert URL".into()];
    url.availability.runtime_requires = vec!["native_browser".into()];

    let mut value = base_construct(
        "assertion.value",
        "expect",
        "assertion",
        "expect <expression>",
        "Assert a boolean expression or a typed comparison/matcher expression.",
    );
    value.parameters = vec![parameter(
        "expression",
        Type::Bool,
        true,
        Some(0),
        false,
        "operand",
        "expression",
    )];
    value.syntax_forms = vec![SyntaxForm {
        id: "default".into(),
        elements: vec![
            SyntaxElement::Literal {
                value: "expect ".into(),
            },
            SyntaxElement::Slot {
                parameter: Some("expression".into()),
                parameter_group: None,
            },
        ],
    }];
    value.produces_value = Some(false);
    value.requires_capabilities = vec![Capability::Test];
    value.allowed_contexts = vec![
        "flow_block".into(),
        "scope.server".into(),
        "scope.browser".into(),
    ];
    value.failure_modes = vec!["assertion_failed".into()];
    value.constraints = vec![
        constraint(
            "typed_matcher_operands",
            "analysis",
            "expression",
            "Comparison and matcher operands must have compatible static types.",
        ),
        constraint(
            "structural_matches_pattern",
            "analysis",
            "matches right operand",
            "The matches operator decodes Json or record data against a type name, record shape, or one-element list shape; it does not interpret a regular expression.",
        ),
    ];
    value.examples = vec![
        example(
            "boolean assertion",
            "expect true",
            "statement_fragment",
            "flow_block",
        ),
        example(
            "value comparison",
            "let count = 2\nexpect count == 2",
            "statement_fragment",
            "flow_block",
        ),
    ];
    value.search_terms = vec![
        "compare".into(),
        "equal".into(),
        "contains".into(),
        "matches".into(),
    ];
    vec![locator, url, value]
}

fn type_constructs() -> Vec<ConstructDescription> {
    let types = vec![
        ("Null", Type::Null, "The null literal type."),
        ("Bool", Type::Bool, "Boolean true/false values."),
        ("Int", Type::Int, "Signed integer values."),
        ("Float", Type::Float, "Floating-point numeric values."),
        ("String", Type::String, "UTF-8 string values."),
        (
            "Duration",
            Type::Duration,
            "Positive bounded duration values.",
        ),
        (
            "Url",
            Type::Url,
            "Nominal URL string values supplied by a typed provider.",
        ),
        ("Json", Type::Json, "Dynamically shaped JSON wire values."),
        (
            "List",
            Type::List(Box::new(Type::String)),
            "Homogeneous list values.",
        ),
        (
            "Option",
            Type::Option(Box::new(Type::String)),
            "Optional nullable values.",
        ),
        (
            "Record",
            Type::Record(BTreeMap::new()),
            "Statically shaped record values.",
        ),
        (
            "StatusCode",
            Type::StatusCode,
            "HTTP response status codes.",
        ),
        (
            "Headers",
            Type::Headers,
            "Case-insensitive HTTP header collections.",
        ),
        ("Bytes", Type::Bytes, "Opaque byte collections."),
        (
            "Response",
            Type::Response(Box::new(Type::Json)),
            "Typed HTTP provider response handles.",
        ),
        (
            "ProcessResult",
            Type::ProcessResult,
            "Direct-process execution results.",
        ),
        (
            "FilePath",
            Type::FilePath,
            "Sandboxed filesystem result paths.",
        ),
        (
            "TempDirectory",
            Type::TempDirectory,
            "Runner-owned temporary directory resources.",
        ),
        (
            "Locator",
            Type::Locator,
            "Protocol-neutral semantic browser locators.",
        ),
        (
            "BrowserPage",
            Type::BrowserPage,
            "A non-transferable browser page capability value.",
        ),
    ];
    types
        .into_iter()
        .map(|(name, ty, summary)| {
            let syntax = match name {
                "List" => "List<T>",
                "Option" => "Option<T>",
                "Response" => "Response<T>",
                "Record" => "{ <field>: <Type>, ... }",
                _ => name,
            };
            let mut value = base_construct(
                format!("type.{name}"),
                name,
                "type",
                syntax,
                summary,
            );
            value.return_type = Some(ty.clone());
            value.allowed_contexts = vec!["let.annotation".into()];
            value.constraints = vec![constraint(
                if ty.is_transferable() {
                    "transferable_value"
                } else {
                    "non_transferable_resource"
                },
                "analysis",
                "value",
                if ty.is_transferable() {
                    "Values of this type may cross from a server block into a later browser block."
                } else {
                    "Values of this type cannot cross capability boundaries or appear in portable wire data."
                },
            )];
            value.examples = vec![example(
                "type form",
                type_form_example(name),
                "type_fragment",
                "let.annotation",
            )];
            if let Some(usage) = type_usage_example(name) {
                value.examples.push(usage);
            }
            if name == "Record" {
                value.guidance = vec![
                    GuidanceDescription {
                        code: "record_member_access".into(),
                        summary: "An unknown record field produces `semantic.unknown_member`. When a known field name is close, the diagnostic includes a bounded `member_candidate` repair hint.".into(),
                    },
                ];
            }
            if name == "Json" {
                value.guidance = vec![
                    GuidanceDescription {
                        code: "json_decode_pattern".into(),
                        summary: "Decode `response.json` by assigning it to an explicitly typed record before reading fields. Direct member access on `Json` produces `semantic.unknown_member`.".into(),
                    },
                ];
                value.examples.push(example(
                    "JSON response decoding into typed record",
                    "let response = http.post(\"http://example.test/api/users\", json: { email: \"alice@example.com\" })\nexpect response.status == 201\nlet user: { id: Int, email: String } = response.json\nexpect user.id > 0",
                    "statement_fragment",
                    "scope.server",
                ));
                value.examples
                    .last_mut()
                    .expect("JSON example")
                    .prerequisites = vec![
                    "the endpoint returns JSON with integer id and string email fields".into(),
                ];
            }
            value.search_terms = vec![ty.to_string(), "static type".into()];
            value
        })
        .collect()
}

fn type_form_example(name: &str) -> &str {
    match name {
        "List" => "List<String>",
        "Option" => "Option<String>",
        "Response" => "Response<Json>",
        "Record" => "{ name: String }",
        _ => name,
    }
}

fn type_usage_example(name: &str) -> Option<SourceExample> {
    let (source, context, prerequisites) = match name {
        "Null" => ("let value: Null = null", "flow_block", vec![]),
        "Bool" => ("let value: Bool = true", "flow_block", vec![]),
        "Int" => ("let value: Int = 1", "flow_block", vec![]),
        "Float" => ("let value: Float = 1.0", "flow_block", vec![]),
        "String" => ("let value: String = \"example\"", "flow_block", vec![]),
        "Duration" => ("let value: Duration = 5s", "flow_block", vec![]),
        "Json" => (
            "let value: Json = { name: \"Alice\" }",
            "flow_block",
            vec![],
        ),
        "List" => (
            "let value: List<String> = [\"a\", \"b\"]",
            "flow_block",
            vec![],
        ),
        "Option" => ("let value: Option<String> = null", "flow_block", vec![]),
        "Record" => (
            "let value: { name: String } = { name: \"Alice\" }",
            "flow_block",
            vec![],
        ),
        "StatusCode" => ("let value: StatusCode = 200", "flow_block", vec![]),
        "Headers" => (
            "let response = http.get(\"http://example.test\")\nlet value: Headers = response.headers",
            "scope.server",
            vec!["a reachable HTTP endpoint when the example is executed"],
        ),
        "Bytes" => (
            "let response = http.get(\"http://example.test\")\nlet value: Bytes = response.body",
            "scope.server",
            vec!["a reachable HTTP endpoint when the example is executed"],
        ),
        "Response" => (
            "let value: Response<Json> = http.get(\"http://example.test\")",
            "scope.server",
            vec!["a reachable HTTP endpoint when the example is executed"],
        ),
        "ProcessResult" => (
            "let value: ProcessResult = process.run(\"example\")",
            "scope.server",
            vec!["an executable named example when the example is executed"],
        ),
        "FilePath" => (
            "let value: FilePath = fs.write_text(\"example.txt\", contents: \"example\")",
            "scope.server",
            vec!["write access within the configured filesystem sandbox"],
        ),
        "TempDirectory" => (
            "let value: TempDirectory = fs.temp_dir()",
            "scope.server",
            vec!["the native sandboxed filesystem provider"],
        ),
        "Url" | "Locator" | "BrowserPage" => {
            let mut value = example(
                "nested type form",
                &format!("Option<{name}>"),
                "type_fragment",
                "let.annotation",
            );
            value.prerequisites = vec![
                "a value producer is required before a concrete binding can use this type".into(),
            ];
            return Some(value);
        }
        _ => return None,
    };
    let mut value = example("typed binding", source, "statement_fragment", context);
    value.prerequisites = prerequisites.into_iter().map(str::to_owned).collect();
    Some(value)
}

fn capability_constructs() -> Vec<ConstructDescription> {
    [
        (
            Capability::Pure,
            "Pure expressions with no external host effect.",
        ),
        (Capability::Server, "Typed server-provider operations."),
        (
            Capability::Browser,
            "Browser page operations and semantic locators.",
        ),
        (Capability::Test, "Test declarations and assertions."),
    ]
    .into_iter()
    .map(|(capability, summary)| {
        let name = capability.to_string();
        let mut value = base_construct(
            format!("capability.{name}"),
            &name,
            "capability",
            &name,
            summary,
        );
        value.requires_capabilities = vec![capability];
        value.allowed_contexts = match capability {
            Capability::Pure => vec!["expression".into()],
            Capability::Server => vec!["scope.server".into()],
            Capability::Browser => vec!["scope.browser".into()],
            Capability::Test => vec!["source_file".into(), "flow_block".into()],
        };
        value.constraints = vec![constraint(
            "capability_checked_statically",
            "analysis",
            "context",
            "Use outside an allowed capability context is rejected before execution.",
        )];
        value.examples = match capability {
            Capability::Pure => vec![
                example(
                    "literal",
                    "let value = 1",
                    "statement_fragment",
                    "flow_block",
                ),
                example(
                    "expression",
                    "expect 1 + 1 == 2",
                    "statement_fragment",
                    "flow_block",
                ),
            ],
            Capability::Server => vec![
                example("server scope", "server {}", "block_fragment", "flow_block"),
                example(
                    "provider call",
                    "server { let response = http.get(\"http://example.test\") }",
                    "block_fragment",
                    "flow_block",
                ),
            ],
            Capability::Browser => vec![
                example(
                    "browser scope",
                    "browser {}",
                    "block_fragment",
                    "flow_block",
                ),
                example(
                    "browser action",
                    "browser { click text(\"Continue\") }",
                    "block_fragment",
                    "flow_block",
                ),
            ],
            Capability::Test => vec![
                example("test", "test \"x\" {}", "source_file", "source_file"),
                example(
                    "assertion",
                    "expect true",
                    "statement_fragment",
                    "flow_block",
                ),
            ],
        };
        value
    })
    .collect()
}

pub(super) fn add_provider_constructs(
    registry: &mut BTreeMap<String, ConstructDescription>,
    providers: &ProviderRegistry,
) {
    for schema in providers.schemas() {
        let installed =
            providers.schema_provenance(&schema.name.0) == Some(ProviderSchemaProvenance::BuiltIn);
        for operation in schema.operations.values() {
            let construct =
                provider_construct(&schema.name.0, operation, &schema.hash(), installed);
            registry.insert(construct.id.clone(), construct);
        }
    }
}

fn provider_construct(
    provider: &str,
    operation: &OperationSchema,
    schema_hash: &str,
    installed: bool,
) -> ConstructDescription {
    let syntax = provider_syntax(provider, operation);
    let mut value = base_construct(
        format!("provider.{provider}.{}", operation.name.0),
        format!("{provider}.{}", operation.name.0),
        "provider_operation",
        &syntax,
        sanitize_documentation(&operation.documentation),
    );
    value.syntax_forms = vec![SyntaxForm {
        id: "call".into(),
        elements: vec![
            SyntaxElement::Literal {
                value: format!("{provider}.{}(", operation.name.0),
            },
            SyntaxElement::Repeat {
                separator: ", ".into(),
                elements: vec![SyntaxElement::Slot {
                    parameter: None,
                    parameter_group: Some("arguments".into()),
                }],
            },
            SyntaxElement::Literal { value: ")".into() },
        ],
    }];
    value.parameters = operation
        .parameters
        .iter()
        .enumerate()
        .map(|(index, schema)| provider_parameter(schema, index))
        .collect();
    value.return_type = Some(operation.result.clone());
    value.retry_safe = Some(operation.retry_safe);
    value.requires_capabilities = vec![operation.capability];
    value.allowed_contexts = vec!["scope.server".into()];
    value.effects = vec![
        match provider {
            "http" => "network_request",
            "process" => "direct_process",
            "fs" => "sandboxed_filesystem",
            _ => "provider_call",
        }
        .into(),
    ];
    value.failure_modes = provider_failures(provider);
    value.constraints = vec![constraint(
        "schema_checked_call",
        "analysis",
        "arguments",
        "Argument names, required status, static types, and capability are checked against this exact provider schema.",
    )];
    value.guidance = vec![GuidanceDescription {
        code: "retry_metadata_only".into(),
        summary: "retry_safe is schema metadata for callers; the sequential WebTest runtime does not automatically retry provider calls.".into(),
    }];
    if provider == "http" {
        value.constraints.push(constraint(
            "http_status_is_data",
            "runtime",
            "result.status",
            "An HTTP 4xx or 5xx status is returned as data until an assertion rejects it.",
        ));
    }
    value.search_terms = provider_search_terms(provider, &operation.name.0);
    if installed {
        value.examples = provider_examples(provider, operation);
    } else {
        value.guidance.push(GuidanceDescription {
            code: "project_examples_not_declared".into(),
            summary: "The validated project provider schema defines types and documentation but no canonical source examples; WebTest does not invent application-specific argument values.".into(),
        });
        value
            .availability
            .configuration_prerequisites
            .push("the project provider schema is loaded and validated".into());
    }
    value.availability.runtime_requires = vec![if installed {
        format!("native_{provider}")
    } else if provider == "app" {
        "configured_app_provider".into()
    } else {
        format!("registered_{provider}_provider")
    }];
    value.related = vec![format!("provider.{provider}"), "scope.server".into()];
    value.provenance = Provenance {
        kind: if installed {
            "built_in_provider"
        } else {
            "project_provider"
        }
        .into(),
        content_trust: if installed {
            "installed"
        } else {
            "project_supplied"
        }
        .into(),
        schema_identity: Some(schema_hash.into()),
    };
    value
}

fn provider_parameter(schema: &ParameterSchema, index: usize) -> ParameterDescription {
    ParameterDescription {
        name: schema.name.clone(),
        ty: schema.ty.clone(),
        required: schema.required,
        position: schema.positional.then_some(index),
        named: !schema.positional,
        default: schema
            .default
            .as_ref()
            .and_then(webtest_provider::value_to_json),
        secret: schema.secret,
        documentation: sanitize_documentation(&schema.documentation),
        syntax_role: "argument".into(),
        grammar_rule: "expression".into(),
    }
}

fn provider_syntax(provider: &str, operation: &OperationSchema) -> String {
    let arguments = operation
        .parameters
        .iter()
        .map(|parameter| {
            let value = format!("<{}>", type_syntax(&parameter.ty));
            let value = if parameter.positional {
                value
            } else {
                format!("{}: {value}", parameter.name)
            };
            if parameter.required {
                value
            } else {
                format!("{value}?")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{provider}.{}({arguments})", operation.name.0)
}

fn provider_examples(provider: &str, operation: &OperationSchema) -> Vec<SourceExample> {
    let required = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.required)
        .map(|parameter| provider_example_argument(provider, parameter))
        .collect::<Vec<_>>();
    let mut composed = required.clone();
    if let Some(optional) = operation
        .parameters
        .iter()
        .find(|parameter| !parameter.required && !parameter.secret)
    {
        composed.push(provider_example_argument(provider, optional));
    }
    let call = |arguments: &[ExampleArgument]| {
        format!(
            "let result = {provider}.{}({})",
            operation.name.0,
            arguments
                .iter()
                .map(|argument| argument.source.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut minimal = example(
        "minimal provider call",
        &call(&required),
        "statement_fragment",
        "scope.server",
    );
    minimal.prerequisites = provider_example_prerequisites(provider, operation, &required);
    let mut expanded = if composed.len() > required.len() {
        example(
            "provider call with an option",
            &call(&composed),
            "statement_fragment",
            "scope.server",
        )
    } else {
        let source = call(&required).replacen(
            "let result =",
            &format!("let result: {} =", type_syntax(&operation.result)),
            1,
        );
        example(
            "provider result annotation",
            &source,
            "statement_fragment",
            "scope.server",
        )
    };
    expanded.prerequisites = provider_example_prerequisites(provider, operation, &composed);
    vec![minimal, expanded]
}

#[derive(Clone)]
struct ExampleArgument {
    source: String,
    prerequisites: Vec<String>,
}

fn provider_example_argument(provider: &str, parameter: &ParameterSchema) -> ExampleArgument {
    let mut value = example_value(&parameter.ty);
    if provider == "http" && parameter.name == "url" {
        value.source = "\"http://example.test\"".into();
    }
    if !parameter.positional {
        value.source = format!("{}: {}", parameter.name, value.source);
    }
    value
}

fn example_value(ty: &Type) -> ExampleArgument {
    let (source, prerequisites) = match ty {
        Type::Null => ("null".into(), vec![]),
        Type::Bool => ("true".into(), vec![]),
        Type::Int => ("1".into(), vec![]),
        Type::StatusCode => ("200".into(), vec![]),
        Type::Float => ("1.0".into(), vec![]),
        Type::Duration => ("5s".into(), vec![]),
        Type::String => ("\"example\"".into(), vec![]),
        Type::List(_) => ("[]".into(), vec![]),
        Type::Option(_) => ("null".into(), vec![]),
        Type::Json => ("{}".into(), vec![]),
        Type::Record(fields) => {
            let mut prerequisites = Vec::new();
            let fields = fields
                .iter()
                .filter(|(_, field)| !field.optional)
                .map(|(name, field)| {
                    let value = example_value(&field.ty);
                    prerequisites.extend(value.prerequisites);
                    format!("{name}: {}", value.source)
                })
                .collect::<Vec<_>>()
                .join(", ");
            (
                if fields.is_empty() {
                    "{}".into()
                } else {
                    format!("{{ {fields} }}")
                },
                prerequisites,
            )
        }
        Type::Url => binding_example("url_value", "Url"),
        Type::Headers => binding_example("headers_value", "Headers"),
        Type::Bytes => binding_example("bytes_value", "Bytes"),
        Type::Response(inner) => binding_example(
            "response_value",
            &format!("Response<{}>", type_syntax(inner)),
        ),
        Type::ProcessResult => binding_example("process_result", "ProcessResult"),
        Type::FilePath => binding_example("file_path", "FilePath"),
        Type::TempDirectory => binding_example("temp_directory", "TempDirectory"),
        Type::Locator => binding_example("locator_value", "Locator"),
        Type::BrowserPage => binding_example("browser_page", "BrowserPage"),
        Type::Unknown => binding_example("value", "the required type"),
    };
    ExampleArgument {
        source,
        prerequisites,
    }
}

fn type_syntax(ty: &Type) -> String {
    match ty {
        Type::List(inner) => format!("List<{}>", type_syntax(inner)),
        Type::Option(inner) => format!("Option<{}>", type_syntax(inner)),
        Type::Response(inner) => format!("Response<{}>", type_syntax(inner)),
        Type::Record(fields) if fields.is_empty() => "{}".into(),
        Type::Record(fields) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(name, field)| format!(
                    "{name}{}: {}",
                    if field.optional { "?" } else { "" },
                    type_syntax(&field.ty)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => ty.to_string(),
    }
}

fn binding_example(name: &str, ty: &str) -> (String, Vec<String>) {
    (
        name.into(),
        vec![format!("a prior binding named {name} with type {ty}")],
    )
}

fn provider_example_prerequisites(
    provider: &str,
    operation: &OperationSchema,
    arguments: &[ExampleArgument],
) -> Vec<String> {
    let mut prerequisites = arguments
        .iter()
        .flat_map(|argument| argument.prerequisites.clone())
        .collect::<BTreeSet<_>>();
    let external = match (provider, operation.name.0.as_str()) {
        ("http", _) => "a reachable HTTP endpoint at the example URL",
        ("process", _) => "an executable named example on the configured process path",
        ("fs", "copy_fixture" | "read_text") => {
            "the referenced project-relative source path exists"
        }
        ("fs", _) => "the native sandboxed filesystem provider is available",
        ("app", _) => "the configured application provider implements this operation",
        _ => "the described provider is registered at runtime",
    };
    prerequisites.insert(external.into());
    prerequisites.into_iter().collect()
}

pub(super) fn provider_failures(provider: &str) -> Vec<String> {
    match provider {
        "http" => vec![
            "http_transport".into(),
            "response_too_large".into(),
            "provider_invalid_argument".into(),
            "provider_unavailable".into(),
        ],
        "process" => vec![
            "process_spawn".into(),
            "process_timeout".into(),
            "process_output_too_large".into(),
            "provider_unavailable".into(),
        ],
        "fs" => vec![
            "filesystem".into(),
            "path_escape".into(),
            "provider_unavailable".into(),
        ],
        "app" => vec![
            "app_provider_failure".into(),
            "app_bridge_handshake".into(),
            "app_bridge_protocol".into(),
            "app_bridge_transport".into(),
            "app_bridge_process".into(),
            "app_schema_drift".into(),
            "app_bridge_validation".into(),
            "app_bridge_timeout".into(),
            "provider_unavailable".into(),
        ],
        _ => vec![
            "provider_invalid_argument".into(),
            "provider_not_registered".into(),
            "provider_unavailable".into(),
        ],
    }
}

fn provider_search_terms(provider: &str, operation: &str) -> Vec<String> {
    let mut terms = vec!["server setup".into(), provider.into(), operation.into()];
    terms.extend(match provider {
        "http" => vec![
            "request".into(),
            "fetch".into(),
            "REST".into(),
            "JSON".into(),
        ],
        "process" => vec!["command".into(), "executable".into(), "subprocess".into()],
        "fs" => vec![
            "file".into(),
            "fixture".into(),
            "temporary directory".into(),
        ],
        "app" => vec![
            "application bridge".into(),
            "application fixture".into(),
            "test data".into(),
        ],
        _ => Vec::new(),
    });
    terms
}

fn sanitize_documentation(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(1_024)
        .collect()
}
