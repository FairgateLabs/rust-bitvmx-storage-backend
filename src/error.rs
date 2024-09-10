use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Failed to write to file")]
    WriteError,

    #[error("Failed to read from file")]
    ReadError,

    #[error("Failed to create file")]
    CreationError,

    #[error("Failed to convert value")]
    ConversionError,

    #[error("Invalid path")]
    PathError,
}