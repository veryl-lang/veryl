use ropey::Rope;

/// Converts a UTF-8 byte offset to a zero-based LSP UTF-16 position.
pub fn byte_position(text: &Rope, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len_bytes());
    let line = text.byte_to_line(offset);
    let index = text.byte_to_char(offset);
    let column = text
        .slice(text.line_to_char(line)..index)
        .chars()
        .take_while(|c| !matches!(c, '\r' | '\n'))
        .map(char::len_utf16)
        .sum::<usize>();
    (line as u32, column as u32)
}

/// Converts an LSP UTF-16 column to a one-based parser scalar column.
/// Out-of-line columns clamp to the line end; split surrogate pairs clamp back.
pub fn parser_column(text: &Rope, line: usize, column: usize) -> usize {
    let Some(line) = text.get_line(line) else {
        return 1;
    };
    let mut units = 0;
    let mut scalars = 0;
    for c in line.chars().take_while(|c| !matches!(c, '\r' | '\n')) {
        if units + c.len_utf16() > column {
            break;
        }
        units += c.len_utf16();
        scalars += 1;
    }
    scalars + 1
}

/// Converts a one-based scalar column to an LSP UTF-16 column.
pub fn utf16_column(text: &str, column: usize) -> u32 {
    text.chars()
        .take(column.saturating_sub(1))
        .take_while(|c| !matches!(c, '\r' | '\n'))
        .map(char::len_utf16)
        .sum::<usize>() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn astral_prefix_round_trips() {
        let text = Rope::from_str("a😀é\n");
        assert_eq!(byte_position(&text, 7), (0, 4));
        assert_eq!(parser_column(&text, 0, 4), 4);
        assert_eq!(utf16_column("a😀é", 4), 4);
    }
    #[test]
    fn split_surrogate_clamps_back() {
        assert_eq!(parser_column(&Rope::from_str("😀x"), 0, 1), 1);
    }
}
