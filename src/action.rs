use crate::{
    fs::ops::TransferOp,
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
    Open(Opener),
    ToggleMark,
    ToggleTab(ToggleDirection),
    MarkAndMove { op: MarkOp, nav_dir: VerticalDir },
    ClearMarks,
    Transfer { op: TransferOp },
    Delete,
    Rename,
    CreateEntry,
    NewTab,
    CloseTab,
    RenameTab,
    Find,
    ToggleQuickView,
    CycleColumns,
}
