use thiserror::Error;
use std::io::Error as IoError;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Document not found")]
    NotFound,
    #[error("Error modifying storage")]
    WriteError,
    #[error("Error reading from storage")]
    ReadError,
    #[error("Error converting data")]
    ConversionError,
    #[error("Error serializing/deserializing data")]
    SerializationError,
    #[error("Error creating storage")]
    CreationError(#[from] rocksdb::Error),
    #[error("Error with the path")]
    PathError,
    #[error("Error while commiting changes")]
    CommitError,
    #[error("Error while eliminating db files: {0}")]
    EliminationError(#[from] IoError),
}
