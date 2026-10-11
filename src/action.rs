use crate::{
    fs::ops::{DeleteMode, TransferOp},
    open::Opener,
    ui::tab::{MarkOp, ToggleDirection},
};

/// Which way the cursor steps through a pane listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerticalDir {
    Up,
    Down,
}

/// Which end of a pane listing the cursor lands on, wherever it was before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListEnd {
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    CancelJob,
    ToggleSide,
    /// Trades the two panels' places, tabs, marks and all. The focus moves
    /// with the panel it was on.
    SwapPanels,
    /// Sends the other panel to the directory the focused one is in. The
    /// focus stays where it is.
    MirrorPanel,
    MoveCursor(VerticalDir),
    MoveCursorTo(ListEnd),
    /// Moves the cursor by as many items as the panel last drew.
    MoveCursorPage(VerticalDir),
    OpenSelected,
    GoToParent,
    /// Opens the prompt for a path to send the focused panel to.
    Jump,
    Open(Opener),
    ToggleMark,
    ToggleTab(ToggleDirection),
    MarkAndMove {
        op: MarkOp,
        nav_dir: VerticalDir,
    },
    /// Applies the op to every item the focused panel is showing.
    MarkVisible(MarkOp),
    /// Opens the prompt for a pattern, to apply the op to the items shown
    /// whose names match it.
    MarkMatching(MarkOp),
    Clear,
    Transfer {
        op: TransferOp,
    },
    /// Makes a symlink to every marked item in the other panel, under the
    /// item's own name. Nothing is asked: a name already taken fails its item.
    Link,
    /// Puts the path under the cursor, or with marks every marked one, on the
    /// system clipboard. The marks are left standing.
    CopyPath,
    /// Compares two files byte by byte: the two marked, or with nothing
    /// marked the two under the cursors. The answer comes as a toast.
    Compare,
    /// Opens the prompt for the archive's name. What format it is written in
    /// follows from that name, so only the asking half is an action here too.
    Pack,
    /// Unpacks every marked archive into the other panel. Nothing is asked:
    /// each one goes into a directory of its own and nothing is overwritten.
    Unpack,
    /// Opens the confirmation for one of the two deletions. Which one is
    /// settled by the key, since only the asking half is an `Action`.
    Delete(DeleteMode),
    Rename,
    /// Hands the names of every marked item to `$EDITOR`, one per line, and
    /// asks before renaming whatever came back changed.
    RenameMarked,
    CreateEntry,
    NewTab,
    CloseTab,
    RenameTab,
    Find,
    Filter,
    OpenFavorites,
    /// Puts the directory the focused panel is in on the list of favorites.
    /// Nothing is asked: the panel is already standing in the answer.
    AddFavorite,
    /// Opens the settings that are kept in the config file.
    OpenConfig,
    TogglePreview,
    CycleColumns,
    ToggleDotFiles,
    CycleSort,
    ReverseSort,
}
