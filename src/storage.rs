use std::{env, path::PathBuf};

use crate::error::StorageError;
use rocksdb::SliceTransform;

pub struct Storage {
    db: rocksdb::DB,
}

impl Storage {
    pub fn new() -> Result<Storage, StorageError> {
        let options = create_options();
        let default_path = env::current_dir()
            .map_err(|_| StorageError::PathError)?
            .join("storage.db");

        let db =
            rocksdb::DB::open(&options, default_path).map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn new_with_path(path: &PathBuf) -> Result<Storage, StorageError> {
        let options = create_options();

        let db = rocksdb::DB::open(&options, path).map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn new_with_options(options: rocksdb::Options) -> Result<Storage, StorageError> {
        let default_path = env::current_dir()
            .map_err(|_| StorageError::PathError)?
            .join("storage.db");

        let db =
            rocksdb::DB::open(&options, default_path).map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn new_with_path_and_option(
        path: &PathBuf,
        options: rocksdb::Options,
    ) -> Result<Storage, StorageError> {
        let db = rocksdb::DB::open(&options, path).map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn open(path: &PathBuf) -> Result<Storage, StorageError> {
        let mut options = rocksdb::Options::default();
        options.set_prefix_extractor(get_prefix_extractor());

        let db = rocksdb::DB::open(&options, path).map_err(|_| StorageError::CreationError)?;
        Ok(Storage { db })
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.db
            .delete(key.as_bytes())
            .map_err(|_| StorageError::WriteError)?;
        Ok(())
    }

    pub fn write<V: AsRef<[u8]>>(&self, key: &str, value: V) -> Result<(), StorageError> {
        self.db
            .put(key.as_bytes(), value.as_ref())
            .map_err(|_| StorageError::WriteError)?;
        Ok(())
    }

    pub fn read(&self, key: &str) -> Result<Option<String>, StorageError> {
        match self.db.get(key.as_bytes()) {
            Ok(Some(value)) => Ok(Some(
                String::from_utf8(value).map_err(|_| StorageError::ConversionError)?,
            )),
            Ok(None) => Ok(None),
            Err(_) => Err(StorageError::ReadError),
        }
    }

    pub fn is_empty(&self) -> bool {
        let iter = self.db.iterator(rocksdb::IteratorMode::Start);
        let is_empty = iter.peekable().peek().is_none();
        is_empty
    }

    pub fn keys(&self) -> Vec<String> {
        let mut result = Vec::new();
        let mut iter = self.db.iterator(rocksdb::IteratorMode::Start);
        while let Some(Ok((k, _))) = iter.next() {
            let k = String::from_utf8(k.to_vec()).unwrap();
            result.push(k);
        }
        result
    }

    pub fn partial_compare(&self, key: &str) -> Result<Vec<(String, String)>, StorageError> {
        let mut result = Vec::new();
        let mut iter = self.db.iterator(rocksdb::IteratorMode::From(
            key.as_bytes(),
            rocksdb::Direction::Forward,
        ));
        while let Some(Ok((k, v))) = iter.next() {
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            let v = String::from_utf8(v.to_vec()).map_err(|_| StorageError::ConversionError)?;
            if k.starts_with(key) {
                result.push((k, v));
            } else {
                break;
            }
        }

        Ok(result)
    }

    pub fn has_key(&self, key: &str) -> Result<bool, StorageError> {
        let result = self
            .db
            .get(key.as_bytes())
            .map_err(|_| StorageError::ReadError)?;
        Ok(result.is_some())
    }
}

fn create_options() -> rocksdb::Options {
    let mut options = rocksdb::Options::default();
    options.create_if_missing(true);
    options.set_prefix_extractor(get_prefix_extractor());
    options
}

pub fn get_prefix_extractor() -> SliceTransform {
    let prefix_extractor = SliceTransform::create("dynamic_prefix", move |key| {
        let mut positions = key.iter().enumerate().filter(|&(_, &c)| c == b'/').map(|(i, _)| i);
    
        if let (Some(_), Some(_), Some(third_pos)) = (positions.next(), positions.next(), positions.next()) {
            return &key[..third_pos + 1];
        }
        key
    }, None);
    prefix_extractor
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, RngCore};
    use std::env;

    fn temp_storage() -> PathBuf {
        let dir = env::temp_dir();
        let mut rng = thread_rng();
        let index = rng.next_u32();
        dir.join(format!("storage_{}.db", index))
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
        assert_eq!(
            result,
            vec![
                ("test1".to_string(), "test_value1".to_string()),
                ("test2".to_string(), "test_value2".to_string()),
                ("test3".to_string(), "test_value3".to_string())
            ]
        );
    }

    #[test]
    fn test_06_has_key() {
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test1", "test_value1");
        assert!(fs.has_key("test1").unwrap());
        assert!(!fs.has_key("test2").unwrap());
    }

    #[test]
    fn test_07_open_storage() {
        let path = temp_storage();
        let fs = Storage::new_with_path(&path).unwrap();
        let _ = fs.write("test1", "test_value1");

        drop(fs);

        let fs2 = Storage::open(&path);
        assert!(fs2.is_ok());
        assert_eq!(fs2.unwrap().read("test1").unwrap(), Some("test_value1".to_string()));
    }

    #[test]
    fn test_08_open_inexistent_storage() {
        let path = temp_storage();
        let fs = Storage::open(&path);
        assert!(fs.is_err());
    }

    #[test]
    fn test_09_keys(){
        let fs = Storage::new_with_path(&temp_storage()).unwrap();
        let _ = fs.write("test1", "test_value1");
        let _ = fs.write("test2", "test_value2");
        let _ = fs.write("test3", "test_value3");
        let _ = fs.write("tes4", "test_value4");

        let keys = fs.keys();
        assert_eq!(keys.len(), 4);
        assert!(keys.contains(&"test1".to_string()));
        assert!(keys.contains(&"test2".to_string()));
        assert!(keys.contains(&"test3".to_string()));
        assert!(keys.contains(&"tes4".to_string()));
    }
}
