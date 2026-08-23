use crate::{
    fs::ops::TransferOp,
    ui::tab::{MarkOp, ToggleDirection},
};

/// Which way the cursor steps through a pane listing.
#[derive(Clone, Copy, Debug)]
pub enum VerticalDir {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    QuitAnyway,
    CancelJob,
    ToggleSide,
    MoveCursor(VerticalDir),
    OpenSelected,
    ToggleMark,
    ToggleTab(ToggleDirection),
    MarkAndMove { op: MarkOp, nav_dir: VerticalDir },
    ClearMarks,
    Transfer { op: TransferOp },
    Delete,
    Rename,
    CreateEntry,
    NewTab,
    RenameTab,
    Find,
    DeleteMarked,
    ShowHelp,
}
