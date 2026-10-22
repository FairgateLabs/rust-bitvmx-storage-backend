use redact::Secret;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct PasswordPolicyConfig {
    pub min_length: usize,
    pub min_number_of_special_chars: usize,
    pub min_number_of_uppercase: usize,
    pub min_number_of_digits: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StorageConfig {
    pub path: String,
    pub password: Option<Secret<String>>,
    /// Fsync every write, including transaction commits.
    #[serde(default = "default_sync_writes")]
    pub sync_writes: bool,
}

fn default_sync_writes() -> bool {
    true
}

impl StorageConfig {
    pub fn new(path: String, password: Option<Secret<String>>) -> Self {
        Self {
            path,
            password,
            sync_writes: default_sync_writes(),
        }
    }

    pub fn without_sync_writes(mut self) -> Self {
        self.sync_writes = false;
        self
    }
}
