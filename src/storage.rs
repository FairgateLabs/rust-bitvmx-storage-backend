use rocksdb;
use crate::error::StorageError;

pub struct Storage {
    db: rocksdb::DB,
}

impl Storage {
    pub fn new() -> Result<Storage, StorageError> {
        let mut options = rocksdb::Options::default();

        options.create_if_missing(true);

        let db = rocksdb::DB::open(&options,"/tmp/storage").map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn new_with_path(path: &str) -> Result<Storage, StorageError> {
        let mut options = rocksdb::Options::default();

        options.create_if_missing(true);

        let db = rocksdb::DB::open(&options, path).map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn delete(&self, key: &str) -> Result<(),StorageError>{
        self.db.delete(key.as_bytes()).map_err(|_| StorageError::WriteError)?;
        Ok(())
    }

    pub fn write(&self, key: &str, value: &str)-> Result<(),StorageError>{
        self.db.put(key.as_bytes(), value.as_bytes()).map_err(|_| StorageError::WriteError)?;
        Ok(())
    }

    pub fn read(&self, key: &str) -> Result<Option<String>, StorageError> {
        match self.db.get(key.as_bytes()) {
            Ok(Some(value)) => Ok(Some(String::from_utf8(value).map_err(|_| StorageError::ConversionError)?)),
            Ok(None) => Ok(None),
            Err(_) => Err(StorageError::ReadError),
        }
    }

    pub fn is_empty(&self) -> bool {
        let iter = self.db.iterator(rocksdb::IteratorMode::Start);
        let is_empty = iter.peekable().peek().is_none();
        is_empty
    }

    pub fn partial_compare(&self, key: &str) -> Result<Vec<(String, String)>, StorageError> {
        let mut result = Vec::new();
        let mut iter = self.db.iterator(rocksdb::IteratorMode::From(key.as_bytes(), rocksdb::Direction::Forward));
        while let Some(Ok((k,v)))= iter.next() {
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            let v = String::from_utf8(v.to_vec()).map_err(|_| StorageError::ConversionError)?;
            if k.starts_with(key) {
                result.push((k,v));
            } else {
                break;
            }
        }
        Ok(result)
    }

    pub fn has_key(&self, key: &str) -> Result<bool, StorageError> {
        let result = self.db.get(key.as_bytes()).map_err(|_| StorageError::ReadError)?;
        Ok(result.is_some())
    }
    
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use rand::{thread_rng, RngCore};

    fn temp_storage() -> String {
        let dir = env::temp_dir();
        let mut rng = thread_rng();
        let index = rng.next_u32();
        let storage_path = dir.join(format!("secure_storage_{}.db", index));
        storage_path.to_str().expect("Failed to get path to temp file").to_string()
    }

    #[test]
    fn test_01_new_storage_starts_empty() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        assert!(fs.is_empty());
    }

    #[test]
    fn test_02_add_value_to_storage() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test", "test_value");
        assert_eq!(fs.read("test").unwrap(), Some("test_value".to_string()));
    }

    #[test]
    fn test_03_read_a_value() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test", "test_value");
        assert_eq!(fs.read("test").unwrap(), Some("test_value".to_string()));
    }

    #[test]
    fn test_04_delete_value() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test", "test_value");
        assert_eq!(fs.read("test").unwrap(), Some("test_value".to_string()));
        let _ = fs.delete("test");
        assert_eq!(fs.read("test").unwrap(), None);
    }

    #[test]
    fn test_05_find_multiple_answers() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test1", "test_value1");
        let _ = fs.write("test2", "test_value2");
        let _ = fs.write("test3", "test_value3");
        let _ = fs.write("tes4", "test_value4");

        let result = fs.partial_compare("test").unwrap();
        assert_eq!(result, vec![("test1".to_string(), "test_value1".to_string()), ("test2".to_string(), "test_value2".to_string()), ("test3".to_string(), "test_value3".to_string())]);
    }

    #[test]
    fn test_06_has_key() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test1", "test_value1");
        assert!(fs.has_key("test1").unwrap());
        assert!(!fs.has_key("test2").unwrap());
    }
}