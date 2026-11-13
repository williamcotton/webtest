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
            b'/' => {
                offset += 1;
                SyntaxKind::Slash
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
            b'[' => {
                offset += 1;
                SyntaxKind::LBracket
            }
            b']' => {
                offset += 1;
                SyntaxKind::RBracket
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
            b'?' => {
                offset += 1;
                SyntaxKind::Question
            }
            b'=' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                SyntaxKind::EqEq
            }
            b'=' => {
                offset += 1;
                SyntaxKind::Equal
            }
            b'!' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                SyntaxKind::BangEq
            }
            b'!' => {
                offset += 1;
                SyntaxKind::Bang
            }
            b'<' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                SyntaxKind::LtEq
            }
            b'<' => {
                offset += 1;
                SyntaxKind::Lt
            }
            b'>' if bytes.get(offset + 1) == Some(&b'=') => {
                offset += 2;
                SyntaxKind::GtEq
            }
            b'>' => {
                offset += 1;
                SyntaxKind::Gt
            }
            b'+' => {
                offset += 1;
                SyntaxKind::Plus
            }
            b'-' if bytes.get(offset + 1) == Some(&b'>') => {
                offset += 2;
                SyntaxKind::Arrow
            }
            b'-' => {
                offset += 1;
                SyntaxKind::Minus
            }
            b'*' => {
                offset += 1;
                SyntaxKind::Star
            }
            b'&' if bytes.get(offset + 1) == Some(&b'&') => {
                offset += 2;
                SyntaxKind::AndAnd
            }
            b'|' if bytes.get(offset + 1) == Some(&b'|') => {
                offset += 2;
                SyntaxKind::OrOr
            }
            byte if byte.is_ascii_digit() => {
                offset += 1;
                while offset < bytes.len() && bytes[offset].is_ascii_digit() {
                    offset += 1;
                }
                let mut kind = SyntaxKind::Int;
                if bytes.get(offset) == Some(&b'.')
                    && bytes.get(offset + 1).is_some_and(u8::is_ascii_digit)
                {
                    offset += 1;
                    while offset < bytes.len() && bytes[offset].is_ascii_digit() {
                        offset += 1;
                    }
                    kind = SyntaxKind::Float;
                }
                if bytes.get(offset) == Some(&b'm') && bytes.get(offset + 1) == Some(&b's') {
                    offset += 2;
                    SyntaxKind::Duration
                } else if matches!(bytes.get(offset), Some(b's' | b'm')) {
                    offset += 1;
                    SyntaxKind::Duration
                } else {
                    kind
                }
            }
            _ if source[offset..].chars().next().is_some_and(is_ident_start) => {
                offset += source[offset..].chars().next().map_or(1, char::len_utf8);
                while let Some(character) = source[offset..].chars().next() {
                    if !is_ident_continue(character)
                        || (character == '-' && bytes.get(offset + 1) == Some(&b'>'))
                    {
                        break;
                    }
                    offset += character.len_utf8();
                }
                match &source[start..offset] {
                    "test" => SyntaxKind::TestKw,
                    "import" => SyntaxKind::ImportKw,
                    "export" => SyntaxKind::ExportKw,
                    "from" => SyntaxKind::FromKw,
                    "as" => SyntaxKind::AsKw,
                    "fn" => SyntaxKind::FnKw,
                    "fixture" => SyntaxKind::FixtureKw,
                    "scope" => SyntaxKind::ScopeKw,
                    "file" => SyntaxKind::FileKw,
                    "worker" => SyntaxKind::WorkerKw,
                    "suite" => SyntaxKind::SuiteKw,
                    "setup" => SyntaxKind::SetupKw,
                    "teardown" => SyntaxKind::TeardownKw,
                    "use" => SyntaxKind::UseKw,
                    "shareable" => SyntaxKind::ShareableKw,

                    "browser" => SyntaxKind::BrowserKw,
                    "server" => SyntaxKind::ServerKw,
                    "timeout" => SyntaxKind::TimeoutKw,
                    "parallel" => SyntaxKind::ParallelKw,
                    "race" => SyntaxKind::RaceKw,
                    "provide" => SyntaxKind::ProvideKw,
                    "retry" => SyntaxKind::RetryKw,
                    "backoff" => SyntaxKind::BackoffKw,
                    "max" => SyntaxKind::MaxKw,
                    "let" => SyntaxKind::LetKw,
                    "true" => SyntaxKind::TrueKw,
                    "false" => SyntaxKind::FalseKw,
                    "null" => SyntaxKind::NullKw,
                    "contains" => SyntaxKind::ContainsKw,
                    "matches" => SyntaxKind::MatchesKw,
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

fn is_ident_start(character: char) -> bool {
    unicode_ident::is_xid_start(character) || character == '_'
}
fn is_ident_continue(character: char) -> bool {
    unicode_ident::is_xid_continue(character) || character == '-'
}

/// Validates names using exactly the lexer and contextual-name contract.
pub fn is_valid_identifier(name: &str) -> bool {
    let tokens = lex(name);
    tokens.len() == 1 && crate::is_name_kind(tokens[0].kind)
}

pub fn is_valid_binding_identifier(name: &str) -> bool {
    let tokens = lex(name);
    tokens.len() == 1 && crate::is_binding_name_kind(tokens[0].kind)
}

pub fn keyword_kind(text: &str) -> Option<SyntaxKind> {
    let tokens = lex(text);
    (tokens.len() == 1 && tokens[0].kind.is_keyword()).then(|| tokens[0].kind)
}
