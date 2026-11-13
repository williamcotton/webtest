//! Transactional LSP UTF-16 synchronization. No partial edit mutates the input.
use tower_lsp_server::ls_types::{Position, TextDocumentContentChangeEvent};
pub(super) fn offset(text: &str, position: Position) -> Option<usize> {
    let mut start = 0;
    let mut lines = text.split('\n');
    for _ in 0..position.line {
        start += lines.next()?.len() + 1;
    }
    let line = lines.next()?;
    let line = if start + line.len() < text.len() {
        line.strip_suffix('\r').unwrap_or(line)
    } else {
        line
    };
    let mut column = 0;
    for (byte, character) in line.char_indices() {
        if column == position.character {
            return Some(start + byte);
        }
        column += character.len_utf16() as u32;
        if column > position.character {
            return None;
        }
    }
    (column == position.character).then_some(start + line.len())
}
pub(super) fn changes(
    source: &str,
    changes: &[TextDocumentContentChangeEvent],
) -> Result<String, &'static str> {
    if changes.is_empty() {
        return Err("empty change batch");
    }
    let mut text = source.to_owned();
    for change in changes {
        if let Some(range) = change.range {
            let start = offset(&text, range.start).ok_or("invalid UTF-16 start position")?;
            let end = offset(&text, range.end).ok_or("invalid UTF-16 end position")?;
            if start > end {
                return Err("reversed edit range");
            }
            if change
                .range_length
                .is_some_and(|length| text[start..end].encode_utf16().count() != length as usize)
            {
                return Err("rangeLength does not match the replaced text");
            }
            text.replace_range(start..end, &change.text);
        } else {
            if change.range_length.is_some() {
                return Err("rangeLength requires a range");
            }
            text = change.text.clone();
        }
    }
    Ok(text)
}
#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Range;
    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(
                Position::new(start.0, start.1),
                Position::new(end.0, end.1),
            )),
            range_length: None,
            text: text.into(),
        }
    }
    #[test]
    fn unicode_crlf_and_sequential_edits_use_the_current_intermediate_text() {
        let source = "a😀é\r\nβ\n";
        assert_eq!(offset(source, Position::new(0, 2)), None);
        assert_eq!(offset(source, Position::new(0, 4)), Some("a😀é".len()));
        assert_eq!(offset(source, Position::new(0, 5)), None);
        assert_eq!(offset(source, Position::new(2, 0)), Some(source.len()));
        assert_eq!(offset(source, Position::new(3, 0)), None);
        assert_eq!(
            changes(
                source,
                &[
                    edit((0, 1), (0, 3), "ü"),
                    edit((0, 2), (0, 3), "語"),
                    edit((1, 0), (2, 0), "done")
                ]
            )
            .unwrap(),
            "aü語\r\ndone"
        );
    }
    #[test]
    fn invalid_batch_is_transactional_and_full_replacement_recovers() {
        let source = "é😀\n";
        assert!(
            changes(
                source,
                &[edit((0, 0), (0, 1), "x"), edit((0, 2), (0, 3), "bad")]
            )
            .is_err()
        );
        assert_eq!(source, "é😀\n");
        assert!(changes(source, &[edit((0, 3), (0, 0), "")]).is_err());
        assert!(changes(source, &[edit((9, 0), (9, 0), "")]).is_err());
        let replacement = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "fixed".into(),
        };
        assert_eq!(changes(source, &[replacement]).unwrap(), "fixed");
    }
}
