use std::path::Path;

use ratatui::text::Span;

pub(crate) mod columns;
pub(crate) mod config;
pub(crate) mod dialog;
pub(crate) mod favorites;
pub(crate) mod filter;
pub(crate) mod finder;
pub(crate) mod graphics;
pub(crate) mod help;
pub(crate) mod icon;
pub(crate) mod infobar;
pub(crate) mod keybar;
pub(crate) mod pane;
pub(crate) mod panel;
pub(crate) mod preview;
pub(crate) mod prompt;
pub(crate) mod sort;
pub(crate) mod tab;
pub(crate) mod text_input;
pub(crate) mod toast;

/// The last component of `path`, or the whole path when it has none, which is
/// what the filesystem root is.
pub(crate) fn name_of(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => path.to_string_lossy().to_string(),
    }
}

/// `1-20 of 29` while part of a list is off screen, and nothing at all while
/// every row of it fits. One wording for every overlay that scrolls.
pub(crate) fn scroll_position(first: usize, shown: usize, total: usize) -> Option<String> {
    (shown < total).then(|| format!(" {}-{} of {total} ", first + 1, first + shown))
}

/// Breaks `message` into lines of at most `width` display columns, splitting
/// at spaces. A word too long to ever fit is split across lines by column.
/// Always returns at least one line.
pub(crate) fn wrap(message: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;

    for word in message.split_whitespace() {
        for chunk in split_word(word, width) {
            let chunk_width = Span::raw(chunk.as_str()).width();
            let space = if line.is_empty() { 0 } else { 1 };

            if !line.is_empty() && line_width + space + chunk_width > width {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            } else if space == 1 {
                line.push(' ');
                line_width += 1;
            }

            line.push_str(&chunk);
            line_width += chunk_width;
        }
    }

    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Cuts `word` into pieces of at most `width` display columns. A word that
/// already fits comes back whole.
fn split_word(word: &str, width: usize) -> Vec<String> {
    if Span::raw(word).width() <= width {
        return vec![word.to_string()];
    }

    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0;

    for character in word.chars() {
        let character_width = Span::raw(character.to_string()).width();
        if !chunk.is_empty() && chunk_width + character_width > width {
            chunks.push(std::mem::take(&mut chunk));
            chunk_width = 0;
        }
        chunk.push(character);
        chunk_width += character_width;
    }

    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

/// Clips text to `max` columns, counting display width rather than bytes, and
/// marks the cut with an ellipsis that is itself one column wide.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if Span::raw(text).width() <= max {
        return text.to_string();
    }

    let mut clipped = String::new();
    let mut width = 0;
    let mut buffer = [0u8; 4];
    for character in text.chars() {
        let character_width = Span::raw(&*character.encode_utf8(&mut buffer)).width();
        if width + character_width + 1 > max {
            break;
        }
        clipped.push(character);
        width += character_width;
    }

    clipped.push('…');
    clipped
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    #[test]
    fn a_message_that_fits_stays_on_one_line() {
        assert_eq!(wrap("Deleted 3 items", 20), ["Deleted 3 items"]);
    }

    #[test]
    fn wrapping_breaks_at_spaces() {
        assert_eq!(
            wrap("Deleted 3 of 5 items", 10),
            ["Deleted 3", "of 5 items"]
        );
    }

    #[test]
    fn a_word_longer_than_the_line_is_split() {
        assert_eq!(
            wrap("/very/long/path/to/a/file", 10),
            ["/very/long", "/path/to/a", "/file"]
        );
    }

    #[test]
    fn an_empty_message_still_yields_one_line() {
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn a_long_name_is_clipped_to_its_display_width() {
        let clipped = clip("a-very-long-file-name-indeed.txt", 12);
        assert_eq!(Span::raw(&clipped).width(), 12);
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn clipping_counts_columns_rather_than_characters() {
        // Every one of these is two columns wide, so only five fit in eleven
        // columns once the ellipsis has taken one.
        assert_eq!(clip("ああああああああ", 11), "あああああ…");
    }

    #[test]
    fn text_that_fits_is_left_alone() {
        assert_eq!(clip("short.txt", 12), "short.txt");
    }
}
