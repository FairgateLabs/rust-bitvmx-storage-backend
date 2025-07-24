use crate::{error::StorageError, storage_config::StorageConfig};
use cocoon::Cocoon;
use rocksdb::TransactionDB;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Cursor, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub struct Storage {
    db: rocksdb::TransactionDB,
    transactions: RefCell<HashMap<Uuid, Box<rocksdb::Transaction<'static, TransactionDB>>>>,
    encrypt: Option<String>,
}

pub trait KeyValueStore {
    fn get<K, V>(&self, key: K) -> Result<Option<V>, StorageError>
    where
        K: AsRef<str>,
        V: DeserializeOwned;

    fn set<K, V>(&self, key: K, value: V, transaction_id: Option<Uuid>) -> Result<(), StorageError>
    where
        K: AsRef<str>,
        V: Serialize;

    fn update<K, V>(
        &self,
        id: K,
        updates: &HashMap<&str, Value>,
        transaction_id: Option<Uuid>,
    ) -> Result<V, StorageError>
    where
        K: AsRef<str> + std::marker::Copy,
        V: Serialize + DeserializeOwned + Clone;
}

impl Storage {
    pub fn new(config: &StorageConfig) -> Result<Storage, StorageError> {
        let mut options = create_options();
        options.create_if_missing(true);
        Self::open_db(config, &options)
    }

    pub fn open(config: &StorageConfig) -> Result<Storage, StorageError> {
        let options = create_options();
        Self::open_db(config, &options)
    }

    fn open_db(
        config: &StorageConfig,
        options: &rocksdb::Options,
    ) -> Result<Storage, StorageError> {
        let db = rocksdb::TransactionDB::open(
            options,
            &rocksdb::TransactionDBOptions::default(),
            config.path.as_str(),
        )?;

        Ok(Storage {
            db,
            transactions: RefCell::new(HashMap::new()),
            encrypt: config.encrypt.clone(),
        })
    }

    pub fn restore_backup<P: AsRef<Path>>(&self, backup_path: &P) -> Result<(), StorageError> {
        let file = File::open(backup_path)?;
        let mut file = BufReader::new(file);
        let mut buf = Vec::new();
        while file.read_until(b';', &mut buf)? != 0 {
            buf.pop();
            let mut parts = buf.splitn(2, |&b| b == b',');
            if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
                let key =
                    String::from_utf8(key.to_vec()).map_err(|_| StorageError::ConversionError)?;
                let value =
                    String::from_utf8(value.to_vec()).map_err(|_| StorageError::ConversionError)?;
                let key = hex::decode(key).map_err(|_| StorageError::ConversionError)?;
                let value = hex::decode(value).map_err(|_| StorageError::ConversionError)?;

                self.db
                    .put(key, value)
                    .map_err(|_| StorageError::WriteError)?;
            }
            buf.clear();
        }

        Ok(())
    }

    pub fn backup<P: AsRef<Path>>(&self, backup_path: P) -> Result<(), StorageError> {
        let snapshot = self.db.snapshot();
        let mut iter = snapshot.iterator(rocksdb::IteratorMode::Start);
        let mut file = File::create(backup_path)?;
        let mut vec = Vec::new();
        let mut item_counter = 0;
        while let Some(Ok((k, v))) = iter.next() {
            vec.push((k.to_vec(), v.to_vec()));

            if item_counter == 1000 {
                let mut serialized_data = String::new();
                for (key, value) in &vec {
                    let key = hex::encode(key);
                    let value = hex::encode(value);
                    serialized_data.push_str(&format!("{},{};", key, value));
                }
                file.write_all(serialized_data.as_bytes())?;
                item_counter = 0;
                vec.clear();
            } else {
                item_counter += 1;
            }
        }

        if !vec.is_empty() {
            let mut serialized_data = String::new();
            for (key, value) in &vec {
                let key = hex::encode(key);
                let value = hex::encode(value);
                serialized_data.push_str(&format!("{},{};", key, value));
            }
            file.write_all(serialized_data.as_bytes())?;
        }

        Ok(())
    }

    pub fn delete_db_files(path: &PathBuf) -> Result<(), StorageError> {
        fs::remove_dir_all(path)?;
        Ok(())
    }

    pub fn delete_backup_file(backup_path: PathBuf) -> Result<(), StorageError> {
        fs::remove_file(backup_path)?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        let tx = self.db.transaction();
        tx.delete(key.as_bytes())
            .map_err(|_| StorageError::WriteError)?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn transactional_delete(
        &self,
        key: &str,
        transaction_id: Uuid,
    ) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map.get_mut(&transaction_id).ok_or(StorageError::NotFound)?;
        tx.delete(key.as_bytes())
            .map_err(|_| StorageError::WriteError)?;

        Ok(())
    }

    pub fn write(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let tx = self.db.transaction();
        let mut data = value.as_bytes().to_vec();

        if self.encrypt.is_some() {
            data = self.encrypt_data(data)?
        }

        tx.put(key.as_bytes(), data)
            .map_err(|_| StorageError::WriteError)?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn transactional_write(
        &self,
        key: &str,
        value: &str,
        transaction_id: Uuid,
    ) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map.get_mut(&transaction_id).ok_or(StorageError::NotFound)?;
        let mut data = value.as_bytes().to_vec();

        if self.encrypt.is_some() {
            data = self.encrypt_data(data)?
        }

        tx.put(key.as_bytes(), data)
            .map_err(|_| StorageError::WriteError)?;

        Ok(())
    }

    pub fn read(&self, key: &str) -> Result<Option<String>, StorageError> {
        match self.db.get(key.as_bytes()) {
            Ok(Some(mut data)) => {
                if self.encrypt.is_some() {
                    data = self.decrypt_data(data)?;
                }

                let data_ret =
                    String::from_utf8(data).map_err(|_| StorageError::ConversionError)?;
                Ok(Some(data_ret))
            }
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

    pub fn partial_compare_keys(&self, key: &str) -> Result<Vec<String>, StorageError> {
        let mut result = Vec::new();
        let mut iter = self.db.iterator(rocksdb::IteratorMode::From(
            key.as_bytes(),
            rocksdb::Direction::Forward,
        ));
        while let Some(Ok((k, _))) = iter.next() {
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            if k.starts_with(key) {
                result.push(k);
            } else {
                break;
            }
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

    pub fn begin_transaction(&self) -> Uuid {
        let transaction = self.db.transaction();
        let mut map = self.transactions.borrow_mut();
        let id = Uuid::new_v4();
        map.insert(
            id,
            Box::new(unsafe {
                std::mem::transmute::<_, rocksdb::Transaction<'static, TransactionDB>>(transaction)
            }),
        );
        id
    }

    pub fn commit_transaction(&self, transaction_id: Uuid) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map.remove(&transaction_id).ok_or(StorageError::NotFound)?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn rollback_transaction(&self, transaction_id: Uuid) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        map.remove(&transaction_id).ok_or(StorageError::NotFound)?;
        Ok(())
    }

    fn encrypt_data(&self, data: Vec<u8>) -> Result<Vec<u8>, StorageError> {
        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(self.encrypt.as_ref().unwrap().as_bytes());
        cocoon
            .dump(data, &mut entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        Ok(entry_cursor.into_inner())
    }

    fn decrypt_data(&self, data: Vec<u8>) -> Result<Vec<u8>, StorageError> {
        let mut entry_cursor = Cursor::new(data);

        let cocoon = Cocoon::new(self.encrypt.as_ref().unwrap().as_bytes());
        cocoon
            .parse(&mut entry_cursor)
            .map_err(|error| StorageError::FailedToDecryptData { error })
    }
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
                let value =
                    serde_json::from_str(&value).map_err(|_| StorageError::ConversionError)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn set<K, V>(&self, key: K, value: V, transaction_id: Option<Uuid>) -> Result<(), StorageError>
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

    fn update<K, V>(
        &self,
        id: K,
        updates: &HashMap<&str, Value>,
        transaction_id: Option<Uuid>,
    ) -> Result<V, StorageError>
    where
        K: AsRef<str> + std::marker::Copy,
        V: Serialize + DeserializeOwned + Clone,
    {
        // 1. Fetch the existing value from the database
        let value: Option<V> = self.get(id)?;

        if let Some(value) = value {
            // 2. Convert the existing value into a JSON object
            let mut json_value =
                serde_json::to_value(&value).map_err(|_| StorageError::SerializationError)?;

            // 3. Apply the updates
            if let Some(json_object) = json_value.as_object_mut() {
                for (key, update) in updates {
                    json_object.insert(key.to_string(), update.clone());
                }
            } else {
                return Err(StorageError::SerializationError);
            }

            // 4. Convert the updated JSON object back to V
            let updated_value: V =
                serde_json::from_value(json_value).map_err(|_| StorageError::SerializationError)?;

            // 5. Save the updated value back to the database
            self.set(id, updated_value.clone(), transaction_id)?;

            Ok(updated_value)
        } else {
            Err(StorageError::NotFound)
        }
    }
}

fn create_options() -> rocksdb::Options {
    let options = rocksdb::Options::default();
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rng, RngCore};
    use std::env;

    fn temp_storage() -> PathBuf {
        let dir = env::temp_dir();
        let mut rang = rng();
        let index = rang.next_u32();
        dir.join(format!("storage_{}.db", index))
    }

    fn create_path_and_storage(
        is_encrypted: bool,
    ) -> Result<(PathBuf, StorageConfig, Storage), StorageError> {
        let path = &temp_storage();

        let encrypt = if is_encrypted {
            Some("password".to_string())
        } else {
            None
        };

        let config = StorageConfig {
            path: path.to_string_lossy().to_string(),
            encrypt,
        };
        let storage = Storage::new(&config)?;

        Ok((path.clone(), config, storage))
    }

    fn delete_storage(path: &PathBuf, storage: Storage) -> Result<(), StorageError> {
        drop(storage);
        Storage::delete_db_files(path)?;
        Ok(())
    }

    #[test]
    fn test_new_storage_starts_empty() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        assert!(store.is_empty());
        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_add_value_to_storage() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value")?;
        assert_eq!(store.read("test").unwrap(), Some("test_value".to_string()));
        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_read_a_value() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value")?;
        assert_eq!(store.read("test")?, Some("test_value".to_string()));
        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_delete_value() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value")?;
        assert_eq!(store.read("test")?, Some("test_value".to_string()));
        store.delete("test")?;
        assert_eq!(store.read("test")?, None);
        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_find_multiple_answers() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        store.write("test2", "test_value2")?;
        store.write("test3", "test_value3")?;
        store.write("tes4", "test_value4")?;

        let result = store.partial_compare("test")?;
        assert_eq!(
            result,
            vec![
                ("test1".to_string(), "test_value1".to_string()),
                ("test2".to_string(), "test_value2".to_string()),
                ("test3".to_string(), "test_value3".to_string())
            ]
        );

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_has_key() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        assert!(store.has_key("test1")?);
        assert!(!store.has_key("test2")?);
        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_open_storage() -> Result<(), StorageError> {
        let (path, config, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        drop(store);

        let open_store = Storage::open(&config);
        assert!(open_store.is_ok());
        assert_eq!(
            open_store.as_ref().unwrap().read("test1")?,
            Some("test_value1".to_string())
        );

        delete_storage(&path, open_store.unwrap())?;
        Ok(())
    }

    #[test]
    fn test_open_inexistent_storage() -> Result<(), StorageError> {
        let path = &temp_storage();
        let config = StorageConfig {
            path: path.to_string_lossy().to_string(),
            encrypt: Some("password".to_string()),
        };
        let open_store = Storage::open(&config);
        assert!(open_store.is_err());
        Ok(())
    }

    #[test]
    fn test_keys() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        store.write("test2", "test_value2")?;
        store.write("test3", "test_value3")?;
        store.write("tes4", "test_value4")?;

        let keys = store.keys()?;
        assert_eq!(keys.len(), 4);
        assert!(keys.contains(&"test1".to_string()));
        assert!(keys.contains(&"test2".to_string()));
        assert!(keys.contains(&"test3".to_string()));
        assert!(keys.contains(&"tes4".to_string()));

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_transaction_commit() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store.transactional_write("test1", "test_value1", transaction_id)?;
        store.transactional_write("test2", "test_value2", transaction_id)?;
        store.commit_transaction(transaction_id)?;

        assert_eq!(store.read("test1")?, Some("test_value1".to_string()));
        assert_eq!(store.read("test2")?, Some("test_value2".to_string()));
        assert_eq!(store.read("test3")?, None);

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_transaction_rollback() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store.transactional_write("test1", "test_value1", transaction_id)?;
        store.transactional_write("test2", "test_value2", transaction_id)?;
        store.rollback_transaction(transaction_id)?;

        assert_eq!(store.read("test1")?, None);
        assert_eq!(store.read("test2")?, None);

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_transactional_delete() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        let transaction_id = store.begin_transaction();
        store.transactional_delete("test1", transaction_id).unwrap();
        store.commit_transaction(transaction_id).unwrap();

        assert_eq!(store.read("test1").unwrap(), None);

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_non_commited_transactions_should_not_appear() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store
            .transactional_write("test1", "test_value1", transaction_id)
            .unwrap();
        store
            .transactional_write("test2", "test_value2", transaction_id)
            .unwrap();
        store.commit_transaction(transaction_id).unwrap();

        let second_transaction_id = store.begin_transaction();
        store
            .transactional_write("test3", "test_value3", second_transaction_id)
            .unwrap();

        assert_eq!(
            store.read("test1").unwrap(),
            Some("test_value1".to_string())
        );
        assert_eq!(
            store.read("test2").unwrap(),
            Some("test_value2".to_string())
        );
        assert_eq!(store.read("test3").unwrap(), None);
        store.rollback_transaction(second_transaction_id).unwrap();

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_encrypt_and_decrypt() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(true)?;
        store.set("test1", "test_value1", None)?;
        let data = store.get::<String, String>("test1".to_string())?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value1");

        store.set("test1", "test_value2", None)?;
        let data = store.get::<String, String>("test1".to_string())?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value2");

        delete_storage(&path, store)?;
        Ok(())
    }

    #[test]
    fn test_backup() -> Result<(), StorageError> {
        let backup_path = temp_storage();
        let (path, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        store.write("test2", "test_value2")?;
        store.backup(backup_path.clone())?;
        assert!(backup_path.exists());

        delete_storage(&path, store)?;
        Storage::delete_backup_file(backup_path).unwrap();
        Ok(())
    }

    #[test]
    fn test_restore_backup() -> Result<(), StorageError> {
        let backup_path = temp_storage();
        let (path, config, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        store.write("test2", "test_value2")?;
        store.backup(backup_path.clone())?;

        delete_storage(&path, store)?;
        let store = Storage::new(&config)?;
        store.restore_backup(&backup_path)?;

        assert_eq!(store.read("test1")?, Some("test_value1".to_string()));
        assert_eq!(store.read("test2")?, Some("test_value2".to_string()));

        delete_storage(&path, store)?;
        Storage::delete_backup_file(backup_path).unwrap();
        Ok(())
    }

    #[test]
    fn test_more_than_1000_values_to_backup() -> Result<(), StorageError> {
        let quantity = 1500;
        let backup_path = temp_storage();
        let (path, config, store) = create_path_and_storage(false)?;
        for i in 0..quantity {
            store.write(&format!("test{}", i), &format!("test_value{}", i))?;
        }
        store.backup(backup_path.clone())?;
        assert!(backup_path.exists());

        delete_storage(&path, store)?;

        let store = Storage::new(&config)?;
        store.restore_backup(&backup_path.clone())?;

        for i in 0..quantity {
            assert_eq!(
                store.read(&format!("test{}", i))?,
                Some(format!("test_value{}", i).to_string())
            );
        }

        delete_storage(&path, store)?;
        Storage::delete_backup_file(backup_path).unwrap();
        Ok(())
    }
}
