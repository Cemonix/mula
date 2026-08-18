use crate::fs::ops::TransferOp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub enum NavDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
pub enum ToggleDirection {
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    FocusSide(Side),
    MoveCursor(NavDirection),
    OpenSelected,
    ToggleMark,
    ToggleTab(ToggleDirection),
    Transfer { op: TransferOp },
    Delete,
    NewTab,
    DeleteMarked,
    ShowHelp,
    None,
}
