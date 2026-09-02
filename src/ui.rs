use std::path::Path;

use ratatui::text::Span;

pub(crate) mod columns;
pub(crate) mod dialog;
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
