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
    CreationError,
    #[error("Error with the path")]
    PathError,
}