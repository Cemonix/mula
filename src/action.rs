use std::{path::Path, rc::Rc};

use crate::fs::ops::{MutationOp, TransferOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn toggle(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
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

/// What a mark action does to the item under the cursor.
#[derive(Clone, Copy, Debug)]
pub enum MarkOp {
    Toggle,
    Mark,
    Unmark,
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Quit,
    ToggleSide,
    MoveCursor(NavDirection),
    OpenSelected,
    ToggleMark,
    ToggleTab(ToggleDirection),
    MarkAndMove { op: MarkOp, nav_dir: NavDirection },
    ClearMarks,
    Transfer { op: TransferOp },
    Delete,
    Rename,
    CreateEntry,
    NewTab,
    RenameTab,
    DeleteMarked,
    ShowHelp,
    None,
}

/// What an `Input` prompt still needs a typed name for.
#[derive(Debug, Clone)]
pub enum InputPurpose {
    Rename(Rc<Path>),
    CreateEntry(Rc<Path>),
}

/// What happens once an `Input` prompt is confirmed: either a filesystem
/// mutation, or the title of the tab that was open for renaming.
#[derive(Debug, Clone)]
pub enum InputTarget {
    Mutation(InputPurpose),
    RenameTab,
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
            InputPurpose::CreateEntry(parent) => match name.strip_suffix('/') {
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
