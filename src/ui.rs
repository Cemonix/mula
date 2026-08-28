use std::path::Path;

pub(crate) mod dialog;
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
