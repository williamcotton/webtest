use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrammarExample {
    pub name: String,
    pub source: String,
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorFacingLanguage {
    pub grammar: BTreeMap<String, String>,
    pub lexical_forms: BTreeMap<String, String>,
    pub string_escapes: BTreeMap<String, String>,
    pub precedence: Vec<String>,
    pub associativity: String,
    pub literal_forms: BTreeMap<String, String>,
    pub type_forms: BTreeMap<String, String>,
    pub reserved_words: Vec<String>,
    pub comment_forms: Vec<String>,
    pub composition: Vec<String>,
    pub examples: Vec<GrammarExample>,
}

/// Returns the stable author-facing projection of the one WebTest grammar.
/// Parser recovery productions and Rowan implementation details are intentionally absent.
pub fn author_facing_language() -> AuthorFacingLanguage {
    AuthorFacingLanguage {
        grammar: [
            ("source_file", "<test_declaration>*"),
            ("test_declaration", "test <StringLiteral> <flow_block>"),
            ("flow_block", "{ <flow_statement>* }"),
            (
                "flow_statement",
                "<let_binding> | <server_block> | <browser_block> | <value_assertion> | <expression_statement>",
            ),
            ("let_binding", "let <Identifier> [: <Type>] = <expression>"),
            ("server_block", "server { <server_statement>* }"),
            (
                "server_statement",
                "<let_binding> | <value_assertion> | <expression_statement>",
            ),
            ("browser_block", "browser { <browser_statement>* }"),
            (
                "browser_statement",
                "<let_binding> | <browser_operation> | <browser_assertion> | <value_assertion> | <expression_statement>",
            ),
            ("value_assertion", "expect <expression>"),
            (
                "provider_call",
                "<provider>.<operation>(<argument_list>?)",
            ),
            (
                "argument_list",
                "<expression> (, <expression>)* (, <name>: <expression>)*",
            ),
            ("locator_expression", "<locator>(<argument_list>?)"),
            (
                "expression",
                "<literal> | <identifier> | <list> | <record> | <member> | <call> | <unary> | <binary> | ( <expression> )",
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
        lexical_forms: [
            (
                "Identifier",
                "ASCII letter or underscore followed by ASCII letters, digits, underscores, or hyphens",
            ),
            (
                "StringLiteral",
                "double-quoted UTF-8 text with backslash escapes",
            ),
            ("Duration", "positive integer followed by ms, s, or m"),
            ("LineComment", "// followed by text through the end of the line"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
        string_escapes: [
            ("\\\"", "double quote"),
            ("\\\\", "backslash"),
            ("\\n", "line feed"),
            ("\\r", "carriage return"),
            ("\\t", "horizontal tab"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
        precedence: vec![
            "||".into(),
            "&&".into(),
            "== != contains matches".into(),
            "< <= > >=".into(),
            "+ -".into(),
            "* /".into(),
            "unary ! -".into(),
            "member access and calls".into(),
        ],
        associativity: "binary operators are left-associative; unary operators bind right"
            .into(),
        literal_forms: [
            ("null", "null"),
            ("boolean", "true | false"),
            ("integer", "ASCII decimal digits"),
            ("float", "ASCII decimal digits, a dot, and decimal digits"),
            ("string", "<StringLiteral>"),
            ("duration", "<Duration>"),
            ("list", "[<expression> (, <expression>)*]"),
            ("record", "{ <name>: <expression> (, <name>: <expression>)* }"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
        type_forms: [
            ("named", "<Identifier>"),
            ("generic", "<Identifier><<Type>>"),
            ("record", "{ <name>?: <Type> (, <name>?: <Type>)* }"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect(),
        reserved_words: [
            "test", "server", "browser", "let", "open", "evaluate", "click", "fill",
            "type", "press", "key", "with", "check", "uncheck", "select", "option",
            "hover", "wait", "expect", "within", "url", "id", "role", "name", "label",
            "text", "placeholder", "test_id", "css", "xpath", "visible", "hidden",
            "attached", "detached", "enabled", "disabled", "checked", "unchecked", "true",
            "false", "null", "contains", "matches",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        comment_forms: vec!["// line comment".into()],
        composition: vec![
            "top-level declarations are tests".into(),
            "server and browser are capability scopes inside a test flow".into(),
            "a binding is visible only after its declaration in the enclosing sequential flow"
                .into(),
            "a transferable server value may be referenced by a later browser block".into(),
        ],
        examples: vec![
            GrammarExample {
                name: "minimal browser test".into(),
                source: "test \"home is visible\" {\n    browser {\n        open \"/\"\n        expect text(\"Home\").visible\n    }\n}".into(),
                source_kind: "source_file".into(),
                prerequisites: vec![
                    "configured browser base URL for a relative URL".into(),
                ],
            },
            GrammarExample {
                name: "server value used by the browser".into(),
                source: "test \"created user signs in\" {\n    server {\n        let response = http.post(\"/api/test/users\", json: { email: \"alice@example.com\" })\n        let user: { id: Int, email: String } = response.json\n    }\n    browser {\n        fill label(\"Email\") with user.email\n    }\n}".into(),
                source_kind: "source_file".into(),
                prerequisites: vec![
                    "configured HTTP base URL for a relative URL".into(),
                ],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_examples_use_the_canonical_parser() {
        for example in author_facing_language().examples {
            let parsed = crate::parse(&example.source);
            assert!(
                parsed.errors().is_empty(),
                "{}: {:?}",
                example.name,
                parsed.errors()
            );
        }
    }
}
