use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct PasswordPolicyConfig{
    pub min_length: usize,
    pub min_number_of_special_chars: usize,
    pub min_number_of_uppercase: usize,
    pub min_number_of_digits: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StorageConfig {
    pub path: String,
    pub password: Option<String>,
    pub password_policy: Option<PasswordPolicyConfig>,
}

impl StorageConfig {
    pub fn new(path: String, password: Option<String>, password_policy: Option<PasswordPolicyConfig>) -> Self {
        Self { path, password, password_policy }
    }
}
