use super::*;
use rowan::{Language, ast::AstNode};

#[test]
fn modules_helpers_and_fixture_phases_are_lossless_typed_views() {
    let source = r#"import { user_fixture, email_for as email } from "./fixtures/users.webtest"
/// Build a stable address.
export fn email_for(name: String) -> String = name + "@example.com"
fn assert_user(email: String) -> String { server { expect email != "" } provide email }
export fixture user_fixture(email: String) -> { id: Int, email: String } scope test {
    setup server { let user = { id: 1, email: email } provide user }
    teardown server { expect user.id == 1 }
}
test "caller" { let user = use user_fixture(email: "a@example.com") expect user.id == 1 }
"#;
    let parsed = parse(source);
    assert_eq!(parsed.syntax().text().to_string(), source);
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    let root = ast::Root::cast(parsed.syntax()).unwrap();
    let import = root.imports().next().unwrap();
    assert_eq!(
        import.path().unwrap().value().as_deref(),
        Some("./fixtures/users.webtest")
    );
    let alias = import.bindings().nth(1).unwrap();
    assert_eq!(alias.name().unwrap().text(), "email_for");
    assert_eq!(alias.alias().unwrap().text(), "email");
    let helper = root.functions().next().unwrap();
    assert!(helper.exported());
    assert_eq!(helper.documentation(), "Build a stable address.");
    assert_eq!(
        helper.parameters().next().unwrap().name().unwrap().text(),
        "name"
    );
    assert!(helper.expression().is_some());
    let fixture = root.fixtures().next().unwrap();
    assert_eq!(fixture.lifetime(), ast::FixtureLifetime::Test);
    assert!(fixture.setups().next().unwrap().server_body().is_some());
    assert!(fixture.teardowns().next().unwrap().server_body().is_some());
    assert_eq!(root.tests().count(), 1);
    let use_call = root
        .syntax()
        .descendants()
        .find_map(ast::UseExpr::cast)
        .unwrap();
    assert!(matches!(use_call.call(), Some(ast::Expr::Call(_))));
    for raw in 0..=SyntaxKind::Eof as u16 {
        let kind = WebtestLanguage::kind_from_raw(rowan::SyntaxKind(raw));
        assert_eq!(WebtestLanguage::kind_to_raw(kind).0, raw);
    }
}

#[test]
fn declarations_recover_from_invalid_and_half_typed_input_without_losing_text() {
    for source in [
        "import",
        "import {",
        "import { user as } from",
        "import { } from",
        "export",
        "export fn",
        "fn helper(",
        "fn helper(value:) ->",
        "fn helper(value: String) -> String =",
        "export fixture user() -> String scope {",
        "fixture user() -> String { setup server { let value =",
        "fixture user() -> String scope worker shareable { teardown browser {",
        "test \"é\" { let value = use",
    ] {
        let parsed = parse(source);
        assert_eq!(parsed.syntax().text().to_string(), source);
        assert!(
            !parsed.errors().is_empty(),
            "accepted malformed input {source}"
        );
    }
    let parsed = parse("export fn broken(\n test \"after\" {}");
    assert_eq!(
        parsed.syntax().text().to_string(),
        "export fn broken(\n test \"after\" {}"
    );
    assert_eq!(ast::Root::cast(parsed.syntax()).unwrap().tests().count(), 1);
}
