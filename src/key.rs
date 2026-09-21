use crate::error::StorageError;
use std::fmt;

/// The character every crate should join key segments with, instead of each picking its own.
pub const KEY_SEPARATOR: char = '/';

/// A structured storage key: an ordered list of segments, joined with [`KEY_SEPARATOR`] on demand.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey(Vec<String>);

impl StorageKey {
    pub fn new<I, S>(parts: I) -> Result<Self, StorageError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let parts: Vec<String> = parts.into_iter().map(Into::into).collect();
        if parts.is_empty() {
            return Err(StorageError::InvalidKey(
                "StorageKey needs at least one segment".to_string(),
            ));
        }
        for part in &parts {
            validate_segment(part)?;
        }
        Ok(Self(parts))
    }

    pub fn joined(&self) -> String {
        self.0.join(&KEY_SEPARATOR.to_string())
    }

    pub fn from_joined(joined: &str) -> Result<Self, StorageError> {
        Self::new(joined.split(KEY_SEPARATOR))
    }

    /// Appends a trailing separator so a prefix scan only matches this key's
    /// children, not a sibling that merely shares a string prefix.
    pub fn to_scan_prefix(&self) -> String {
        let mut prefix = self.joined();
        prefix.push(KEY_SEPARATOR);
        prefix
    }
}

fn validate_segment(part: &str) -> Result<(), StorageError> {
    if part.is_empty() {
        return Err(StorageError::InvalidKey(
            "StorageKey segments must not be empty".to_string(),
        ));
    }
    if part.contains(KEY_SEPARATOR) {
        return Err(StorageError::InvalidKey(format!(
            "StorageKey segment {part:?} contains the reserved separator {KEY_SEPARATOR:?}"
        )));
    }
    Ok(())
}

impl fmt::Display for StorageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.joined())
    }
}

impl TryFrom<&str> for StorageKey {
    type Error = StorageError;

    fn try_from(part: &str) -> Result<Self, StorageError> {
        Self::new([part])
    }
}

impl TryFrom<String> for StorageKey {
    type Error = StorageError;

    fn try_from(part: String) -> Result<Self, StorageError> {
        Self::new([part])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_segments_with_the_shared_separator() {
        let key = StorageKey::new(["program", "123", "state"]).unwrap();
        assert_eq!(key.joined(), "program/123/state");
    }

    #[test]
    fn single_segment_key_has_no_separator() {
        let key = StorageKey::new(["comms_allow_list"]).unwrap();
        assert_eq!(key.joined(), "comms_allow_list");
    }

    #[test]
    fn scan_prefix_appends_a_trailing_separator() {
        let key = StorageKey::new(["program", "123"]).unwrap();
        assert_eq!(key.to_scan_prefix(), "program/123/");
    }

    #[test]
    fn scan_prefix_does_not_match_a_sibling_that_shares_a_string_prefix() {
        let prefix = StorageKey::new(["program", "123"])
            .unwrap()
            .to_scan_prefix();
        assert!("program/123/state".starts_with(&prefix));
        assert!(!"program/1234/state".starts_with(&prefix));
    }

    #[test]
    fn try_from_str_and_try_from_string_build_single_segment_keys() {
        assert_eq!(StorageKey::try_from("wallet").unwrap().joined(), "wallet");
        assert_eq!(
            StorageKey::try_from("wallet".to_string()).unwrap().joined(),
            "wallet"
        );
    }

    #[test]
    fn from_joined_round_trips_an_already_joined_key() {
        let key = StorageKey::new(["program", "123", "state"]).unwrap();
        assert_eq!(StorageKey::from_joined(&key.joined()).unwrap(), key);
    }

    #[test]
    fn from_joined_errors_on_a_double_separator() {
        let err = StorageKey::from_joined("program//state").unwrap_err();
        assert!(matches!(err, StorageError::InvalidKey(msg) if msg.contains("must not be empty")));
    }

    #[test]
    fn new_errors_on_empty_parts() {
        let err = StorageKey::new(Vec::<String>::new()).unwrap_err();
        assert!(
            matches!(err, StorageError::InvalidKey(msg) if msg.contains("at least one segment"))
        );
    }

    #[test]
    fn new_errors_on_empty_segment() {
        let err = StorageKey::new(["program", ""]).unwrap_err();
        assert!(matches!(err, StorageError::InvalidKey(msg) if msg.contains("must not be empty")));
    }

    #[test]
    fn new_errors_on_segment_containing_the_separator() {
        let err = StorageKey::new(["program", "123/state"]).unwrap_err();
        assert!(matches!(err, StorageError::InvalidKey(msg) if msg.contains("reserved separator")));
    }
}
