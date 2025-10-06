use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct StorageConfig {
    pub path: String,
    pub password: Option<String>,
}

impl StorageConfig {
    pub fn new(path: String, password: Option<String>) -> Self {
        Self { path, password }
    }
}
