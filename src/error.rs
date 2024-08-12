use std::fmt;

#[derive(Debug)]
pub enum StorageError {
    WriteError,
    ReadError,
    ConversionError,
    CreationError,
    PathError,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            StorageError::WriteError => write!(f, "Error modifying to storage"),
            StorageError::ReadError => write!(f, "Error reading from storage"),
            StorageError::ConversionError => write!(f, "Error converting data"),
            StorageError::CreationError => write!(f, "Error creating storage"),
            StorageError::PathError => write!(f, "Error with the path"),
        }
    }
}
