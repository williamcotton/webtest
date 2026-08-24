//! The single lexer, parser, lossless CST, and typed AST facade for WebTest.

pub mod ast;
mod error;
mod kind;
mod lexer;
mod parser;
mod reference;

pub use error::SyntaxError;
pub use kind::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, WebtestLanguage};
pub use parser::{Parse, parse};
pub use reference::{AuthorFacingLanguage, GrammarExample, author_facing_language};

#[cfg(test)]
mod tests {
    use rowan::ast::AstNode;

    use super::*;

    const SOURCE: &str = r#"test "x" {
    // retained
    browser {
        open "http://example.test"
        evaluate "window.bootstrap && window.bootstrap();"
        fill label("Email") with "alice@example.com"
        press placeholder("Search") key "Enter"
        click role("button", name: "Submit")
        expect text("submitted").visible within 5s
        wait url("/done")
    }
}
"#;

    #[test]
    fn cst_is_lossless() {
        let parsed = parse(SOURCE);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        assert_eq!(parsed.syntax().text().to_string(), SOURCE);
    }

    #[test]
    fn typed_ast_is_a_view_over_the_cst() {
        let root = ast::Root::cast(parse(SOURCE).syntax()).expect("root");
        let test = root.tests().next().expect("test");
        assert_eq!(
            test.name().and_then(|token| token.value()).as_deref(),
            Some("x")
        );
        let block = test.browser_blocks().next().expect("browser block");
        let operations: Vec<_> = block.operations().collect();
        assert_eq!(operations.len(), 7);
        let ast::BrowserOperation::Evaluate(evaluate) = &operations[1] else {
            panic!("evaluate")
        };
        assert_eq!(
            evaluate
                .expression()
                .and_then(|token| token.value())
                .as_deref(),
            Some("window.bootstrap && window.bootstrap();")
        );
        let ast::BrowserOperation::Fill(fill) = &operations[2] else {
            panic!("fill")
        };
        assert!(matches!(fill.locator(), Some(ast::Locator::Label(_))));
        assert_eq!(
            fill.value().and_then(|token| token.value()).as_deref(),
            Some("alice@example.com")
        );
        let ast::BrowserOperation::Click(click) = &operations[4] else {
            panic!("click")
        };
        let Some(ast::Locator::Role(role)) = click.locator() else {
            panic!("role")
        };
        assert_eq!(
            role.role().and_then(|token| token.value()).as_deref(),
            Some("button")
        );
        assert_eq!(
            role.name().and_then(|token| token.value()).as_deref(),
            Some("Submit")
        );
        let ast::BrowserOperation::ExpectLocator(expectation) = &operations[5] else {
            panic!("expect")
        };
        assert_eq!(expectation.state(), Some(ast::LocatorState::Visible));
        assert_eq!(
            expectation.timeout().and_then(|token| token.value()),
            Some(std::time::Duration::from_secs(5))
        );
    }

    #[test]
    fn malformed_source_is_lossless_and_diagnostic() {
        let source = "test { browser { click id(💥 }";
        let parsed = parse(source);
        assert!(!parsed.errors().is_empty());
        assert_eq!(parsed.syntax().text().to_string(), source);
    }

    #[test]
    fn unterminated_strings_are_diagnostic() {
        let source = "test \"unfinished";
        let parsed = parse(source);
        assert!(
            parsed
                .errors()
                .iter()
                .any(|error| error.code == "syntax.unterminated_string")
        );
        assert_eq!(parsed.syntax().text().to_string(), source);
    }

    #[test]
    fn all_locator_action_and_state_forms_are_lossless() {
        let source = r##"test "all" { browser {
            click id("one")
            evaluate "window.ready = true"
            fill label("Email") with "a"
            type placeholder("Search") with "b"
            press role("textbox", name: "Query") key "Control+a"
            check test_id("mail")
            uncheck css("#sms")
            select xpath("//select") option "UTC"
            hover text("Account")
            wait id("ready").attached within 250ms
            expect id("gone").detached
            expect id("busy").hidden
            expect id("go").enabled
            expect id("stop").disabled
            expect id("yes").checked
            expect id("no").unchecked
        } }"##;
        let parsed = parse(source);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        assert_eq!(parsed.syntax().text().to_string(), source);
    }

    #[test]
    fn half_typed_locator_does_not_consume_enclosing_block() {
        let source = "test \"x\" { browser { fill role(\"textbox\", name: } }";
        let parsed = parse(source);
        assert!(!parsed.errors().is_empty());
        assert_eq!(parsed.syntax().text().to_string(), source);
        assert_eq!(
            parsed
                .syntax()
                .descendants()
                .filter(|node| node.kind() == SyntaxKind::BrowserBlock)
                .count(),
            1
        );
    }

    #[test]
    fn half_typed_evaluate_does_not_consume_enclosing_block() {
        let source = "test \"x\" { browser { evaluate click id(\"x\") } }";
        let parsed = parse(source);
        assert!(!parsed.errors().is_empty());
        assert_eq!(parsed.syntax().text().to_string(), source);
        assert_eq!(
            parsed
                .errors()
                .iter()
                .filter(|error| error.code == "syntax.expected_expression")
                .count(),
            1
        );
        assert_eq!(
            parsed
                .syntax()
                .descendants()
                .filter(|node| node.kind() == SyntaxKind::ClickStmt)
                .count(),
            1
        );
    }

    #[test]
    fn typed_workflow_language_is_lossless_and_precedence_is_structural() {
        let source = r#"test "typed" {
            server {
                let response = http.post("/users", json: { email: "a@example.test" })
                expect response.status >= 200 && response.status < 300
                let user: { id: Int, email: String, admin?: Bool } = response.json
            }
            browser {
                fill label("Email") with user.email
                expect user.email contains "@"
            }
        }"#;
        let parsed = parse(source);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        assert_eq!(parsed.syntax().text().to_string(), source);
        assert!(
            parsed
                .syntax()
                .descendants()
                .any(|node| node.kind() == SyntaxKind::ServerBlock)
        );
        assert_eq!(
            parsed
                .syntax()
                .descendants()
                .filter(|node| node.kind() == SyntaxKind::BinaryExpr)
                .count(),
            4
        );
    }

    #[test]
    fn half_typed_expression_preserves_the_enclosing_server_block() {
        let source = "test \"x\" { server { let value = 1 + } browser { open \"/\" } }";
        let parsed = parse(source);
        assert!(!parsed.errors().is_empty());
        assert_eq!(parsed.syntax().text().to_string(), source);
        assert_eq!(
            parsed
                .syntax()
                .descendants()
                .filter(|node| matches!(
                    node.kind(),
                    SyntaxKind::ServerBlock | SyntaxKind::BrowserBlock
                ))
                .count(),
            2
        );
    }
}
