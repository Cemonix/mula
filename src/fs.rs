use std::{io, path::Path, rc::Rc};

#[derive(PartialEq, Debug)]
pub enum DirEntryKind {
    Parent,
    Directory,
    File,
}

#[derive(Debug)]
pub struct DirEntry {
    pub path: Rc<Path>,
    pub kind: DirEntryKind,
}

pub fn list_dir_entries(dir: &Path) -> Result<Vec<DirEntry>, io::Error> {
    let mut entries = Vec::new();

    if let Some(parent) = dir.parent() {
        entries.push(DirEntry {
            path: Rc::from(parent),
            kind: DirEntryKind::Parent,
        });
    }

    for entry in std::fs::read_dir(dir)? {
        let path: Rc<Path> = Rc::from(entry?.path());
        let kind = if path.is_dir() {
            DirEntryKind::Directory
        } else {
            DirEntryKind::File
        };
        entries.push(DirEntry { path, kind });
    }

    Ok(entries)
}
