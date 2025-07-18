use std::io::Error as IoError;
use thiserror::Error;

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
    #[error("Error while commiting changes")]
    CommitError,
    #[error("Failed I/O action: {0}")]
    IoError(#[from] IoError),
    #[error("Failed to encrypt data")]
    FailedToEncryptData { error: cocoon::Error },
    #[error("Failed to decrypt data")]
    FailedToDecryptData { error: cocoon::Error },
    #[error("Backup path not set")]
    BackupPathNotSet,
}
