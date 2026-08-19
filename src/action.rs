use std::{path::Path, rc::Rc};

use crate::fs::ops::{MutationOp, TransferOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub enum NavDirection {
    Up,
    Down,
    Left,
    Right,
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
    MarkAndMove(NavDirection),
    ClearMarks,
    Transfer { op: TransferOp },
    Delete,
    Rename,
    New,
    NewTab,
    DeleteMarked,
    ShowHelp,
    None,
}

/// What an `Input` prompt still needs a typed name for.
#[derive(Debug, Clone)]
pub enum InputPurpose {
    Rename(Rc<Path>),
    New(Rc<Path>),
}

impl InputPurpose {
    /// A name ending in `/` creates a directory; anything else creates a
    /// file. Either may carry intermediate directories that do not exist
    /// yet (`fol/fol2/fol3/file1.txt`), which the op creates along the way.
    pub fn op(self, name: String) -> MutationOp {
        match self {
            InputPurpose::Rename(target) => MutationOp::Rename {
                path: target.to_path_buf(),
                new_name: name,
            },
            InputPurpose::New(parent) => match name.strip_suffix('/') {
                Some(dirs) => MutationOp::CreateDir {
                    parent: parent.to_path_buf(),
                    name: dirs.to_string(),
                },
                None => MutationOp::CreateFile {
                    parent: parent.to_path_buf(),
                    name,
                },
            },
        }
    }
}
