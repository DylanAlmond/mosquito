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
