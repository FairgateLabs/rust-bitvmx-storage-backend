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
}

impl StorageConfig {
    pub fn new(path: String, password: Option<String>) -> Self {
        let secret: Option<Secret<String>> = if let Some(password) = password {
            Some(Secret::from(password))
        } else {
            None
        };

        Self {
            path,
            password: secret,
        }
    }
}
