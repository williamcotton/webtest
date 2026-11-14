//! Canonical CST formatter shared by the CLI and editor service.

use webtest_syntax::{Parse, SyntaxKind};

pub fn format_file(parse: &Parse) -> String {
    let mut output = String::new();
    let mut indent = 0usize;
    let mut line_start = true;
    let mut previous = None;

    for token in parse
        .syntax()
        .descendants_with_tokens()
        .filter_map(|it| it.into_token())
    {
        let kind = token.kind();
        let structural_brace = token.parent().is_some_and(|parent| {
            matches!(
                parent.kind(),
                SyntaxKind::RecordExpr
                    | SyntaxKind::PatternRecordExpr
                    | SyntaxKind::RecordType
                    | SyntaxKind::ImportDecl
            )
        });
        match kind {
            SyntaxKind::Whitespace => {}
            SyntaxKind::LineComment => {
                if !line_start
                    && token.prev_token().is_some_and(|previous| {
                        previous.kind() == SyntaxKind::Whitespace && previous.text().contains('\n')
                    })
                {
                    output.push('\n');
                    line_start = true;
                }
                if line_start {
                    push_indent(&mut output, indent);
                } else if !output.ends_with([' ', '\n']) {
                    output.push(' ');
                }
                output.push_str(token.text().trim_end());
                output.push('\n');
                line_start = true;
            }
            SyntaxKind::LBrace => {
                if structural_brace {
                    if line_start {
                        push_indent(&mut output, indent);
                    } else if needs_space(previous, kind) && !output.ends_with(' ') {
                        output.push(' ');
                    }
                    output.push('{');
                    output.push(' ');
                    line_start = false;
                } else {
                    if line_start {
                        push_indent(&mut output, indent);
                    } else if !output.ends_with(' ') {
                        output.push(' ');
                    }
                    output.push('{');
                    output.push('\n');
                    indent += 1;
                    line_start = true;
                }
            }
            SyntaxKind::RBrace => {
                if structural_brace {
                    while output.ends_with(' ') {
                        output.pop();
                    }
                    if output.ends_with(',') {
                        output.pop();
                    }
                    if previous != Some(SyntaxKind::LBrace) {
                        output.push(' ');
                    }
                    output.push('}');
                    line_start = false;
                } else {
                    if !line_start {
                        output.push('\n');
                    }
                    indent = indent.saturating_sub(1);
                    push_indent(&mut output, indent);
                    output.push('}');
                    output.push('\n');
                    line_start = true;
                }
            }
            SyntaxKind::LParen => {
                if line_start {
                    push_indent(&mut output, indent);
                }
                output.push('(');
                line_start = false;
            }
            SyntaxKind::RParen => {
                while output.ends_with(' ') {
                    output.pop();
                }
                output.push(')');
                line_start = false;
            }
            SyntaxKind::LBracket => {
                if line_start {
                    push_indent(&mut output, indent);
                } else if needs_space(previous, kind) && !output.ends_with(' ') {
                    output.push(' ');
                }
                output.push('[');
                if token
                    .parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::CaseList)
                {
                    output.push('\n');
                    indent += 1;
                    line_start = true;
                } else {
                    line_start = false;
                }
            }
            SyntaxKind::RBracket => {
                while output.ends_with(' ') {
                    output.pop();
                }
                if token
                    .parent()
                    .is_some_and(|p| p.kind() == SyntaxKind::CaseList)
                {
                    if !line_start {
                        output.push('\n');
                    }
                    indent = indent.saturating_sub(1);
                    push_indent(&mut output, indent);
                }
                output.push(']');
                line_start = false;
            }
            SyntaxKind::Question => {
                while output.ends_with(' ') {
                    output.pop();
                }
                output.push('?');
                line_start = false;
            }
            SyntaxKind::Dot => {
                while output.ends_with(' ') {
                    output.pop();
                }
                output.push('.');
                line_start = false;
            }
            SyntaxKind::Comma | SyntaxKind::Colon => {
                while output.ends_with(' ') {
                    output.pop();
                }
                output.push(if kind == SyntaxKind::Comma { ',' } else { ':' });
                line_start = false;
                if kind == SyntaxKind::Comma
                    && token
                        .parent()
                        .is_some_and(|p| p.kind() == SyntaxKind::CaseList)
                {
                    output.push('\n');
                    line_start = true;
                }
            }
            _ => {
                let declaration_header = token.parent().is_some_and(|parent| {
                    matches!(
                        parent.kind(),
                        SyntaxKind::FixtureDecl
                            | SyntaxKind::FunctionDecl
                            | SyntaxKind::ActorDecl
                            | SyntaxKind::FixtureSetup
                            | SyntaxKind::FixtureTeardown
                    )
                });
                let bound_select = token.parent().is_some_and(|parent| {
                    parent.kind() == SyntaxKind::ReactiveSelect
                        && parent
                            .parent()
                            .is_some_and(|p| p.kind() == SyntaxKind::LetStmt)
                });
                let modeling_statement = token.parent().is_some_and(|parent| {
                    matches!(
                        parent.kind(),
                        SyntaxKind::ActorDecl
                            | SyntaxKind::ActorBlock
                            | SyntaxKind::GuardScope
                            | SyntaxKind::PatternDecl
                            | SyntaxKind::GuardDecl
                            | SyntaxKind::CheckBlock
                            | SyntaxKind::AssumeStmt
                            | SyntaxKind::VerdictStmt
                            | SyntaxKind::EventArm
                            | SyntaxKind::SelectTimeout
                            | SyntaxKind::CaseDecl
                    ) && parent
                        .children_with_tokens()
                        .filter_map(|element| element.into_token())
                        .find(|candidate| !candidate.kind().is_trivia())
                        .as_ref()
                        == Some(&token)
                        && previous != Some(SyntaxKind::ExportKw)
                });
                let starts_statement = modeling_statement
                    || (!declaration_header
                        && !bound_select
                        && matches!(
                            kind,
                            SyntaxKind::TestKw
                                | SyntaxKind::BrowserKw
                                | SyntaxKind::ServerKw
                                | SyntaxKind::LetKw
                                | SyntaxKind::OpenKw
                                | SyntaxKind::EvaluateKw
                                | SyntaxKind::ClickKw
                                | SyntaxKind::FillKw
                                | SyntaxKind::TypeKw
                                | SyntaxKind::PressKw
                                | SyntaxKind::CheckKw
                                | SyntaxKind::UncheckKw
                                | SyntaxKind::SelectKw
                                | SyntaxKind::HoverKw
                                | SyntaxKind::WaitKw
                                | SyntaxKind::ExpectKw
                        ))
                    || matches!(
                        kind,
                        SyntaxKind::ImportKw
                            | SyntaxKind::ExportKw
                            | SyntaxKind::SetupKw
                            | SyntaxKind::TeardownKw
                    )
                    || (matches!(kind, SyntaxKind::FnKw | SyntaxKind::FixtureKw)
                        && previous != Some(SyntaxKind::ExportKw))
                    || (kind == SyntaxKind::RetryKw
                        && token
                            .parent()
                            .is_some_and(|parent| parent.kind() == SyntaxKind::RetryStmt))
                    || (kind == SyntaxKind::TimeoutKw
                        && token
                            .parent()
                            .is_some_and(|parent| parent.kind() == SyntaxKind::TimeoutStmt))
                    || (kind == SyntaxKind::ParallelKw
                        && token
                            .parent()
                            .is_some_and(|parent| parent.kind() == SyntaxKind::ParallelStmt))
                    || (kind == SyntaxKind::ProvideKw
                        && token
                            .parent()
                            .is_some_and(|parent| parent.kind() == SyntaxKind::ProvideStmt))
                    || (kind == SyntaxKind::RaceKw
                        && token.parent().is_some_and(|parent| {
                            parent.kind() == SyntaxKind::RaceStmt
                                && parent
                                    .parent()
                                    .is_none_or(|node| node.kind() != SyntaxKind::LetStmt)
                        }));
                if starts_statement && !line_start {
                    output.push('\n');
                    line_start = true;
                }
                if line_start {
                    push_indent(&mut output, indent);
                } else if needs_space(previous, kind) && !output.ends_with(' ') {
                    output.push(' ');
                }
                output.push_str(token.text());
                line_start = false;
            }
        }
        if !kind.is_trivia() {
            previous = Some(kind);
        }
    }

    while output.ends_with("\n\n") {
        output.pop();
    }
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn push_indent(output: &mut String, indent: usize) {
    for _ in 0..indent {
        output.push_str("    ");
    }
}

fn needs_space(previous: Option<SyntaxKind>, current: SyntaxKind) -> bool {
    !matches!(
        current,
        SyntaxKind::LParen
            | SyntaxKind::RParen
            | SyntaxKind::RBracket
            | SyntaxKind::Dot
            | SyntaxKind::Comma
            | SyntaxKind::Colon
            | SyntaxKind::Question
    ) && !matches!(previous, None | Some(SyntaxKind::LParen | SyntaxKind::Dot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modeling_constructs_keep_headers_and_source_boundaries_idempotently() {
        let source = "export pattern Message(body:String)=subset{\"type\":\"message\",body:body} guard healthy(actor:BrowserActor){when actor.console.error(event){fail event.message}} test \"chat\" cases[case \"one\"{body:\"hello\"},case \"two\"{body:\"☕\"}]as input{actor alice browser actor bob browser with guards[healthy(bob)]{let mark=checkpoint(bob) alice{open \"/\"}let received=select bob since mark within 5s{when websocket.received_json(event)if event.value matches Message(input.body){provide event.value}timeout{fail \"missing\"}}expect received matches Message(input.body)pass}}";
        let parsed = webtest_syntax::parse(source);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
        let formatted = format_file(&parsed);
        for boundary in [
            "export pattern Message(body: String)",
            "\n    case \"one\"",
            "\n    case \"two\"",
            "\n    actor alice browser\n    actor bob browser\n",
            "let received = select bob",
            "\n        alice {",
            "\n        pass\n",
        ] {
            assert!(formatted.contains(boundary), "{boundary}: {formatted}");
        }
        let reparsed = webtest_syntax::parse(&formatted);
        assert!(reparsed.errors().is_empty(), "{:?}", reparsed.errors());
        assert_eq!(format_file(&reparsed), formatted);
        let tokens = |parse: &Parse| {
            parse
                .syntax()
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .filter(|token| !token.kind().is_trivia())
                .map(|token| (token.kind(), token.text().to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(tokens(&parsed), tokens(&reparsed));
    }

    #[test]
    fn workspace_declarations_preserve_documentation_and_inline_headers() {
        let source = "import{seed as value}from \"./support\"\n/// helper docs\nexport fn normalize(value:String)->String=lower(value)\n/// fixture docs\nexport fixture email()->String scope test{setup server{provide \"ok\"}teardown server{expect 1==1}}";
        let expected = "import { seed as value } from \"./support\"\n/// helper docs\nexport fn normalize(value: String) -> String = lower(value)\n/// fixture docs\nexport fixture email() -> String scope test {\n    setup server {\n        provide \"ok\"\n    }\n    teardown server {\n        expect 1 == 1\n    }\n}\n";
        let formatted = format_file(&webtest_syntax::parse(source));
        assert_eq!(formatted, expected);
        assert!(webtest_syntax::parse(&formatted).errors().is_empty());
        assert_eq!(format_file(&webtest_syntax::parse(&formatted)), expected);
    }

    #[test]
    fn formats_parallel_and_contextual_names_without_losing_comments() {
        let source = "test \"é\"{let parallel=7 parallel{// siblings\nserver{expect parallel==7}timeout 1s{expect parallel==7}}}";
        let expected = "test \"é\" {\n    let parallel = 7\n    parallel {\n        // siblings\n        server {\n            expect parallel == 7\n        }\n        timeout 1s {\n            expect parallel == 7\n        }\n    }\n}\n";
        let formatted = format_file(&webtest_syntax::parse(source));
        assert_eq!(formatted, expected);
        assert!(webtest_syntax::parse(&formatted).errors().is_empty());
        assert_eq!(format_file(&webtest_syntax::parse(&formatted)), expected);
    }

    #[test]
    fn formats_and_preserves_comments() {
        let source = "test   \"x\"{// hello\nbrowser{open \"u\" evaluate \"init()\" click id ( \"x\" ) expect text ( \"done\" ) . visible}}";
        let expected = "test \"x\" {\n    // hello\n    browser {\n        open \"u\"\n        evaluate \"init()\"\n        click id(\"x\")\n        expect text(\"done\").visible\n    }\n}\n";
        let formatted = format_file(&webtest_syntax::parse(source));
        assert_eq!(formatted, expected);
        assert_eq!(format_file(&webtest_syntax::parse(&formatted)), expected);
    }

    #[test]
    fn formats_browser_calls_actions_and_deadlines_canonically() {
        let source = "test \"x\"{browser{fill role ( \"textbox\" ,name :\"Email\")with \"a\" wait id (\"ready\"). visible within 5s}}";
        let expected = "test \"x\" {\n    browser {\n        fill role(\"textbox\", name: \"Email\") with \"a\"\n        wait id(\"ready\").visible within 5s\n    }\n}\n";
        let formatted = format_file(&webtest_syntax::parse(source));
        assert_eq!(formatted, expected);
        assert_eq!(format_file(&webtest_syntax::parse(&formatted)), expected);
    }

    #[test]
    fn formats_typed_server_workflows_canonically() {
        let source = "test \"x\"{server{let response=http.post(\"/users\",json:{email:\"a\"})expect response.status==201 let user:{id:Int,email:String}=response.json}browser{fill label(\"Email\")with user.email}}";
        let expected = "test \"x\" {\n    server {\n        let response = http.post(\"/users\", json: { email: \"a\" })\n        expect response.status == 201\n        let user: { id: Int, email: String } = response.json\n    }\n    browser {\n        fill label(\"Email\") with user.email\n    }\n}\n";
        let formatted = format_file(&webtest_syntax::parse(source));
        assert_eq!(formatted, expected);
        assert_eq!(format_file(&webtest_syntax::parse(&formatted)), expected);
    }
}

#[cfg(test)]
mod race_tests {
    #[test]
    fn retry_headers_and_contextual_names_format_idempotently() {
        let source = "test \"x\"{let retry={backoff:1,max:2}retry 3 backoff 20ms max 1s{expect retry.max>retry.backoff}}";
        let formatted = super::format_file(&webtest_syntax::parse(source));
        assert!(
            formatted.contains("\n    retry 3 backoff 20ms max 1s {"),
            "{formatted}"
        );
        assert!(
            formatted.contains("retry.max > retry.backoff"),
            "{formatted}"
        );
        assert!(webtest_syntax::parse(&formatted).errors().is_empty());
        assert_eq!(
            super::format_file(&webtest_syntax::parse(&formatted)),
            formatted
        );
    }
    #[test]
    fn bound_race_and_contextual_names_format_losslessly_and_idempotently() {
        let source = "test \"x\" {let race=1 let provide=2 let selected=race{server{provide race}server{provide provide}}expect selected>0}";
        let formatted = super::format_file(&webtest_syntax::parse(source));
        assert!(formatted.contains("let selected = race {"), "{formatted}");
        assert!(formatted.contains("provide race"));
        assert!(webtest_syntax::parse(&formatted).errors().is_empty());
        assert_eq!(
            super::format_file(&webtest_syntax::parse(&formatted)),
            formatted
        );
    }
}
