use std::fmt;

/// The character every crate should join key segments with, instead of each picking its own.
pub const KEY_SEPARATOR: char = '/';

/// A structured storage key: an ordered list of segments, joined with [`KEY_SEPARATOR`] on demand.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey(Vec<String>);

impl StorageKey {
    pub fn new<I, S>(parts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let parts: Vec<String> = parts.into_iter().map(Into::into).collect();
        assert!(!parts.is_empty(), "StorageKey needs at least one segment");
        for part in &parts {
            validate_segment(part);
        }
        Self(parts)
    }

    pub fn joined(&self) -> String {
        self.0.join(&KEY_SEPARATOR.to_string())
    }

    pub fn from_joined(joined: &str) -> Self {
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

fn validate_segment(part: &str) {
    assert!(!part.is_empty(), "StorageKey segments must not be empty");
    assert!(
        !part.contains(KEY_SEPARATOR),
        "StorageKey segment {part:?} contains the reserved separator {KEY_SEPARATOR:?}"
    );
}

impl fmt::Display for StorageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.joined())
    }
}

impl From<&str> for StorageKey {
    fn from(part: &str) -> Self {
        Self::new([part])
    }
}

impl From<String> for StorageKey {
    fn from(part: String) -> Self {
        Self::new([part])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_segments_with_the_shared_separator() {
        let key = StorageKey::new(["program", "123", "state"]);
        assert_eq!(key.joined(), "program/123/state");
    }

    #[test]
    fn single_segment_key_has_no_separator() {
        let key = StorageKey::new(["comms_allow_list"]);
        assert_eq!(key.joined(), "comms_allow_list");
    }

    #[test]
    fn scan_prefix_appends_a_trailing_separator() {
        let key = StorageKey::new(["program", "123"]);
        assert_eq!(key.to_scan_prefix(), "program/123/");
    }

    #[test]
    fn scan_prefix_does_not_match_a_sibling_that_shares_a_string_prefix() {
        let prefix = StorageKey::new(["program", "123"]).to_scan_prefix();
        assert!("program/123/state".starts_with(&prefix));
        assert!(!"program/1234/state".starts_with(&prefix));
    }

    #[test]
    fn from_str_and_from_string_build_single_segment_keys() {
        assert_eq!(StorageKey::from("wallet").joined(), "wallet");
        assert_eq!(StorageKey::from("wallet".to_string()).joined(), "wallet");
    }

    #[test]
    fn from_joined_round_trips_an_already_joined_key() {
        let key = StorageKey::new(["program", "123", "state"]);
        assert_eq!(StorageKey::from_joined(&key.joined()), key);
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn from_joined_panics_on_a_double_separator() {
        StorageKey::from_joined("program//state");
    }

    #[test]
    #[should_panic(expected = "at least one segment")]
    fn new_panics_on_empty_parts() {
        StorageKey::new(Vec::<String>::new());
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn new_panics_on_empty_segment() {
        StorageKey::new(["program", ""]);
    }

    #[test]
    #[should_panic(expected = "reserved separator")]
    fn new_panics_on_segment_containing_the_separator() {
        StorageKey::new(["program", "123/state"]);
    }
}
