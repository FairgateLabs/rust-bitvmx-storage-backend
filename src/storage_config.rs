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
    /// Fsync writes made outside a transaction (commits always sync).
    #[serde(default)]
    pub sync_writes: bool,
}

impl StorageConfig {
    pub fn new(path: String, password: Option<Secret<String>>) -> Self {
        Self {
            path,
            password,
            sync_writes: false,
        }
    }

    pub fn with_sync_writes(mut self, sync_writes: bool) -> Self {
        self.sync_writes = sync_writes;
        self
    }
}
