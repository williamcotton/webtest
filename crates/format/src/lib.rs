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
        match kind {
            SyntaxKind::Whitespace => {}
            SyntaxKind::LineComment => {
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
            SyntaxKind::RBrace => {
                if !line_start {
                    output.push('\n');
                }
                indent = indent.saturating_sub(1);
                push_indent(&mut output, indent);
                output.push('}');
                output.push('\n');
                line_start = true;
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
            _ => {
                let starts_statement = matches!(
                    kind,
                    SyntaxKind::TestKw
                        | SyntaxKind::BrowserKw
                        | SyntaxKind::OpenKw
                        | SyntaxKind::ClickKw
                );
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
    !matches!(current, SyntaxKind::LParen | SyntaxKind::RParen)
        && !matches!(previous, None | Some(SyntaxKind::LParen))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_preserves_comments() {
        let source = "test   \"x\"{// hello\nbrowser{open \"u\" click id ( \"x\" )}}";
        let expected = "test \"x\" {\n    // hello\n    browser {\n        open \"u\"\n        click id(\"x\")\n    }\n}\n";
        let formatted = format_file(&webtest_syntax::parse(source));
        assert_eq!(formatted, expected);
        assert_eq!(format_file(&webtest_syntax::parse(&formatted)), expected);
    }
}
