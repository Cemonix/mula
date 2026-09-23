use crate::{
    fs::ops::{DeleteMode, TransferOp},
    open::Opener,
    ui::tab::{MarkOp, ToggleDirection},
};

/// Which way the cursor steps through a pane listing.
#[derive(Clone, Copy, Debug)]
pub enum VerticalDir {
    Up,
    Down,
}

/// Which end of a pane listing the cursor lands on, wherever it was before.
#[derive(Clone, Copy, Debug)]
pub enum ListEnd {
    First,
    Last,
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    CancelJob,
    ToggleSide,
    MoveCursor(VerticalDir),
    MoveCursorTo(ListEnd),
    OpenSelected,
    GoToParent,
    Open(Opener),
    ToggleMark,
    ToggleTab(ToggleDirection),
    MarkAndMove {
        op: MarkOp,
        nav_dir: VerticalDir,
    },
    MarkAll,
    Clear,
    Transfer {
        op: TransferOp,
    },
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
    TogglePreview,
    CycleColumns,
    ToggleDotFiles,
    CycleSort,
    ReverseSort,
}
