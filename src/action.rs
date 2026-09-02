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
    ClearMarks,
    Transfer {
        op: TransferOp,
    },
    /// Opens the confirmation for one of the two deletions. Which one is
    /// settled by the key, since only the asking half is an `Action`.
    Delete(DeleteMode),
    Rename,
    CreateEntry,
    NewTab,
    CloseTab,
    RenameTab,
    Find,
    ToggleQuickView,
    CycleColumns,
}
