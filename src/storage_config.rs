use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct StorageConfig {
    pub path: String,
    pub password: Option<String>,
}
