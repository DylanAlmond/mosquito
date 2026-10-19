//! Shared constructors for the unit tests of the pure planning/rendering
//! modules. Nothing here touches a disk state the code under test cares
//! about — the paths are fabricated and never opened in these tests.

use std::path::PathBuf;

use crate::file_store::{FileId, FolderProvenance, SharedFile};

/// A fake location that still splits on the platform's separators:
/// a literal `C:\nowhere\Photos` is one component on Linux (where `\` is
/// not a separator), so `file_name()` returned the whole fabricated
/// prefix and the archive layout tests failed there.
/// Nothing is ever created on disk — the planning is pure.
pub(crate) fn nowhere(name: &str) -> PathBuf {
    std::env::temp_dir().join("mosquito-nowhere").join(name)
}

pub(crate) fn shared_file(id: u64, name: &str, provenance: Option<FolderProvenance>) -> SharedFile {
    SharedFile {
        id: FileId(id),
        name: name.into(),
        path: nowhere(name),
        size: 1,
        provenance,
    }
}

pub(crate) fn member(id: u64, name: &str, root: &str, relative_path: &str) -> SharedFile {
    shared_file(
        id,
        name,
        Some(FolderProvenance {
            folder_root: nowhere(root),
            relative_path: relative_path.into(),
        }),
    )
}

/// Disk-backed fixtures for the tests that exercise real walks
/// (counting, materialization).
pub(crate) mod disk {
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// Create a real file with the given contents inside a directory.
    pub(crate) fn make_file(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::File::create(&path)
            .unwrap()
            .write_all(contents)
            .unwrap();
        path
    }

    /// Three files over three levels plus an empty directory:
    /// `a.txt`, `sub/b.txt`, `sub/deep/c.txt`, `empty/`.
    pub(crate) fn nested_tree(dir: &Path) -> PathBuf {
        let root = dir.join("photos");
        fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        make_file(&root, "a.txt", b"a");
        make_file(&root.join("sub"), "b.txt", b"b");
        make_file(&root.join("sub").join("deep"), "c.txt", b"c");
        fs::create_dir(root.join("empty")).unwrap();
        root
    }

    /// Deny read access to a directory — Everyone SID on Windows, mode 0
    /// on Unix. Returns None where the environment won't cooperate (no
    /// icacls success there, or privileges that retain access), so tests
    /// can skip instead of failing. Access is restored on drop.
    pub(crate) fn seal_directory(path: &Path) -> Option<SealedDir> {
        SealedDir::attempt(path).then_some(SealedDir {
            path: path.to_path_buf(),
            sealed: true,
        })
    }

    pub(crate) struct SealedDir {
        path: PathBuf,
        sealed: bool,
    }

    impl SealedDir {
        #[cfg(windows)]
        fn attempt(path: &Path) -> bool {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;

            std::process::Command::new("icacls")
                .arg(path)
                .args(["/deny", "*S-1-1-0:(OI)(CI)R"])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        }

        #[cfg(unix)]
        fn attempt(path: &Path) -> bool {
            use std::os::unix::fs::PermissionsExt;

            let _ = fs::set_permissions(path, std::fs::Permissions::from_mode(0));
            // Privileges may retain access (e.g. running as root): the
            // try must be verified, not assumed.
            if fs::read_dir(path).is_ok() {
                restore(path);
                return false;
            }
            true
        }

        #[cfg(windows)]
        fn restore(path: &Path) {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;

            let _ = std::process::Command::new("icacls")
                .arg(path)
                .args(["/reset", "/t", "/q"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }

        #[cfg(unix)]
        fn restore(path: &Path) {
            use std::os::unix::fs::PermissionsExt;

            let _ = fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        }
    }

    impl Drop for SealedDir {
        fn drop(&mut self) {
            if self.sealed {
                SealedDir::restore(&self.path);
            }
        }
    }
}
