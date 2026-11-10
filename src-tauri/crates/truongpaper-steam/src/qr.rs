//! ASCII half-block QR code (as printed by `DepotDownloader -qr`) to boolean module matrix.

/// Prompt line printed by `DepotDownloader` right before the QR block.
pub const QR_HEADER: &str = "Use the Steam Mobile App to sign in with this QR code";

/// Returns true when the line is non-empty and every char is one of `█ ▀ ▄ ' '`.
/// An all-space line counts: it is a light quiet-zone row inside the QR block.
pub fn is_qr_line(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| matches!(c, '█' | '▀' | '▄' | ' '))
}

/// Converts Unicode half-block lines (`█`, `▀`, `▄`, `' '`) into a module matrix.
/// Each text line yields two matrix rows (top half, bottom half). `true` = dark module.
pub fn parse_ascii_qr_to_matrix(lines: &[String]) -> Vec<Vec<bool>> {
    let mut matrix = Vec::with_capacity(lines.len() * 2);
    for line in lines {
        let mut top_row = Vec::new();
        let mut bot_row = Vec::new();
        for ch in line.chars() {
            match ch {
                '█' => {
                    top_row.push(true);
                    bot_row.push(true);
                }
                '▀' => {
                    top_row.push(true);
                    bot_row.push(false);
                }
                '▄' => {
                    top_row.push(false);
                    bot_row.push(true);
                }
                _ => {
                    top_row.push(false);
                    bot_row.push(false);
                }
            }
        }
        matrix.push(top_row);
        matrix.push(bot_row);
    }
    matrix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_small_qr_block() {
        let lines = vec!["█▀▄ ".to_string(), " ▄▀█".to_string()];
        let m = parse_ascii_qr_to_matrix(&lines);
        assert_eq!(m.len(), 4);
        assert_eq!(m[0], vec![true, true, false, false]);
        assert_eq!(m[1], vec![true, false, true, false]);
        assert_eq!(m[2], vec![false, false, true, true]);
        assert_eq!(m[3], vec![false, true, false, true]);
    }

    #[test]
    fn empty_input_yields_empty_matrix() {
        assert!(parse_ascii_qr_to_matrix(&[]).is_empty());
    }

    #[test]
    fn unknown_chars_are_light_modules() {
        let m = parse_ascii_qr_to_matrix(&["x█".to_string()]);
        assert_eq!(m[0], vec![false, true]);
        assert_eq!(m[1], vec![false, true]);
    }

    #[test]
    fn qr_line_detection() {
        assert!(is_qr_line("██ ▀▄"));
        assert!(!is_qr_line(""));
        assert!(is_qr_line("    "));
        assert!(!is_qr_line("██ abc"));
        assert!(!is_qr_line(
            "Use the Steam Mobile App to sign in with this QR code:"
        ));
    }
}
