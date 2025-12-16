use crate::{backup_io::{BackupFileReader, BackupFileWriter}, error::StorageError, password_policy::PasswordPolicy, storage_config::{PasswordPolicyConfig, StorageConfig}};
use cocoon::Cocoon;
use rand::{rngs::OsRng, TryRngCore};
use rocksdb::TransactionDB;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Cursor, Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const DEK_KEY: &str = "DEK";

/// Storage is limited to single threaded access due to the use of RefCell for transaction management.
pub struct Storage {
    db: rocksdb::TransactionDB,
    transactions: RefCell<HashMap<Uuid, Box<rocksdb::Transaction<'static, TransactionDB>>>>,
    password: Option<Vec<u8>>,
    password_policy: PasswordPolicy,
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
    pub fn new_with_policy(
        config: &StorageConfig,
        password_policy_config: Option<PasswordPolicyConfig>,
    ) -> Result<Storage, StorageError> {
        let mut options = create_options();
        options.create_if_missing(true);
        Self::open_db(config, password_policy_config, &options)
    }

    pub fn open_with_policy(
        config: &StorageConfig,
        password_policy_config: Option<PasswordPolicyConfig>,
    ) -> Result<Storage, StorageError> {
        let options = create_options();
        Self::open_db(config, password_policy_config, &options)
    }

    pub fn new(config: &StorageConfig) -> Result<Storage, StorageError> {
        let mut options = create_options();
        options.create_if_missing(true);
        Self::open_db(config, None, &options)
    }

    pub fn open(config: &StorageConfig) -> Result<Storage, StorageError> {
        let options = create_options();
        Self::open_db(config, None, &options)
    }

    fn open_db(
        config: &StorageConfig,
        password_policy_config: Option<PasswordPolicyConfig>,
        options: &rocksdb::Options,
    ) -> Result<Storage, StorageError> {
        let db = rocksdb::TransactionDB::open(
            options,
            &rocksdb::TransactionDBOptions::default(),
            config.path.as_str(),
        )?;

        let password_policy = if let Some(ref policy) = password_policy_config {
                PasswordPolicy::new(policy.clone())
            } else {
                PasswordPolicy::default()
            };

        let dek = if let Some(ref password) = config.password {

            if !password_policy.is_valid(password) {
                return Err(StorageError::WeakPassword(password_policy));
            }
            let dek = match db.get(DEK_KEY).map_err(|_| StorageError::ReadError)? {
                Some(encrypted_dek) => {
                    let mut entry_cursor = Cursor::new(encrypted_dek);

                    let cocoon = Cocoon::new(password.as_bytes());
                    let dek = cocoon
                        .parse(&mut entry_cursor)
                        .map_err(|_| StorageError::WrongPassword)?;

                    dek
                }
                None => {
                    let mut bytes = [0u8; 32];
                    OsRng.try_fill_bytes(&mut bytes)?;

                    let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
                    let mut cocoon = Cocoon::new(password.as_bytes());
                    cocoon
                        .dump(bytes.to_vec(), &mut entry_cursor)
                        .map_err(|error| StorageError::FailedToEncryptData { error })?;
                    let encrypted_dek = entry_cursor.into_inner();
                    db.put(DEK_KEY.as_bytes(), encrypted_dek)
                        .map_err(|_| StorageError::WriteError)?;
                    bytes.to_vec()
                }
            };

            Some(dek)
        } else {
            None
        };

        Ok(Storage {
            db,
            transactions: RefCell::new(HashMap::new()),
            password: dek,
            password_policy,
        })
    }

    pub fn change_password(
        &self,
        old_password: String,
        new_password: String,
    ) -> Result<(), StorageError> {
        match &self.password {
            Some(_) => {
                if !self.password_policy.is_valid(&new_password) {
                    return Err(StorageError::WeakPassword(self.password_policy.clone()));
                }
            }
            None => return Err(StorageError::NoPasswordSet),
        }

        let dek = match self.db.get(DEK_KEY).map_err(|_| StorageError::ReadError)? {
            Some(encrypted_dek) => {
                let mut entry_cursor = Cursor::new(encrypted_dek);

                let cocoon = Cocoon::new(old_password.as_bytes());
                let dek = cocoon
                    .parse(&mut entry_cursor)
                    .map_err(|_| StorageError::WrongPassword)?;

                dek
            }
            None => return Err(StorageError::NotFound("DEK".to_string())),
        };

        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(new_password.as_bytes());
        cocoon
            .dump(dek, &mut entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        let encrypted_dek = entry_cursor.into_inner();
        self.db
            .put(DEK_KEY.as_bytes(), encrypted_dek)
            .map_err(|_| StorageError::WriteError)?;

        Ok(())
    }

    pub fn change_backup_password<P: AsRef<Path>>(&self, dek_path: &P, old_password: String, new_password: String) -> Result<(), StorageError> {
        if !self.password_policy.is_valid(&new_password) {
            return Err(StorageError::WeakPassword(self.password_policy.clone()));
        }

        let mut dek_file = File::open(dek_path)?;
        let mut buf = Vec::new();
        dek_file.read_to_end(&mut buf)?;

        let mut entry_cursor = Cursor::new(buf);

        let cocoon = Cocoon::new(old_password.as_bytes());
        let dek = cocoon
            .parse(&mut entry_cursor)
            .map_err(|_| StorageError::WrongPassword)?;

        let mut new_entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut new_cocoon = Cocoon::new(new_password.as_bytes());
        new_cocoon
            .dump(dek, &mut new_entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        let encrypted_dek = new_entry_cursor.into_inner();

        let mut dek_file = File::create(dek_path)?;
        dek_file.write_all(&encrypted_dek)?;

        Ok(())
    }

    pub fn restore_backup<P: AsRef<Path>>(&self, backup_path: &P, dek_path: &P, password: String) -> Result<(), StorageError> {
        let backup_file = File::open(backup_path)?;
        let backup_file = BufReader::new(backup_file);
        let mut dek_file = File::open(dek_path)?;
        let mut buf = Vec::new();
        let transaction_id = self.begin_transaction();
        let result: Result<(), StorageError> = {
            let mut encrypted_dek = Vec::new();
            dek_file.read_to_end(&mut encrypted_dek)?;
            let mut entry_cursor = Cursor::new(encrypted_dek);

            let cocoon = Cocoon::new(password.as_bytes());
            let dek = cocoon
                .parse(&mut entry_cursor)
                .map_err(|_| StorageError::WrongPassword)?;

            let mut backup_reader = BackupFileReader::new(backup_file, dek)?;

            while backup_reader.read_until(b';', &mut buf)? != 0 {
                buf.pop();
                let mut parts = buf.splitn(2, |&b| b == b',');
                if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
                    let key = String::from_utf8(key.to_vec())
                        .map_err(|_| StorageError::ConversionError)?;
                    let value = String::from_utf8(value.to_vec())
                        .map_err(|_| StorageError::ConversionError)?;
                    let key = hex::decode(key).map_err(|_| StorageError::ConversionError)?;
                    let value = hex::decode(value).map_err(|_| StorageError::ConversionError)?;

                    let mut map = self.transactions.borrow_mut();
                    let tx = map
                        .get_mut(&transaction_id)
                        .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                    tx.put(&key, &value).map_err(|_| StorageError::WriteError)?;
                }
                buf.clear();
            }
            Ok(())
        };

        if result.is_err() {
            self.rollback_transaction(transaction_id)?;
        } else {
            self.commit_transaction(transaction_id)?;
        }

        result
    }

    pub fn backup<P: AsRef<Path>>(&self, backup_path: P, dek_path: P, password: String) -> Result<(), StorageError> {
        if !self.password_policy.is_valid(&password) {
            return Err(StorageError::WeakPassword(self.password_policy.clone()));
        }

        let snapshot = self.db.snapshot();
        let mut iter = snapshot.iterator(rocksdb::IteratorMode::Start);
        let backup_file = File::create(backup_path)?;
        let mut dek_file = File::create(dek_path)?;
        let mut data_vec = Vec::new();
        let mut item_counter = 0;

        let mut dek = [0u8; 32];
        OsRng.try_fill_bytes(&mut dek)?;

        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(password.as_bytes());
        cocoon
            .dump(dek.to_vec(), &mut entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        let encrypted_dek = entry_cursor.into_inner();
        dek_file.write_all(&encrypted_dek)?;

        let mut backup_writer = BackupFileWriter::new(backup_file, dek.to_vec())?;

        while let Some(Ok((k, v))) = iter.next() {
            data_vec.push((k.to_vec(), v.to_vec()));

            if item_counter == 1000 {
                let mut serialized_data = String::new();
                for (key, value) in &data_vec {
                    let key = hex::encode(key);
                    let value = hex::encode(value);
                    serialized_data.push_str(&format!("{},{};", key, value));
                }
                backup_writer.write_all(serialized_data.as_bytes())?;
                item_counter = 0;
                data_vec.clear();
            } else {
                item_counter += 1;
            }
        }

        if !data_vec.is_empty() {
            let mut serialized_data = String::new();
            for (key, value) in &data_vec {
                let key = hex::encode(key);
                let value = hex::encode(value);
                serialized_data.push_str(&format!("{},{};", key, value));
            }
            backup_writer.write_all(serialized_data.as_bytes())?;
        }

        backup_writer.finish()?;

        Ok(())
    }

    pub fn delete_db_files(storage: Storage) -> Result<(), StorageError> {
        let path = PathBuf::from(storage.db.path());
        drop(storage);
        fs::remove_dir_all(path)?;
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
        let tx = map
            .get_mut(&transaction_id)
            .ok_or(StorageError::NotFound("Transaction".to_string()))?;
        tx.delete(key.as_bytes())
            .map_err(|_| StorageError::WriteError)?;

        Ok(())
    }

    pub fn write(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let tx = self.db.transaction();
        let mut data = value.as_bytes().to_vec();

        if self.password.is_some() {
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
        let tx = map
            .get_mut(&transaction_id)
            .ok_or(StorageError::NotFound("Transaction".to_string()))?;
        let mut data = value.as_bytes().to_vec();

        if self.password.is_some() {
            data = self.encrypt_data(data)?
        }

        tx.put(key.as_bytes(), data)
            .map_err(|_| StorageError::WriteError)?;

        Ok(())
    }

    pub fn read(&self, key: &str) -> Result<Option<String>, StorageError> {
        match self.db.get(key.as_bytes()) {
            Ok(Some(mut data)) => {
                if self.password.is_some() {
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
            let v = if self.password.is_some() {
                self.decrypt_data(v.to_vec())?
            } else {
                v.to_vec()
            };
            let v = String::from_utf8(v).map_err(|_| StorageError::ConversionError)?;
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
    
    /// # Safety
    /// This method uses `std::mem::transmute` to extend the transaction's lifetime to `'static`,
    /// which is safe in this context because all transactions are stored in a `RefCell` within the `Storage` struct,
    /// and are only accessed from the same thread.
    /// Ensure that all transactions are properly committed or rolled back to avoid resource leaks.
    pub fn begin_transaction(&self) -> Uuid {
        let transaction = self.db.transaction();
        let mut map = self.transactions.borrow_mut();
        let id = Uuid::new_v4();
        map.insert(
            id,
            Box::new(unsafe {
                std::mem::transmute::<rocksdb::Transaction<'_, TransactionDB>, rocksdb::Transaction<'static, TransactionDB>>(transaction)
            }),
        );
        id
    }

    pub fn commit_transaction(&self, transaction_id: Uuid) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        let tx = map
            .remove(&transaction_id)
            .ok_or(StorageError::NotFound("Transaction".to_string()))?;
        tx.commit().map_err(|_| StorageError::CommitError)?;

        Ok(())
    }

    pub fn rollback_transaction(&self, transaction_id: Uuid) -> Result<(), StorageError> {
        let mut map = self.transactions.borrow_mut();
        map.remove(&transaction_id)
            .ok_or(StorageError::NotFound("Transaction".to_string()))?;
        Ok(())
    }

    fn encrypt_data(&self, data: Vec<u8>) -> Result<Vec<u8>, StorageError> {
        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(self.password.as_ref().unwrap());
        cocoon
            .dump(data, &mut entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        Ok(entry_cursor.into_inner())
    }

    fn decrypt_data(&self, data: Vec<u8>) -> Result<Vec<u8>, StorageError> {
        let mut entry_cursor = Cursor::new(data);

        let cocoon = Cocoon::new(self.password.as_ref().unwrap());
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
            Err(StorageError::NotFound("Value".to_string()))
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
    use crate::storage_config::PasswordPolicyConfig;
    use rand::{rng, RngCore};
    use std::env;

    fn temp_storage() -> PathBuf {
        let dir = env::temp_dir();
        let mut rang = rng();
        let index = rang.next_u32();
        dir.join(format!("storage_{}.db", index))
    }

    fn temp_backup() -> (PathBuf, PathBuf) {
        let dir = env::temp_dir();
        let mut rang = rng();
        let index = rang.next_u32();
        (dir.join(format!("backup_{}", index)), dir.join(format!("dek_{}", index)))
    }

    fn create_path_and_storage(
        is_encrypted: bool,
    ) -> Result<(PathBuf, StorageConfig, Storage), StorageError> {
        let path = &temp_storage();

        let password = if is_encrypted {
            Some("password".to_string())
        } else {
            None
        };

        let config = StorageConfig {
            path: path.to_string_lossy().to_string(),
            password,
        };
        
        let storage = Storage::new_with_policy(
            &config,
            Some(PasswordPolicyConfig {
                min_length: 1,
                min_number_of_special_chars: 0,
                min_number_of_uppercase: 0,
                min_number_of_digits: 0,
            }),
        )?;

        Ok((path.clone(), config, storage))
    }

    #[test]
    fn test_new_storage_starts_empty() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        assert!(store.is_empty());
        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_add_value_to_storage() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value")?;
        assert_eq!(store.read("test").unwrap(), Some("test_value".to_string()));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_read_a_value() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value")?;
        assert_eq!(store.read("test")?, Some("test_value".to_string()));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_delete_value() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value")?;
        assert_eq!(store.read("test")?, Some("test_value".to_string()));
        store.delete("test")?;
        assert_eq!(store.read("test")?, None);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_find_multiple_answers() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
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

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_has_key() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        assert!(store.has_key("test1")?);
        assert!(!store.has_key("test2")?);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_open_storage() -> Result<(), StorageError> {
        let (_, config, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        drop(store);

        let open_store = Storage::open(&config);
        assert!(open_store.is_ok());
        assert_eq!(
            open_store.as_ref().unwrap().read("test1")?,
            Some("test_value1".to_string())
        );

        Storage::delete_db_files(open_store.unwrap())?;
        Ok(())
    }

    #[test]
    fn test_open_inexistent_storage() -> Result<(), StorageError> {
        let path = &temp_storage();

        let config = StorageConfig {
            path: path.to_string_lossy().to_string(),
            password: Some("password".to_string()),
        };
        let open_store = Storage::open(&config);
        assert!(open_store.is_err());
        Ok(())
    }

    #[test]
    fn test_keys() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
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

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_transaction_commit() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store.transactional_write("test1", "test_value1", transaction_id)?;
        store.transactional_write("test2", "test_value2", transaction_id)?;
        store.commit_transaction(transaction_id)?;

        assert_eq!(store.read("test1")?, Some("test_value1".to_string()));
        assert_eq!(store.read("test2")?, Some("test_value2".to_string()));
        assert_eq!(store.read("test3")?, None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_transaction_rollback() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store.transactional_write("test1", "test_value1", transaction_id)?;
        store.transactional_write("test2", "test_value2", transaction_id)?;
        store.rollback_transaction(transaction_id)?;

        assert_eq!(store.read("test1")?, None);
        assert_eq!(store.read("test2")?, None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_transactional_delete() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        let transaction_id = store.begin_transaction();
        store.transactional_delete("test1", transaction_id).unwrap();
        store.commit_transaction(transaction_id).unwrap();

        assert_eq!(store.read("test1").unwrap(), None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_non_commited_transactions_should_not_appear() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
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

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_encrypt_and_decrypt() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(true)?;
        store.set("test1", "test_value1", None)?;
        let data = store.get::<String, String>("test1".to_string())?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value1");

        store.set("test1", "test_value2", None)?;
        let data = store.get::<String, String>("test1".to_string())?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value2");

        Storage::delete_db_files(store)?;
        Ok(())
    }

    #[test]
    fn test_backup() -> Result<(), StorageError> {
        let (backup_path, dek_path) = temp_backup();
        let password = "password".to_string();
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        store.write("test2", "test_value2")?;
        store.backup(&backup_path, &dek_path, password)?;
        assert!(backup_path.exists());
        assert!(dek_path.exists());

        Ok(())
    }

    #[test]
    fn test_restore_backup() -> Result<(), StorageError> {
        let (backup_path, dek_path) = temp_backup();
        let password = "password".to_string();
        let (_, config, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1")?;
        store.write("test2", "test_value2")?;
        store.backup(&backup_path, &dek_path, password.clone())?;

        Storage::delete_db_files(store)?;
        let store = Storage::new(&config)?;
        store.restore_backup(&backup_path, &dek_path, password)?;

        assert_eq!(store.read("test1")?, Some("test_value1".to_string()));
        assert_eq!(store.read("test2")?, Some("test_value2".to_string()));

        Storage::delete_db_files(store)?;
        fs::remove_file(backup_path)?;
        fs::remove_file(dek_path)?;
        Ok(())
    }

    #[test]
    fn test_more_than_1000_values_to_backup() -> Result<(), StorageError> {
        let quantity = 1500;
        let (backup_path, dek_path) = temp_backup();
        let password = "password".to_string();
        let (_, config, store) = create_path_and_storage(false)?;
        for i in 0..quantity {
            store.write(&format!("test{}", i), &format!("test_value{}", i))?;
        }
        store.backup(&backup_path, &dek_path, password.clone())?;
        assert!(backup_path.exists());

        Storage::delete_db_files(store)?;

        let store = Storage::new(&config)?;
        store.restore_backup(&backup_path, &dek_path, password)?;

        for i in 0..quantity {
            assert_eq!(
                store.read(&format!("test{}", i))?,
                Some(format!("test_value{}", i).to_string())
            );
        }

        Storage::delete_db_files(store)?;
        fs::remove_file(backup_path)?;
        fs::remove_file(dek_path)?;
        Ok(())
    }

    #[test]
    fn test_change_password() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(true)?;
        store.set("test1", "test_value1", None)?;

        store.change_password("password".to_string(), "new_password".to_string())?;

        drop(store);

        let store = Storage::new_with_policy(&StorageConfig {
            path: path.to_string_lossy().to_string(),
            password: Some("new_password".to_string()),
            },
            Some(PasswordPolicyConfig {
                min_length: 1,
                min_number_of_special_chars: 0,
                min_number_of_uppercase: 0,
                min_number_of_digits: 0,
            }),
        )?;

        assert_eq!(
            store.get::<String, String>("test1".to_string())?,
            Some("test_value1".to_string())
        );
        Storage::delete_db_files(store)?;

        Ok(())
    }

    #[test]
    fn test_change_backup_password() -> Result<(), StorageError> {
        let (backup_path, dek_path) = temp_backup();
        let password = "password".to_string();
        let new_password = "new_password".to_string();
        let path = &temp_storage();
        
        let store = Storage::new_with_policy(&StorageConfig {
            path: path.to_string_lossy().to_string(),
            password: None
            },
            Some(PasswordPolicyConfig {
                min_length: 1,
                min_number_of_special_chars: 0,
                min_number_of_uppercase: 0,
                min_number_of_digits: 0,
            }),
        )?;

        store.write("test1", "test_value1")?;
        store.backup(&backup_path, &dek_path, password.clone())?;
        store.change_backup_password(&dek_path, password.clone(), new_password.clone())?;
        Storage::delete_db_files(store)?;

        let store = Storage::new_with_policy(&StorageConfig {
            path: path.to_string_lossy().to_string(),
            password: None
            },
            Some(PasswordPolicyConfig {
                min_length: 1,
                min_number_of_special_chars: 0,
                min_number_of_uppercase: 0,
                min_number_of_digits: 0,
            }),
        )?;

        store.restore_backup(&backup_path, &dek_path, new_password)?;

        assert_eq!(store.read("test1")?, Some("test_value1".to_string()));
        
        Storage::delete_db_files(store)?;
        fs::remove_file(backup_path)?;
        fs::remove_file(dek_path)?;
        Ok(())
    }
}
