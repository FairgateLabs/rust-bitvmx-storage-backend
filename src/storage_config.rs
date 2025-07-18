use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct StorageConfig {
    pub path: String,
    pub encrypt: Option<String>,
    pub backup_path: Option<String>,
}

impl StorageConfig {
    pub fn new(path: String, encrypt: Option<String>, backup_path: Option<String>) -> Self {
        Self {
            path,
            encrypt,
            backup_path,
        }
    }
}
