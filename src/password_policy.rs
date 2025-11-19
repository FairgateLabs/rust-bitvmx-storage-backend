use crate::storage_config::PasswordPolicyConfig;

pub const UPPERCASE: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S',
    'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
];
pub const DIGITS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];
pub const SPECIAL: &[char] = &[
    '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/', ':', ';', '<', '=',
    '>', '?', '@', '[', '\\', ']', '^', '_', '`', '{', '|', '}', '~',
];

pub struct PasswordPolicy {
    min_length: usize,
    min_number_of_special_chars: usize,
    min_number_of_uppercase: usize,
    min_number_of_digits: usize,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        PasswordPolicy {
            min_length: 12,
            min_number_of_special_chars: 3,
            min_number_of_uppercase: 3,
            min_number_of_digits: 3,
        }
    }
}

impl PasswordPolicy {
    pub fn new(
        config: PasswordPolicyConfig
    ) -> Self {
        PasswordPolicy {
            min_length: config.min_length,
            min_number_of_special_chars: config.min_number_of_special_chars,
            min_number_of_uppercase: config.min_number_of_uppercase,
            min_number_of_digits: config.min_number_of_digits,
        }
    }

    pub fn is_valid(&self, password: &str) -> bool {
        let has_enough_length = password.len() >= self.min_length;
        let has_enough_special_chars = password.chars().filter(|c| SPECIAL.contains(c)).count()
            >= self.min_number_of_special_chars;
        let has_enough_uppercase_chars = password.chars().filter(|c| UPPERCASE.contains(c)).count()
            >= self.min_number_of_uppercase;
        let has_enough_digits =
            password.chars().filter(|c| DIGITS.contains(c)).count() >= self.min_number_of_digits;

        has_enough_length
            && has_enough_special_chars
            && has_enough_uppercase_chars
            && has_enough_digits
    }
}
