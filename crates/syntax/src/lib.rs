//! The single lexer, parser, lossless CST, and typed AST facade for WebTest.

pub mod ast;
mod error;
mod kind;
mod lexer;
mod parser;

pub use error::SyntaxError;
pub use kind::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, WebtestLanguage};
pub use parser::{Parse, parse};

#[cfg(test)]
mod tests {
    use rowan::ast::AstNode;

    use super::*;

    const SOURCE: &str = r#"test "x" {
    // retained
    browser {
        open "http://example.test"
        click id("foo")
        expect text("submitted").visible
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
        assert_eq!(operations.len(), 3);
        match &operations[1] {
            ast::BrowserOperation::Click(click) => match click.locator().expect("locator") {
                ast::Locator::Id(locator) => {
                    assert_eq!(
                        locator.value().and_then(|token| token.value()).as_deref(),
                        Some("foo")
                    );
                }
                ast::Locator::Text(_) => panic!("expected id locator"),
            },
            _ => panic!("expected click"),
        }
        match &operations[2] {
            ast::BrowserOperation::ExpectVisible(expectation) => {
                match expectation.locator().expect("locator") {
                    ast::Locator::Text(locator) => assert_eq!(
                        locator.value().and_then(|token| token.value()).as_deref(),
                        Some("submitted")
                    ),
                    ast::Locator::Id(_) => panic!("expected text locator"),
                }
            }
            _ => panic!("expected visible expectation"),
        }
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
}
