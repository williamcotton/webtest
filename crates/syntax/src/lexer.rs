use crate::SyntaxKind;
use webtest_text::{TextRange, TextSize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: SyntaxKind,
    pub range: TextRange,
    pub error: Option<(&'static str, &'static str)>,
}

pub(crate) fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut offset = 0usize;

    while offset < bytes.len() {
        let start = offset;
        let mut token_error = None;
        let kind = match bytes[offset] {
            byte if byte.is_ascii_whitespace() => {
                offset += 1;
                while offset < bytes.len() && bytes[offset].is_ascii_whitespace() {
                    offset += 1;
                }
                SyntaxKind::Whitespace
            }
            b'/' if bytes.get(offset + 1) == Some(&b'/') => {
                offset += 2;
                while offset < bytes.len() && bytes[offset] != b'\n' {
                    offset += 1;
                }
                SyntaxKind::LineComment
            }
            b'"' => {
                offset += 1;
                let mut escaped = false;
                let mut terminated = false;
                while offset < bytes.len() {
                    let current = bytes[offset];
                    offset += 1;
                    if escaped {
                        escaped = false;
                    } else if current == b'\\' {
                        escaped = true;
                    } else if current == b'"' {
                        terminated = true;
                        break;
                    }
                }
                if !terminated {
                    token_error =
                        Some(("syntax.unterminated_string", "unterminated string literal"));
                }
                SyntaxKind::String
            }
            b'{' => {
                offset += 1;
                SyntaxKind::LBrace
            }
            b'}' => {
                offset += 1;
                SyntaxKind::RBrace
            }
            b'(' => {
                offset += 1;
                SyntaxKind::LParen
            }
            b')' => {
                offset += 1;
                SyntaxKind::RParen
            }
            b'.' => {
                offset += 1;
                SyntaxKind::Dot
            }
            b',' => {
                offset += 1;
                SyntaxKind::Comma
            }
            b':' => {
                offset += 1;
                SyntaxKind::Colon
            }
            byte if byte.is_ascii_digit() => {
                offset += 1;
                while offset < bytes.len() && bytes[offset].is_ascii_alphanumeric() {
                    offset += 1;
                }
                SyntaxKind::Duration
            }
            byte if is_ident_start(byte) => {
                offset += 1;
                while offset < bytes.len() && is_ident_continue(bytes[offset]) {
                    offset += 1;
                }
                match &source[start..offset] {
                    "test" => SyntaxKind::TestKw,
                    "browser" => SyntaxKind::BrowserKw,
                    "open" => SyntaxKind::OpenKw,
                    "evaluate" => SyntaxKind::EvaluateKw,
                    "click" => SyntaxKind::ClickKw,
                    "fill" => SyntaxKind::FillKw,
                    "type" => SyntaxKind::TypeKw,
                    "press" => SyntaxKind::PressKw,
                    "key" => SyntaxKind::KeyKw,
                    "with" => SyntaxKind::WithKw,
                    "check" => SyntaxKind::CheckKw,
                    "uncheck" => SyntaxKind::UncheckKw,
                    "select" => SyntaxKind::SelectKw,
                    "option" => SyntaxKind::OptionKw,
                    "hover" => SyntaxKind::HoverKw,
                    "wait" => SyntaxKind::WaitKw,
                    "expect" => SyntaxKind::ExpectKw,
                    "within" => SyntaxKind::WithinKw,
                    "url" => SyntaxKind::UrlKw,
                    "id" => SyntaxKind::IdKw,
                    "role" => SyntaxKind::RoleKw,
                    "name" => SyntaxKind::NameKw,
                    "label" => SyntaxKind::LabelKw,
                    "text" => SyntaxKind::TextKw,
                    "placeholder" => SyntaxKind::PlaceholderKw,
                    "test_id" => SyntaxKind::TestIdKw,
                    "css" => SyntaxKind::CssKw,
                    "xpath" => SyntaxKind::XPathKw,
                    "visible" => SyntaxKind::VisibleKw,
                    "hidden" => SyntaxKind::HiddenKw,
                    "attached" => SyntaxKind::AttachedKw,
                    "detached" => SyntaxKind::DetachedKw,
                    "enabled" => SyntaxKind::EnabledKw,
                    "disabled" => SyntaxKind::DisabledKw,
                    "checked" => SyntaxKind::CheckedKw,
                    "unchecked" => SyntaxKind::UncheckedKw,
                    _ => SyntaxKind::Ident,
                }
            }
            _ => {
                let character_len = source[offset..].chars().next().map_or(1, char::len_utf8);
                offset += character_len;
                SyntaxKind::Error
            }
        };

        tokens.push(Token {
            kind,
            range: TextRange::new(TextSize::from(start as u32), TextSize::from(offset as u32)),
            error: token_error,
        });
    }

    tokens
}

const fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

const fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit() || byte == b'-'
}
