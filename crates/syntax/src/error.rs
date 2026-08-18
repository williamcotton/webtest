use webtest_text::TextRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub code: &'static str,
    pub message: String,
}

impl SyntaxError {
    pub(crate) fn new(range: TextRange, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            range,
            code,
            message: message.into(),
        }
    }
}
