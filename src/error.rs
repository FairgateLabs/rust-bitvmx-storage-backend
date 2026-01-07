use crate::password_policy::PasswordPolicy;
use std::io::Error as IoError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Value not found {0}")]
    NotFound(String),
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
    #[error("Password does not meet complexity requirements. Required policy: {0:?}")]
    WeakPassword(PasswordPolicy),
    #[error("Error generating random DEK: {0}")]
    RandomDekGenerationError(#[from] rand::rand_core::OsError),
    #[error("Wrong password provided")]
    WrongPassword,
    #[error("No password set for the storage")]
    NoPasswordSet,
    #[error("Global transaction is already active")]
    GlobalTransactionAlreadyActiveError,
}
