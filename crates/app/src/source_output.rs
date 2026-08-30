use webtest_text::TextRange;

use crate::report::{ByteRangeReport, MachineSourceReport, SourceSpanReport};

pub(crate) fn source_span(source: &str, range: TextRange) -> SourceSpanReport {
    let (line, column, line_text, width) = line_details(source, range);
    let end_offset =
        floor_char_boundary(source, (u32::from(range.end()) as usize).min(source.len()));
    let (end_line, end_column) = offset_line_column(source, end_offset);
    SourceSpanReport {
        line: line + 1,
        column: column + 1,
        source_line: line_text.into(),
        underline_start: column,
        underline_width: width.max(1),
        end_line: end_line + 1,
        end_column: end_column + 1,
        byte_start: range.start().into(),
        byte_end: range.end().into(),
    }
}

pub(crate) fn machine_source(
    path: &str,
    source_revision: &str,
    span: &SourceSpanReport,
) -> MachineSourceReport {
    MachineSourceReport {
        path: path.into(),
        source_revision: source_revision.into(),
        byte_range: ByteRangeReport {
            start: span.byte_start,
            end: span.byte_end,
        },
        start_line: span.line,
        start_column: span.column,
        end_line: span.end_line,
        end_column: span.end_column,
    }
}

fn offset_line_column(source: &str, offset: usize) -> (usize, usize) {
    let line_start = source[..offset]
        .rfind('\n')
        .map_or(0, |line_break| line_break + 1);
    let line = source[..line_start]
        .chars()
        .filter(|character| *character == '\n')
        .count();
    let column = source[line_start..offset].chars().count();
    (line, column)
}

fn line_details(source: &str, range: TextRange) -> (usize, usize, &str, usize) {
    let requested_start = u32::from(range.start()) as usize;
    let requested_end = u32::from(range.end()) as usize;
    let start = floor_char_boundary(source, requested_start.min(source.len()));
    let end = floor_char_boundary(source, requested_end.min(source.len()));
    let line_start = source[..start].rfind('\n').map_or(0, |offset| offset + 1);
    let line_end = source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset);
    let line = source[..line_start]
        .chars()
        .filter(|character| *character == '\n')
        .count();
    let column = source[line_start..start].chars().count();
    let underline_end = end.min(line_end);
    let width = source[start..underline_end].chars().count();
    (line, column, &source[line_start..line_end], width)
}

fn floor_char_boundary(source: &str, mut offset: usize) -> usize {
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use webtest_text::{TextRange, TextSize};

    use super::*;

    #[test]
    fn source_locations_handle_unicode() {
        let source = "😀\nclick id(\"x\")";
        let start = source.find("id").expect("locator");
        let range = TextRange::new(
            TextSize::from(start as u32),
            TextSize::from((start + 7) as u32),
        );
        let span = source_span(source, range);
        assert_eq!(
            (
                span.line,
                span.column,
                span.source_line.as_str(),
                span.underline_width
            ),
            (2, 7, "click id(\"x\")", 7)
        );
    }

    #[test]
    fn multiline_zero_width_and_out_of_bounds_ranges_are_safe() {
        let source = "first\nsecond";
        let zero = source_span(source, TextRange::new(0.into(), 0.into()));
        assert_eq!((zero.line, zero.column, zero.underline_width), (1, 1, 1));

        let multiline = source_span(source, TextRange::new(3.into(), 9.into()));
        assert_eq!((multiline.line, multiline.end_line), (1, 2));
        assert_eq!(multiline.underline_width, 2);

        let outside = source_span(source, TextRange::new(100.into(), 120.into()));
        assert_eq!(
            (outside.line, outside.column, outside.underline_width),
            (2, 7, 1)
        );
    }

    #[test]
    fn non_boundary_offsets_floor_to_a_valid_character() {
        let source = "éx";
        let span = source_span(source, TextRange::new(1.into(), 2.into()));
        assert_eq!((span.column, span.underline_width), (1, 1));
    }
}
