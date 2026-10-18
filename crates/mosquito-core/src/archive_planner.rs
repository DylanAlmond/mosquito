//! Plans the zip archive layout before a byte is written: the archive
//! name for Content-Disposition and each member's **entry path** (ADR-0001).
//!
//! Pure and deterministic, so the layout rules are unit-testable without
//! touching disk or HTTP. Selection order = write order.

use std::collections::HashSet;

use crate::file_store::SharedFile;

/// Generic archive name whenever the single-root rule doesn't apply.
const GENERIC_ARCHIVE_NAME: &str = "mosquito-files.zip";

/// One selected file and the path it gets inside the archive.
pub(crate) struct ArchiveEntry {
    pub(crate) file: SharedFile,
    pub(crate) entry_path: String,
}

/// The complete archive layout: the name for Content-Disposition plus
/// entry paths in write order.
pub(crate) struct ArchivePlan {
    pub(crate) name: String,
    pub(crate) entries: Vec<ArchiveEntry>,
}

/// Derive the archive layout at write time from the stored frozen
/// snapshots (ADR-0001): folder members keep their root's name as a path
/// prefix, loose files sit at the archive root, and colliding paths get
/// numeric suffixes so nothing is dropped. Selection order = write order.
pub(crate) fn plan_archive(files: &[SharedFile]) -> ArchivePlan {
    let mut used = HashSet::new();
    let entries = files
        .iter()
        .map(|file| {
            let entry_path = unique_entry_path(&archive_entry_path(file), &mut used);
            ArchiveEntry {
                file: file.clone(),
                entry_path,
            }
        })
        .collect();

    ArchivePlan {
        name: archive_name(files),
        entries,
    }
}

/// Path inside the archive: `Photos/vacation/beach.jpg` for a member of
/// root `<...>/Photos`, the display name for a loose file. Forward
/// slashes are already guaranteed by the stored relative path, and a
/// directory name can't contain one.
fn archive_entry_path(file: &SharedFile) -> String {
    let Some(provenance) = &file.provenance else {
        return file.name.clone();
    };

    let root = provenance
        .folder_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    // A root without a file name (a drive letter, say) has nothing to
    // prefix; members land at the archive root and collisions are
    // resolved like any other.
    if root.is_empty() {
        provenance.relative_path.clone()
    } else {
        format!("{root}/{}", provenance.relative_path)
    }
}

/// The first unused entry path, renaming collisions ` (2)`, ` (3)`, ...
/// before the extension (`beach.jpg` → `beach (2).jpg`). Same file added
/// twice and same-named roots both land here; nothing is dropped.
fn unique_entry_path(requested: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = requested.to_string();
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = with_numeric_suffix(requested, n);
        n += 1;
    }
    candidate
}

/// `dir/name.ext` → `dir/name (2).ext`. A leading dot is part of a
/// dotfile's name, not an extension (` .hidden` → `.hidden (2)`).
fn with_numeric_suffix(path: &str, n: usize) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };

    match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => format!("{dir}{} ({n}){}", &name[..dot], &name[dot..]),
        None => format!("{dir}{name} ({n})"),
    }
}

/// `<root-name>.zip` when every selected file belongs to exactly one
/// folder root; the generic name otherwise (loose files in the mix,
/// several roots, or an empty selection).
fn archive_name(files: &[SharedFile]) -> String {
    let mut root: Option<&std::path::Path> = None;
    for file in files {
        let Some(provenance) = &file.provenance else {
            return GENERIC_ARCHIVE_NAME.to_string();
        };
        match root {
            Some(seen) if *seen != provenance.folder_root => {
                return GENERIC_ARCHIVE_NAME.to_string();
            }
            _ => root = Some(&provenance.folder_root),
        }
    }

    match root.and_then(|root| root.file_name()) {
        Some(name) => format!("{}.zip", name.to_string_lossy()),
        None => GENERIC_ARCHIVE_NAME.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{member, shared_file};

    // --- plan_archive: entry paths ---

    #[test]
    fn plan_puts_folder_members_under_the_root_name_and_loose_files_at_the_archive_root() {
        let files = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            member(1, "b.txt", "Photos", "sub/b.txt"),
            shared_file(2, "c.txt", None),
        ];

        let plan = plan_archive(&files);

        let paths: Vec<_> = plan.entries.iter().map(|e| e.entry_path.as_str()).collect();
        assert_eq!(paths, ["Photos/a.txt", "Photos/sub/b.txt", "c.txt"]);
    }

    // --- plan_archive: collisions ---

    #[test]
    fn colliding_entry_paths_get_numeric_suffixes_before_the_extension() {
        let files = vec![
            shared_file(0, "a.txt", None),
            shared_file(1, "a.txt", None),
            shared_file(2, "a.txt", None),
        ];

        let plan = plan_archive(&files);

        let paths: Vec<_> = plan.entries.iter().map(|e| e.entry_path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "a (2).txt", "a (3).txt"]);
    }

    #[test]
    fn collisions_suffix_the_file_name_and_keep_multi_part_extensions() {
        let files = vec![
            member(0, "a.jpg", "Photos", "vacation/beach.jpg"),
            member(1, "b.jpg", "photos", "trip/beach.jpg"),
            shared_file(2, "archive.tar.gz", None),
            shared_file(3, "archive.tar.gz", None),
            shared_file(4, ".hidden", None),
            shared_file(5, ".hidden", None),
        ];

        let plan = plan_archive(&files);

        let paths: Vec<_> = plan.entries.iter().map(|e| e.entry_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Photos/vacation/beach.jpg",
                "photos/trip/beach.jpg",
                "archive.tar.gz",
                "archive.tar (2).gz",
                ".hidden",
                ".hidden (2)",
            ]
        );
    }

    // --- plan_archive: archive name ---

    #[test]
    fn single_root_selection_names_the_archive_after_the_root() {
        let files = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            member(1, "b.txt", "Photos", "sub/b.txt"),
        ];

        let plan = plan_archive(&files);

        assert_eq!(plan.name, "Photos.zip");
    }

    #[test]
    fn mixed_selections_fall_back_to_the_generic_archive_name() {
        let loose_plus_one_root = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            shared_file(1, "c.txt", None),
        ];
        assert_eq!(
            plan_archive(&loose_plus_one_root).name,
            "mosquito-files.zip"
        );

        let two_roots = vec![
            member(0, "a.txt", "Photos", "a.txt"),
            member(1, "b.txt", "Docs", "b.txt"),
        ];
        assert_eq!(plan_archive(&two_roots).name, "mosquito-files.zip");

        let all_loose = vec![shared_file(0, "a.txt", None)];
        assert_eq!(plan_archive(&all_loose).name, "mosquito-files.zip");
    }
}
