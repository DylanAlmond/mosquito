use std::path::PathBuf;

use serde::Serialize;

use crate::error::{AddFileError, RemoveError};

/// Opaque identifier for a shared file.
/// Newtype over u64 so signatures can't confuse it with a random number,
/// and we can swap the representation later (e.g. random tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FileId(pub u64);

/// A file the user dropped into the app.
#[derive(Debug, Clone, Serialize)]
pub struct SharedFile {
    pub id: FileId,
    /// File name only (e.g. `report.pdf`) — shown in UI and listing page.
    pub name: String,
    /// Absolute path on disk — the server streams from here.
    pub path: PathBuf,
    /// Size in bytes, captured when the file was added.
    pub size: u64,
}

/// In-memory registry of files currently being shared.
#[derive(Debug, Default)]
pub struct FileStore {
    files: Vec<SharedFile>,
    next_id: u64,
}

impl FileStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a file on disk. Metadata is read once, up front,
    /// so the UI and listing page never touch the filesystem.
    pub fn add(&mut self, path: impl Into<PathBuf>) -> Result<SharedFile, AddFileError> {
        let path = path.into();

        let metadata = std::fs::metadata(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AddFileError::NotFound(path.clone()),
            _ => AddFileError::Metadata(e),
        })?;

        if metadata.is_dir() {
            return Err(AddFileError::IsADirectory(path));
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());

        let file = SharedFile {
            id: FileId(self.next_id),
            name,
            path,
            size: metadata.len(),
        };

        self.next_id += 1;
        self.files.push(file.clone());

        Ok(file)
    }

    /// Remove a file from the registry. Returns what was removed
    /// so the caller can confirm exactly what disappeared.
    pub fn remove(&mut self, id: FileId) -> Result<SharedFile, RemoveError> {
        let index = self
            .files
            .iter()
            .position(|f| f.id == id)
            .ok_or(RemoveError::NotFound(id))?;
        Ok(self.files.remove(index))
    }

    pub fn get(&self, id: FileId) -> Option<&SharedFile> {
        self.files.iter().find(|f| f.id == id)
    }

    /// Insertion-ordered view of everything currently shared.
    pub fn list(&self) -> &[SharedFile] {
        &self.files
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    /// Helper: create a real file with the given contents inside a temp dir.
    fn make_file(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(contents)
            .unwrap();
        path
    }

    #[test]
    fn new_store_is_empty() {
        let store = FileStore::new();
        assert!(store.list().is_empty());
    }

    #[test]
    fn add_assigns_sequential_ids() {
        let dir = TempDir::new().unwrap();
        let a = make_file(dir.path(), "a.txt", b"hello");
        let b = make_file(dir.path(), "b.txt", b"world");

        let mut store = FileStore::new();
        let first = store.add(&a).unwrap();
        let second = store.add(&b).unwrap();

        assert_eq!(first.id, FileId(0));
        assert_eq!(second.id, FileId(1));
    }

    #[test]
    fn add_captures_name_and_size() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "report.pdf", b"0123456789");

        let mut store = FileStore::new();
        let file = store.add(&path).unwrap();

        assert_eq!(file.name, "report.pdf");
        assert_eq!(file.size, 10);
        assert_eq!(file.path, path);
    }

    #[test]
    fn add_nonexistent_file_is_an_error() {
        let mut store = FileStore::new();
        let err = store.add("/definitely/not/here.txt").unwrap_err();
        assert!(matches!(err, AddFileError::NotFound(_)));
    }

    #[test]
    fn add_directory_is_an_error() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        let err = store.add(dir.path()).unwrap_err();
        assert!(matches!(err, AddFileError::IsADirectory(_)));
    }

    #[test]
    fn get_finds_file_by_id() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "a.txt", b"hi");
        let mut store = FileStore::new();
        let added = store.add(&path).unwrap();

        assert_eq!(store.get(added.id).unwrap().name, "a.txt");
        assert!(store.get(FileId(999)).is_none());
    }

    #[test]
    fn remove_deletes_the_entry() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "a.txt", b"hi");
        let mut store = FileStore::new();
        let added = store.add(&path).unwrap();

        let removed = store.remove(added.id).unwrap();
        assert_eq!(removed.id, added.id);
        assert!(store.get(added.id).is_none());
        assert!(store.list().is_empty());
    }

    #[test]
    fn remove_unknown_id_is_an_error() {
        let mut store = FileStore::new();
        assert!(matches!(
            store.remove(FileId(42)),
            Err(RemoveError::NotFound(FileId(42)))
        ));
    }

    #[test]
    fn list_preserves_insertion_order() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        for name in ["c.txt", "a.txt", "b.txt"] {
            store.add(make_file(dir.path(), name, b"x")).unwrap();
        }
        let names: Vec<_> = store.list().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["c.txt", "a.txt", "b.txt"]);
    }

    #[test]
    fn ids_are_never_reused_after_removal() {
        let dir = TempDir::new().unwrap();
        let a = make_file(dir.path(), "a.txt", b"a");
        let b = make_file(dir.path(), "b.txt", b"b");

        let mut store = FileStore::new();
        let first = store.add(&a).unwrap();
        store.remove(first.id).unwrap();
        let second = store.add(&b).unwrap();

        assert_eq!(second.id, FileId(1)); // not 0 again
    }

    #[test]
    fn clear_removes_everything() {
        let dir = TempDir::new().unwrap();
        let mut store = FileStore::new();
        store.add(make_file(dir.path(), "a.txt", b"a")).unwrap();

        store.clear();

        assert!(store.list().is_empty());
    }

    #[test]
    fn non_ascii_file_names_are_kept() {
        let dir = TempDir::new().unwrap();
        let path = make_file(dir.path(), "résumé final.txt", b"x");
        let mut store = FileStore::new();

        let file = store.add(&path).unwrap();

        assert_eq!(file.name, "résumé final.txt");
    }
}
