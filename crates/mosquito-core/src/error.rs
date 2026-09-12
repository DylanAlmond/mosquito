use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

use crate::FileId;

#[derive(Debug, Error)]
pub enum AddFileError {
    #[error("path does not exist: {0}")]
    NotFound(PathBuf),
    #[error("directories can't be shared: {0}")]
    IsADirectory(PathBuf),
    #[error("failed to read file metadata")]
    Metadata(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum RemoveError {
    #[error("no shared file with id {0}")]
    NotFound(FileId),
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
