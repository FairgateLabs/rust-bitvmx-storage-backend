use std::{cell::RefCell, collections::HashMap, env, path::PathBuf};

use crate::error::StorageError;
use rocksdb::{TransactionDB, SliceTransform, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub struct Storage{
    db: rocksdb::TransactionDB,
    transactions: RefCell<HashMap<usize, Box<rocksdb::Transaction<'static, TransactionDB>>>>,
}

pub trait KeyValueStore {
    fn get<K, V>(&self, key: K) -> Result<Option<V>, StorageError>
    where
        K: AsRef<str>,
        V: DeserializeOwned;
    
    fn set<K, V>(&self, key: K, value: V, transaction_id: Option<usize>) -> Result<(), StorageError>
    where
        K: AsRef<str>,
        V: Serialize;

    fn save<V>(&self, value: V, transaction_id: Option<usize>) -> Result<String, StorageError>
    where
        V: Serialize;

    fn update<V>(&self, id: &str, updates: HashMap<&str, Value>, transaction_id: Option<usize>) -> Result<V, StorageError>
    where
        V: Serialize + DeserializeOwned + Clone;
}    

impl Storage {
    pub fn new() -> Result<Storage, StorageError> {
        let options = create_options();
        let default_path = env::current_dir()
            .map_err(|_| StorageError::PathError)?
            .join("storage.db");
        Storage::new_with_path_and_option(&default_path, options)
    }

    /// Creates a new storage or opens the existing one if present.
    pub fn new_with_path(path: &PathBuf) -> Result<Storage, StorageError> {
        let options = create_options();
        Storage::new_with_path_and_option(path, options)
    }

    pub fn new_with_options(options: rocksdb::Options) -> Result<Storage, StorageError> {
        let default_path = env::current_dir()
            .map_err(|_| StorageError::PathError)?
            .join("storage.db");

        Storage::new_with_path_and_option(&default_path, options)
    }

    pub fn new_with_path_and_option(
        path: &PathBuf,
        options: rocksdb::Options,
    ) -> Result<Storage, StorageError> {
        let db = rocksdb::TransactionDB::open(&options, &rocksdb::TransactionDBOptions::default(), path)?;
        Ok(Storage { db, transactions: RefCell::new(HashMap::new()) })
    }


    pub fn open(path: &PathBuf) -> Result<Storage, StorageError> {
        let mut options = rocksdb::Options::default();
        options.set_prefix_extractor(get_prefix_extractor());
        Storage::new_with_path_and_option(path, options)
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        let tx = self.db.transaction();
        delete_with_transaction(key, &tx)?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn transactional_delete(&self, key: &str, transaction_id: usize) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map.get_mut(&transaction_id).ok_or(StorageError::NotFound)?;
        delete_with_transaction(key, tx)?;

        Ok(())
    }
    
    pub fn write(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let tx = self.db.transaction();
        write_with_transaction(&tx, key, value)?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn transactional_write(&self, key: &str, value: &str, transaction_id: usize) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map.get_mut(&transaction_id).ok_or(StorageError::NotFound)?;
        write_with_transaction(tx, key, value)?;
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

    pub fn keys(&self) -> Result<Vec<String>, StorageError> {
        let mut result = Vec::new();
        let mut iter = self.db.iterator(rocksdb::IteratorMode::Start);
        while let Some(Ok((k, _))) = iter.next() {
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            result.push(k);
        }
        Ok(result)
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

    pub fn begin_transaction(&self) -> usize {
        let transaction = self.db.transaction();
        let mut map = self.transactions.borrow_mut();
        let id = map.len() + 1;
        map.insert(id, Box::new(
            unsafe { std::mem::transmute::<_, rocksdb::Transaction<'static, TransactionDB>>(transaction) 
        }));
        id
    }

    pub fn commit_transaction(&self, transaction_id: usize) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map.remove(&transaction_id).ok_or(StorageError::NotFound)?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn rollback_transaction(&self, transaction_id: usize) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        map.remove(&transaction_id).ok_or(StorageError::NotFound)?;
        Ok(())
    }
}

fn delete_with_transaction(key: &str, tx: &Transaction<TransactionDB>) -> Result<(), StorageError> {
    tx.delete(key.as_bytes())
      .map_err(|_| StorageError::WriteError)?;

    Ok(())
}

fn write_with_transaction(tx: &Transaction<TransactionDB>, key: &str, value: &str) -> Result<(), StorageError> {
    tx.put(key.as_bytes(), value.as_bytes())
      .map_err(|_| StorageError::WriteError)?;
    Ok(())
}

impl KeyValueStore for Storage {
    fn get<K, V>(&self, key: K) -> Result<Option<V>, StorageError>
    where
        K: AsRef<str>,
        V: DeserializeOwned,
    {
        let key = key.as_ref();
        let value = self.read(key)?;

        match value {
            Some(value) => {
                let value = serde_json::from_str(&value).map_err(|_| StorageError::ConversionError)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn set<K, V>(&self, key: K, value: V, transaction_id: Option<usize>) -> Result<(), StorageError>
    where
        K: AsRef<str>,
        V: Serialize,
    {
        let key = key.as_ref();
        let value = serde_json::to_string(&value).map_err(|_| StorageError::ConversionError)?;

        match transaction_id {
            Some(id) => Ok(self.transactional_write(key, &value, id)?),
            None => Ok(self.write(key, &value)?),
            
        }
    }

    fn save<V>(&self, value: V, transaction_id: Option<usize>) -> Result<String, StorageError>
    where
        V: Serialize,
    {
        let id = Uuid::new_v4().to_string();
        self.set(id.clone(), value, transaction_id)?;
        Ok(id)
    }

    fn update<V>(&self, id: &str, updates: HashMap<&str, Value>, transaction_id: Option<usize>) -> Result<V, StorageError>
    where
        V: Serialize + DeserializeOwned + Clone,
    {
        // 1. Fetch the existing value from the database
        let value: Option<V> = self.get(id)?;

        if let Some(value) = value {
            // 2. Convert the existing value into a JSON object
            let mut json_value = serde_json::to_value(&value).map_err(|_| StorageError::SerializationError)?;

            // 3. Apply the updates
            if let Some(json_object) = json_value.as_object_mut() {
                for (key, update) in updates {
                    json_object.insert(key.to_string(), update);
                }
            } else {
                return Err(StorageError::SerializationError);
            }

            // 4. Convert the updated JSON object back to V
            let updated_value: V = serde_json::from_value(json_value).map_err(|_| StorageError::SerializationError)?;

            // 5. Save the updated value back to the database
            self.set(id, updated_value.clone(), transaction_id)?;

            Ok(updated_value)
        } else {
            Err(StorageError::NotFound)
        }
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
    use std::fs;

    fn temp_storage() -> PathBuf {
        let dir = env::temp_dir();
        let mut rng = thread_rng();
        let index = rng.next_u32();
        dir.join(format!("storage_{}.db", index))
    }

    fn cleanup_storage(path: &PathBuf) {
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn test_01_new_storage_starts_empty() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        assert!(fs.is_empty());
        cleanup_storage(path);
    }

    #[test]
    fn test_02_add_value_to_storage() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test", "test_value");
        assert_eq!(fs.read("test").unwrap(), Some("test_value".to_string()));
        cleanup_storage(path);
    }

    #[test]
    fn test_03_read_a_value() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test", "test_value");
        assert_eq!(fs.read("test").unwrap(), Some("test_value".to_string()));
        cleanup_storage(path);
    }

    #[test]
    fn test_04_delete_value() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test", "test_value");
        assert_eq!(fs.read("test").unwrap(), Some("test_value".to_string()));
        let _ = fs.delete("test");
        assert_eq!(fs.read("test").unwrap(), None);
        cleanup_storage(path);
    }

    #[test]
    fn test_05_find_multiple_answers() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
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

        cleanup_storage(path);
    }

    #[test]
    fn test_06_has_key() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test1", "test_value1");
        assert!(fs.has_key("test1").unwrap());
        assert!(!fs.has_key("test2").unwrap());
        cleanup_storage(path);
    }

    #[test]
    fn test_07_open_storage() {
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test1", "test_value1");

        drop(fs);

        let fs2 = Storage::open(&path);
        assert!(fs2.is_ok());
        assert_eq!(fs2.unwrap().read("test1").unwrap(), Some("test_value1".to_string()));

        cleanup_storage(path);
    }

    #[test]
    fn test_08_open_inexistent_storage() {
        let path = temp_storage();
        let fs = Storage::open(&path);
        assert!(fs.is_err());
    }

    #[test]
    fn test_09_keys(){
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test1", "test_value1");
        let _ = fs.write("test2", "test_value2");
        let _ = fs.write("test3", "test_value3");
        let _ = fs.write("tes4", "test_value4");

        let keys = fs.keys().unwrap();
        assert_eq!(keys.len(), 4);
        assert!(keys.contains(&"test1".to_string()));
        assert!(keys.contains(&"test2".to_string()));
        assert!(keys.contains(&"test3".to_string()));
        assert!(keys.contains(&"tes4".to_string()));

        cleanup_storage(path);
    }

    #[test]
    fn test_10_transaction_commit(){
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let transaction_id = fs.begin_transaction();
        fs.transactional_write("test1", "test_value1", transaction_id).unwrap();
        fs.transactional_write("test2", "test_value2", transaction_id).unwrap();
        fs.commit_transaction(transaction_id).unwrap();

        assert_eq!(fs.read("test1").unwrap(), Some("test_value1".to_string()));
        assert_eq!(fs.read("test2").unwrap(), Some("test_value2".to_string()));
        assert_eq!(fs.read("test3").unwrap(), None);

        cleanup_storage(path);
    }

    #[test]
    fn test_11_transaction_rollback(){
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let transaction_id = fs.begin_transaction();
        fs.transactional_write("test1", "test_value1", transaction_id).unwrap();
        fs.transactional_write("test2", "test_value2", transaction_id).unwrap();
        fs.rollback_transaction(transaction_id).unwrap();

        assert_eq!(fs.read("test1").unwrap(), None);
        assert_eq!(fs.read("test2").unwrap(), None);

        cleanup_storage(path);
    }

    #[test]
    fn test_12_transactional_delete(){
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let _ = fs.write("test1", "test_value1");
        let transaction_id = fs.begin_transaction();
        fs.transactional_delete("test1", transaction_id).unwrap();
        fs.commit_transaction(transaction_id).unwrap();

        assert_eq!(fs.read("test1").unwrap(), None);

        cleanup_storage(path);
    }

    #[test]
    fn test_13_non_commited_transactions_should_not_appear(){
        let path = &temp_storage();
        let fs = Storage::new_with_path(path).unwrap();
        let transaction_id = fs.begin_transaction();
        fs.transactional_write("test1", "test_value1", transaction_id).unwrap();
        fs.transactional_write("test2", "test_value2", transaction_id).unwrap();
        fs.commit_transaction(transaction_id).unwrap();

        let second_transaction_id = fs.begin_transaction();
        fs.transactional_write("test3", "test_value3", second_transaction_id).unwrap();

        assert_eq!(fs.read("test1").unwrap(), Some("test_value1".to_string()));
        assert_eq!(fs.read("test2").unwrap(), Some("test_value2".to_string()));
        assert_eq!(fs.read("test3").unwrap(), None);
        fs.rollback_transaction(transaction_id).unwrap();

        cleanup_storage(path);
    }
}