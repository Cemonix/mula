use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub enum TransferOp {
    Copy,
    Move,
}

impl TransferOp {
    pub fn execute(&self, from: &Path, to: &Path) -> io::Result<()> {
        ensure_destination_outside_source(from, to)?;

        if to.symlink_metadata().is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", to.display()),
            ));
        }

        match self {
            TransferOp::Copy => copy_recursive(from, to).inspect_err(|_| remove_partial(to)),
            TransferOp::Move => match fs::rename(from, to) {
                // rename(2) cannot cross filesystems. Fall back to a full copy,
                // and only delete the source once that copy has fully succeeded.
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                    copy_recursive(from, to).inspect_err(|_| remove_partial(to))?;
                    remove_recursive(from)
                }
                other => other,
            },
        }
    }
}

#[derive(Debug)]
pub enum MutationOp {
    Delete { path: PathBuf },
    Rename { path: PathBuf, new_name: String },
    CreateDir { parent: PathBuf, name: String },
}

impl MutationOp {
    pub fn execute(&self) -> io::Result<()> {
        match self {
            MutationOp::Delete { path } => remove_recursive(path),
            MutationOp::Rename { path, new_name } => {
                fs::rename(path, path.with_file_name(new_name))
            }
            MutationOp::CreateDir { parent, name } => fs::create_dir(parent.join(name)),
        }
    }
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    // symlink_metadata does not follow links, which is what stops a symlink
    // loop from being walked into.
    let file_type = from.symlink_metadata()?.file_type();

    if file_type.is_symlink() {
        copy_symlink(from, to)
    } else if file_type.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ()) // copy returns u64, normalize to ()
    }
}

/// Recreates a symlink at `to` pointing wherever `from` pointed, rather than
/// materialising a copy of whatever it resolved to.
#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_symlink(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copying symlinks is only implemented on unix",
    ))
}

fn remove_partial(to: &Path) {
    if to.symlink_metadata().is_err() {
        return;
    }
    if let Err(e) = remove_recursive(to) {
        tracing::error!(path = ?to, error = %e, "could not clean up partial transfer");
    }
}

fn remove_recursive(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Rejects copying a directory into its own subtree, which would otherwise
/// recurse into what it is writing until the path length limit is hit.
fn ensure_destination_outside_source(from: &Path, to: &Path) -> io::Result<()> {
    let invalid = |msg: String| io::Error::new(io::ErrorKind::InvalidInput, msg);

    let parent = to
        .parent()
        .ok_or_else(|| invalid(format!("{} has no parent directory", to.display())))?;
    let file_name = to
        .file_name()
        .ok_or_else(|| invalid(format!("{} has no file name", to.display())))?;

    // `to` does not exist yet, so only its parent can be canonicalised.
    // Resolving both sides is what stops the check being sidestepped via a
    // symlink or a `..` component.
    let from = from.canonicalize()?;
    let to = parent.canonicalize()?.join(file_name);

    if to.starts_with(&from) {
        return Err(invalid(format!(
            "cannot transfer {} into itself ({})",
            from.display(),
            to.display()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod ops_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "mula-ops-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn at(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }

        fn make_dir(&self, path: &Path) {
            fs::create_dir_all(path).unwrap();
        }

        fn make_file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.at(relative);
            self.make_dir(path.parent().unwrap());
            fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copies_a_directory_tree_preserving_structure() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");
        t.make_file("src/sub/b.txt", "b");
        t.make_file("src/sub/deep/c.txt", "c");

        TransferOp::Copy
            .execute(&t.at("src"), &t.at("dest"))
            .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(t.at("dest/sub/b.txt")).unwrap(), "b");
        assert_eq!(
            fs::read_to_string(t.at("dest/sub/deep/c.txt")).unwrap(),
            "c"
        );
        assert!(t.at("src/a.txt").exists(), "source must be left alone");
    }

    #[test]
    fn refuses_to_overwrite_an_existing_destination() {
        let t = TempTree::new();
        t.make_file("src.txt", "new");
        t.make_file("dest.txt", "original");

        let err = TransferOp::Copy
            .execute(&t.at("src.txt"), &t.at("dest.txt"))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(t.at("dest.txt")).unwrap(),
            "original",
            "existing file must not be clobbered"
        );
    }

    #[test]
    fn refuses_to_copy_a_directory_into_its_own_subtree() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");

        let err = TransferOp::Copy
            .execute(&t.at("src"), &t.at("src/nested"))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!t.at("src/nested").exists());
    }

    #[test]
    fn moves_a_tree_within_one_filesystem() {
        let t = TempTree::new();
        t.make_file("src/sub/a.txt", "a");

        TransferOp::Move
            .execute(&t.at("src"), &t.at("dest"))
            .unwrap();

        assert!(!t.at("src").exists(), "source must be gone after a move");
        assert_eq!(fs::read_to_string(t.at("dest/sub/a.txt")).unwrap(), "a");
    }

    #[cfg(unix)]
    #[test]
    fn recreates_symlinks_instead_of_following_them() {
        let t = TempTree::new();
        t.make_file("src/real.txt", "real");
        std::os::unix::fs::symlink("real.txt", t.at("src/link.txt")).unwrap();

        TransferOp::Copy
            .execute(&t.at("src"), &t.at("dest"))
            .unwrap();

        let copied = t.at("dest/link.txt");
        assert!(
            copied.symlink_metadata().unwrap().file_type().is_symlink(),
            "link must stay a link, not become a copy of its target"
        );
        assert_eq!(fs::read_link(&copied).unwrap(), Path::new("real.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn does_not_walk_into_a_symlink_loop() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");
        // Points back above `src`, so following it would recurse forever.
        std::os::unix::fs::symlink("..", t.at("src/loop")).unwrap();

        TransferOp::Copy
            .execute(&t.at("src"), &t.at("dest"))
            .unwrap();

        assert!(
            t.at("dest/loop")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "a");
    }

    #[cfg(unix)]
    #[test]
    fn removes_the_partial_tree_when_a_copy_fails_midway() {
        use std::os::unix::fs::PermissionsExt;

        let t = TempTree::new();
        t.make_file("src/ok.txt", "ok");
        let blocked = t.make_file("src/blocked.txt", "secret");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let err = TransferOp::Copy
            .execute(&t.at("src"), &t.at("dest"))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            !t.at("dest").exists(),
            "a failed copy must not leave a partial tree behind"
        );
        assert!(t.at("src/ok.txt").exists(), "source must be untouched");
    }

    #[test]
    fn deletes_file() {
        let t = TempTree::new();
        t.make_file("src/file.txt", "hello world");

        MutationOp::Delete {
            path: t.at("src/file.txt"),
        }
        .execute()
        .unwrap();

        assert!(
            !t.at("src/file.txt").exists(),
            "deleted file must not exists"
        );
    }

    #[test]
    fn deletes_dir() {
        let t = TempTree::new();
        t.make_dir(&t.at("src"));

        MutationOp::Delete { path: t.at("src") }.execute().unwrap();

        assert!(!t.at("src").exists(), "deleted directory must not exists");
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_to_a_file_leaves_its_target_intact() {
        let t = TempTree::new();
        let target = t.make_file("src/target.txt", "precious");
        let link = t.at("src/link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        MutationOp::Delete { path: link.clone() }.execute().unwrap();

        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be gone"
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "precious",
            "the target must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_to_a_directory_leaves_its_contents_intact() {
        let t = TempTree::new();
        t.make_file("real/precious.txt", "precious");
        let link = t.at("link");
        std::os::unix::fs::symlink(t.at("real"), &link).unwrap();

        MutationOp::Delete { path: link.clone() }.execute().unwrap();

        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be gone"
        );
        assert_eq!(
            fs::read_to_string(t.at("real/precious.txt")).unwrap(),
            "precious",
            "the directory the link pointed at must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deletes_dangling_symlink() {
        let t = TempTree::new();
        let link = t.at("link");
        std::os::unix::fs::symlink(t.at("real"), &link).unwrap();

        MutationOp::Delete { path: link.clone() }.execute().unwrap();

        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be gone"
        );
    }

    #[test]
    fn deleting_non_existing_path_should_fail() {
        let t = TempTree::new();
        let err = MutationOp::Delete {
            path: t.at("non_existing"),
        }
        .execute()
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
