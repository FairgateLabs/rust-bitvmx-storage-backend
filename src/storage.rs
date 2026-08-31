use crate::{
    backup_io::{BackupFileReader, BackupFileWriter},
    error::StorageError,
    password_policy::PasswordPolicy,
    storage_config::{PasswordPolicyConfig, StorageConfig},
};
use cocoon::{Cocoon, MiniCocoon};
use rand::{rngs::SysRng, TryRng};
use redact::Secret;
use rocksdb::{TransactionDB, WriteOptions};
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
const GLOBAL_TRANSACTION_ID: Uuid = Uuid::nil();

/// Storage is limited to single threaded access due to the use of RefCell for transaction management.
pub struct Storage {
    db: rocksdb::TransactionDB,
    transactions: RefCell<HashMap<Uuid, Box<rocksdb::Transaction<'static, TransactionDB>>>>,
    password: Option<Vec<u8>>,
    password_policy: PasswordPolicy,
}

pub trait KeyValueStore {
    fn get<K, V>(&self, key: K, transaction_id: Option<Uuid>) -> Result<Option<V>, StorageError>
    where
        K: AsRef<str>,
        V: DeserializeOwned;

    fn set<K, V>(&self, key: K, value: V, transaction_id: Option<Uuid>) -> Result<(), StorageError>
    where
        K: AsRef<str>,
        V: Serialize;

    fn remove<K>(&self, key: K, transaction_id: Option<Uuid>) -> Result<(), StorageError>
    where
        K: AsRef<str>;

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

// Clearing the transaction map is equivalent to rolling back all active transactions.
impl Drop for Storage {
    fn drop(&mut self) {
        let mut map = self.transactions.borrow_mut();
        map.clear();
    }
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
            if !password_policy.is_valid(password.expose_secret()) {
                return Err(StorageError::WeakPassword(password_policy));
            }
            let dek = match db.get(DEK_KEY).map_err(|_| StorageError::ReadError)? {
                Some(encrypted_dek) => {
                    let mut entry_cursor = Cursor::new(encrypted_dek);

                    let cocoon = Cocoon::new(password.expose_secret().as_bytes());

                    cocoon
                        .parse(&mut entry_cursor)
                        .map_err(|_| StorageError::WrongPassword)?
                }
                None => {
                    let mut bytes = [0u8; 32];
                    SysRng.try_fill_bytes(&mut bytes)?;

                    let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
                    let mut cocoon = Cocoon::new(password.expose_secret().as_bytes());
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
        old_password: Secret<String>,
        new_password: Secret<String>,
    ) -> Result<(), StorageError> {
        match &self.password {
            Some(_) => {
                if !self.password_policy.is_valid(new_password.expose_secret()) {
                    return Err(StorageError::WeakPassword(self.password_policy.clone()));
                }
            }
            None => return Err(StorageError::NoPasswordSet),
        }

        let dek = match self.db.get(DEK_KEY).map_err(|_| StorageError::ReadError)? {
            Some(encrypted_dek) => {
                let mut entry_cursor = Cursor::new(encrypted_dek);

                let cocoon = Cocoon::new(old_password.expose_secret().as_bytes());

                cocoon
                    .parse(&mut entry_cursor)
                    .map_err(|_| StorageError::WrongPassword)?
            }
            None => return Err(StorageError::NotFound("DEK".to_string())),
        };

        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(new_password.expose_secret().as_bytes());
        cocoon
            .dump(dek, &mut entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        let encrypted_dek = entry_cursor.into_inner();
        self.db
            .put(DEK_KEY.as_bytes(), encrypted_dek)
            .map_err(|_| StorageError::WriteError)?;

        Ok(())
    }

    pub fn change_backup_password<P: AsRef<Path>>(
        &self,
        dek_path: &P,
        old_password: Secret<String>,
        new_password: Secret<String>,
    ) -> Result<(), StorageError> {
        if !self.password_policy.is_valid(new_password.expose_secret()) {
            return Err(StorageError::WeakPassword(self.password_policy.clone()));
        }

        let mut dek_file = File::open(dek_path)?;
        let mut buf = Vec::new();
        dek_file.read_to_end(&mut buf)?;

        let mut entry_cursor = Cursor::new(buf);

        let cocoon = Cocoon::new(old_password.expose_secret().as_bytes());
        let dek = cocoon
            .parse(&mut entry_cursor)
            .map_err(|_| StorageError::WrongPassword)?;

        let mut new_entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut new_cocoon = Cocoon::new(new_password.expose_secret().as_bytes());
        new_cocoon
            .dump(dek, &mut new_entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        let encrypted_dek = new_entry_cursor.into_inner();

        let mut dek_file = File::create(dek_path)?;
        dek_file.write_all(&encrypted_dek)?;

        Ok(())
    }

    pub fn restore_backup<P: AsRef<Path>>(
        &self,
        backup_path: &P,
        dek_path: &P,
        password: Secret<String>,
    ) -> Result<(), StorageError> {
        let backup_file = File::open(backup_path)?;
        let backup_file = BufReader::new(backup_file);
        let mut dek_file = File::open(dek_path)?;
        let mut buf = Vec::new();
        let transaction_id = self.begin_transaction();
        let result: Result<(), StorageError> = {
            let mut encrypted_dek = Vec::new();
            dek_file.read_to_end(&mut encrypted_dek)?;
            let mut entry_cursor = Cursor::new(encrypted_dek);

            let cocoon = Cocoon::new(password.expose_secret().as_bytes());
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

    pub fn backup<P: AsRef<Path>>(
        &self,
        backup_path: P,
        dek_path: P,
        password: Secret<String>,
    ) -> Result<(), StorageError> {
        if !self.password_policy.is_valid(password.expose_secret()) {
            return Err(StorageError::WeakPassword(self.password_policy.clone()));
        }

        let snapshot = self.db.snapshot();
        let mut iter = snapshot.iterator(rocksdb::IteratorMode::Start);
        let backup_file = File::create(backup_path)?;
        let mut dek_file = File::create(dek_path)?;
        let mut data_vec = Vec::new();
        let mut item_counter = 0;

        let mut dek = [0u8; 32];
        SysRng.try_fill_bytes(&mut dek)?;

        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(password.expose_secret().as_bytes());
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

    fn delete(&self, key: &str, transaction_id: Option<Uuid>) -> Result<(), StorageError> {
        match transaction_id {
            Some(tx_id) => {
                let mut map = self.transactions.borrow_mut();
                let tx = map
                    .get_mut(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                tx.delete(key.as_bytes())
                    .map_err(|_| StorageError::WriteError)?;
            }
            None => {
                self.db
                    .delete(key.as_bytes())
                    .map_err(|_| StorageError::WriteError)?;
            }
        }

        Ok(())
    }

    fn write(
        &self,
        key: &str,
        value: &str,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        let mut data = value.as_bytes().to_vec();

        if self.password.is_some() {
            data = self.encrypt_data(data)?
        }

        match transaction_id {
            Some(tx_id) => {
                let mut map = self.transactions.borrow_mut();
                let tx = map
                    .get_mut(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                tx.put(key.as_bytes(), data)
                    .map_err(|_| StorageError::WriteError)?;
            }
            None => {
                self.db
                    .put(key.as_bytes(), data)
                    .map_err(|_| StorageError::WriteError)?;
            }
        }

        Ok(())
    }

    fn read(
        &self,
        key: &str,
        transaction_id: Option<Uuid>,
    ) -> Result<Option<String>, StorageError> {
        let data = match transaction_id {
            Some(tx_id) => {
                let map = self.transactions.borrow();
                let tx = map
                    .get(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                tx.get(key.as_bytes())
                    .map_err(|_| StorageError::ReadError)?
            }
            None => self
                .db
                .get(key.as_bytes())
                .map_err(|_| StorageError::ReadError)?,
        };

        match data {
            Some(mut data) => {
                if self.password.is_some() {
                    data = self.decrypt_data(data)?;
                }

                let data_ret =
                    String::from_utf8(data).map_err(|_| StorageError::ConversionError)?;
                Ok(Some(data_ret))
            }
            None => Ok(None),
        }
    }

    pub fn is_empty(&self, transaction_id: Option<Uuid>) -> Result<bool, StorageError> {
        match self.effective_transaction_id(transaction_id) {
            Some(tx_id) => {
                let map = self.transactions.borrow();
                let tx = map
                    .get(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                let result = tx.iterator(rocksdb::IteratorMode::Start).next().is_none();
                Ok(result)
            }
            None => Ok(self
                .db
                .iterator(rocksdb::IteratorMode::Start)
                .next()
                .is_none()),
        }
    }

    pub fn keys(&self, transaction_id: Option<Uuid>) -> Result<Vec<String>, StorageError> {
        let mut result = Vec::new();

        self.retrieve_partial_keys(
            None,
            None,
            &mut result,
            self.effective_transaction_id(transaction_id),
        )?;

        Ok(result)
    }

    pub fn partial_compare_keys(
        &self,
        key: &str,
        transaction_id: Option<Uuid>,
    ) -> Result<Vec<String>, StorageError> {
        self.partial_compare_keys_limited(key, None, transaction_id)
    }

    /// Prefix scan over keys, stopping after `limit` matches. `None` scans the whole range.
    pub fn partial_compare_keys_limited(
        &self,
        key: &str,
        limit: Option<usize>,
        transaction_id: Option<Uuid>,
    ) -> Result<Vec<String>, StorageError> {
        let mut result = Vec::new();

        self.retrieve_partial_keys(
            Some(key),
            limit,
            &mut result,
            self.effective_transaction_id(transaction_id),
        )?;

        Ok(result)
    }

    pub fn partial_compare(
        &self,
        key: &str,
        transaction_id: Option<Uuid>,
    ) -> Result<Vec<(String, String)>, StorageError> {
        self.partial_compare_limited(key, None, transaction_id)
    }

    /// Prefix scan stopping after `limit` entries. `None` scans the whole range.
    pub fn partial_compare_limited(
        &self,
        key: &str,
        limit: Option<usize>,
        transaction_id: Option<Uuid>,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let mut result = Vec::new();

        self.retrieve_partial_entries(
            key,
            limit,
            &mut result,
            self.effective_transaction_id(transaction_id),
        )?;

        Ok(result)
    }

    /// Prefix scan that deserializes each stored value into `V`. The typed analog of `partial_compare`.
    pub fn partial_get<V>(
        &self,
        key: &str,
        transaction_id: Option<Uuid>,
    ) -> Result<Vec<V>, StorageError>
    where
        V: DeserializeOwned,
    {
        self.partial_get_limited(key, None, transaction_id)
    }

    /// The typed analog of `partial_compare_limited`.
    pub fn partial_get_limited<V>(
        &self,
        key: &str,
        limit: Option<usize>,
        transaction_id: Option<Uuid>,
    ) -> Result<Vec<V>, StorageError>
    where
        V: DeserializeOwned,
    {
        let entries = self.partial_compare_limited(key, limit, transaction_id)?;
        let mut values = Vec::with_capacity(entries.len());
        for (_, value) in entries {
            let value = serde_json::from_str(&value).map_err(|_| StorageError::ConversionError)?;
            values.push(value);
        }
        Ok(values)
    }

    fn retrieve_partial_keys(
        &self,
        key: Option<&str>,
        limit: Option<usize>,
        result: &mut Vec<String>,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        match transaction_id {
            Some(tx_id) => {
                let map = self.transactions.borrow();
                let tx = map
                    .get(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;

                let iter = match key {
                    Some(k) => tx.iterator(rocksdb::IteratorMode::From(
                        k.as_bytes(),
                        rocksdb::Direction::Forward,
                    )),
                    None => tx.iterator(rocksdb::IteratorMode::Start),
                };

                self.collect_keys(iter, key, limit, result)?;
            }
            None => {
                let iter = match key {
                    Some(k) => self.db.iterator(rocksdb::IteratorMode::From(
                        k.as_bytes(),
                        rocksdb::Direction::Forward,
                    )),
                    None => self.db.iterator(rocksdb::IteratorMode::Start),
                };

                self.collect_keys(iter, key, limit, result)?;
            }
        };

        Ok(())
    }

    fn collect_keys<I>(
        &self,
        mut iter: I,
        key: Option<&str>,
        limit: Option<usize>,
        result: &mut Vec<String>,
    ) -> Result<(), StorageError>
    where
        I: Iterator<Item = Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>>,
    {
        if limit == Some(0) {
            return Ok(());
        }

        while let Some(Ok((k, _))) = iter.next() {
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            if let Some(prefix) = key {
                if !k.starts_with(prefix) {
                    break;
                }
            }
            result.push(k);
            if let Some(l) = limit {
                if result.len() >= l {
                    break;
                }
            }
        }
        Ok(())
    }

    fn retrieve_partial_entries(
        &self,
        key: &str,
        limit: Option<usize>,
        result: &mut Vec<(String, String)>,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        match transaction_id {
            Some(tx_id) => {
                let map = self.transactions.borrow();
                let tx = map
                    .get(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                let iter = tx.iterator(rocksdb::IteratorMode::From(
                    key.as_bytes(),
                    rocksdb::Direction::Forward,
                ));

                self.collect_entries(iter, key, limit, result)?;
            }
            None => {
                let iter = self.db.iterator(rocksdb::IteratorMode::From(
                    key.as_bytes(),
                    rocksdb::Direction::Forward,
                ));

                self.collect_entries(iter, key, limit, result)?;
            }
        };

        Ok(())
    }

    fn collect_entries<I>(
        &self,
        mut iter: I,
        prefix: &str,
        limit: Option<usize>,
        result: &mut Vec<(String, String)>,
    ) -> Result<(), StorageError>
    where
        I: Iterator<Item = Result<(Box<[u8]>, Box<[u8]>), rocksdb::Error>>,
    {
        if limit == Some(0) {
            return Ok(());
        }

        while let Some(Ok((k, v))) = iter.next() {
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            if !k.starts_with(prefix) {
                break;
            }
            let v = if self.password.is_some() {
                self.decrypt_data(v.to_vec())?
            } else {
                v.to_vec()
            };
            let v = String::from_utf8(v).map_err(|_| StorageError::ConversionError)?;
            result.push((k, v));
            if let Some(l) = limit {
                if result.len() >= l {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn has_key(&self, key: &str, transaction_id: Option<Uuid>) -> Result<bool, StorageError> {
        let result = match self.effective_transaction_id(transaction_id) {
            Some(tx_id) => {
                let map = self.transactions.borrow();
                let tx = map
                    .get(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                tx.get(key.as_bytes())
                    .map_err(|_| StorageError::ReadError)?
            }
            None => self
                .db
                .get(key.as_bytes())
                .map_err(|_| StorageError::ReadError)?,
        };

        Ok(result.is_some())
    }

    /// # Safety
    /// This method uses `std::mem::transmute` to extend the transaction's lifetime to `'static`,
    /// which is safe in this context because all transactions are stored in a `RefCell` within the `Storage` struct,
    /// and are only accessed from the same thread.
    /// Ensure that all transactions are properly committed or rolled back to avoid resource leaks.
    pub fn begin_transaction(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.create_transaction(id);
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

    pub fn begin_global_transaction(&self) -> Result<(), StorageError> {
        if !self.global_transaction_is_active() {
            self.create_transaction(GLOBAL_TRANSACTION_ID);
            Ok(())
        } else {
            Err(StorageError::GlobalTransactionAlreadyActiveError)
        }
    }

    pub fn commit_global_transaction(&self) -> Result<(), StorageError> {
        self.commit_transaction(GLOBAL_TRANSACTION_ID)
    }

    pub fn rollback_global_transaction(&self) -> Result<(), StorageError> {
        self.rollback_transaction(GLOBAL_TRANSACTION_ID)
    }

    fn create_transaction(&self, transaction_id: Uuid) {
        let mut write_options = WriteOptions::default();
        write_options.set_sync(true);

        let transaction = self.db.transaction_opt(&write_options, &Default::default());
        let mut map = self.transactions.borrow_mut();
        map.insert(
            transaction_id,
            Box::new(unsafe {
                std::mem::transmute::<
                    rocksdb::Transaction<'_, TransactionDB>,
                    rocksdb::Transaction<'static, TransactionDB>,
                >(transaction)
            }),
        );
    }

    fn global_transaction_is_active(&self) -> bool {
        self.transactions
            .borrow()
            .contains_key(&GLOBAL_TRANSACTION_ID)
    }

    fn effective_transaction_id(&self, transaction_id: Option<Uuid>) -> Option<Uuid> {
        transaction_id.or_else(|| {
            self.global_transaction_is_active()
                .then_some(GLOBAL_TRANSACTION_ID)
        })
    }

    fn encrypt_data(&self, data: Vec<u8>) -> Result<Vec<u8>, StorageError> {
        let key: &[u8; 32] = self
            .password
            .as_ref()
            .unwrap()
            .as_slice()
            .try_into()
            .expect("DEK is always 32 bytes");
        let mut nonce_seed = [0u8; 32];
        SysRng.try_fill_bytes(&mut nonce_seed)?;
        MiniCocoon::from_key(key, &nonce_seed)
            .wrap(&data)
            .map_err(|error| StorageError::FailedToEncryptData { error })
    }

    fn decrypt_data(&self, data: Vec<u8>) -> Result<Vec<u8>, StorageError> {
        let key: &[u8; 32] = self
            .password
            .as_ref()
            .unwrap()
            .as_slice()
            .try_into()
            .expect("DEK is always 32 bytes");
        // Nonce seed is not used during unwrap — the nonce is read from the ciphertext.
        MiniCocoon::from_key(key, &[0u8; 32])
            .unwrap(&data)
            .map_err(|error| StorageError::FailedToDecryptData { error })
    }
}

impl KeyValueStore for Storage {
    fn get<K, V>(&self, key: K, transaction_id: Option<Uuid>) -> Result<Option<V>, StorageError>
    where
        K: AsRef<str>,
        V: DeserializeOwned,
    {
        let key = key.as_ref();
        let value = self.read(key, self.effective_transaction_id(transaction_id))?;

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

        self.write(key, &value, self.effective_transaction_id(transaction_id))
    }

    fn remove<K>(&self, key: K, transaction_id: Option<Uuid>) -> Result<(), StorageError>
    where
        K: AsRef<str>,
    {
        let key = key.as_ref();

        self.delete(key, self.effective_transaction_id(transaction_id))
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
        let value: Option<V> = self.get(id, transaction_id)?;

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
    rocksdb::Options::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_config::PasswordPolicyConfig;
    use rand::{rng, Rng};
    use redact::Secret;
    use std::env;

    // Helper: create a unique temporary path for a RocksDB storage directory.
    fn temp_storage() -> PathBuf {
        let dir = env::temp_dir();
        let mut rang = rng();
        let index = rang.next_u32();
        dir.join(format!("storage_{}.db", index))
    }

    // Helper: create unique temporary paths for a backup file and its dek file.
    fn temp_backup() -> (PathBuf, PathBuf) {
        let dir = env::temp_dir();
        let mut rng = rng();
        let index = rng.next_u32();
        (
            dir.join(format!("backup_{}", index)),
            dir.join(format!("dek_{}", index)),
        )
    }

    // Helper: create a StorageConfig and Storage instance. If is_encrypted is true,
    // set a password (so the DB will be encrypted); otherwise open without password.
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
            password: password.map(|p| Secret::from(p)),
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

    // Test that a newly created storage starts empty.
    #[test]
    fn test_new_storage_starts_empty() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        assert!(store.is_empty(None)?);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test writing a plain key/value and reading it back.
    #[test]
    fn test_add_value_to_storage() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value", None)?;
        assert_eq!(
            store.read("test", None).unwrap(),
            Some("test_value".to_string())
        );
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Another read test that uses read() returned Result.
    #[test]
    fn test_read_a_value() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value", None)?;
        assert_eq!(store.read("test", None)?, Some("test_value".to_string()));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test deleting a key removes it from the DB.
    #[test]
    fn test_delete_value() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test", "test_value", None)?;
        assert_eq!(store.read("test", None)?, Some("test_value".to_string()));
        store.delete("test", None)?;
        assert_eq!(store.read("test", None)?, None);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test partial prefix search returns matching entries in order until the prefix no longer matches.
    #[test]
    fn test_find_multiple_answers() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        store.write("test2", "test_value2", None)?;
        store.write("test3", "test_value3", None)?;
        store.write("tes4", "test_value4", None)?;

        let result = store.partial_compare("test", None)?;
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

    // Test partial prefix search on an encrypted store does not decrypt entries outside the prefix.
    #[test]
    fn test_partial_compare_encrypted_ignores_out_of_range_entries() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(true)?;
        store.write("AAA", "test_value", None)?;

        let result = store.partial_compare("AAA", None)?;
        assert_eq!(result, vec![("AAA".to_string(), "test_value".to_string())]);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test partial_get deserializes each matching value into the requested type.
    #[test]
    fn test_partial_get_deserializes_values() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.set("item1", "value1".to_string(), None)?;
        store.set("item2", "value2".to_string(), None)?;
        store.set("other", "value3".to_string(), None)?; // outside the prefix

        let mut values: Vec<String> = store.partial_get("item", None)?;
        values.sort();
        assert_eq!(values, vec!["value1".to_string(), "value2".to_string()]);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test partial_compare_limited returns at most `limit` entries, smallest keys first.
    #[test]
    fn test_partial_compare_limited_stops_at_the_limit() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test2", "test_value2", None)?;
        store.write("test1", "test_value1", None)?;
        store.write("test3", "test_value3", None)?;

        assert_eq!(
            store.partial_compare_limited("test", Some(1), None)?,
            vec![("test1".to_string(), "test_value1".to_string())]
        );
        assert_eq!(
            store.partial_compare_limited("test", Some(2), None)?,
            vec![
                ("test1".to_string(), "test_value1".to_string()),
                ("test2".to_string(), "test_value2".to_string())
            ]
        );
        // A limit larger than the matching range returns the whole range.
        assert_eq!(
            store.partial_compare_limited("test", Some(10), None)?.len(),
            3
        );
        assert!(store
            .partial_compare_limited("test", Some(0), None)?
            .is_empty());
        assert!(store
            .partial_compare_limited("missing", Some(1), None)?
            .is_empty());

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test partial_compare_keys_limited returns at most `limit` keys, smallest first.
    #[test]
    fn test_partial_compare_keys_limited_stops_at_the_limit() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test2", "test_value2", None)?;
        store.write("test1", "test_value1", None)?;
        store.write("tes4", "test_value4", None)?; // outside the prefix

        assert_eq!(
            store.partial_compare_keys_limited("test", Some(1), None)?,
            vec!["test1".to_string()]
        );
        assert_eq!(
            store.partial_compare_keys_limited("test", None, None)?,
            vec!["test1".to_string(), "test2".to_string()]
        );
        assert!(store
            .partial_compare_keys_limited("test", Some(0), None)?
            .is_empty());
        assert!(store
            .partial_compare_keys_limited("missing", Some(1), None)?
            .is_empty());

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test partial_get_limited deserializes at most `limit` values.
    #[test]
    fn test_partial_get_limited_deserializes_values() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.set("item2", "value2".to_string(), None)?;
        store.set("item1", "value1".to_string(), None)?;
        store.set("other", "value3".to_string(), None)?; // outside the prefix

        let values: Vec<String> = store.partial_get_limited("item", Some(1), None)?;
        assert_eq!(values, vec!["value1".to_string()]);

        let values: Vec<String> = store.partial_get_limited("item", None, None)?;
        assert_eq!(values, vec!["value1".to_string(), "value2".to_string()]);

        let values: Vec<String> = store.partial_get_limited("missing", Some(1), None)?;
        assert!(values.is_empty());

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test the limited scans read through the global transaction when given no transaction id.
    #[test]
    fn test_partial_compare_limited_uses_global_transaction() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.begin_global_transaction()?;
        // set routes to the global transaction when given no transaction id.
        store.set("test1", "test_value1".to_string(), None)?;

        // Staged in the global transaction, so only a read that goes through it can see this.
        let values: Vec<String> = store.partial_get_limited("test", Some(1), None)?;
        assert_eq!(values, vec!["test_value1".to_string()]);
        assert_eq!(
            store.partial_compare_keys_limited("test", Some(1), None)?,
            vec!["test1".to_string()]
        );

        store.rollback_global_transaction()?;
        assert!(store
            .partial_compare_limited("test", Some(1), None)?
            .is_empty());
        assert!(store
            .partial_compare_keys_limited("test", Some(1), None)?
            .is_empty());

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test has_key returns true for existing keys and false otherwise.
    #[test]
    fn test_has_key() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        assert!(store.has_key("test1", None)?);
        assert!(!store.has_key("test2", None)?);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test that closing (drop) and reopening the DB works and data persists.
    #[test]
    fn test_open_storage() -> Result<(), StorageError> {
        let (_, config, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        drop(store);

        let open_store = Storage::open(&config);
        assert!(open_store.is_ok());
        assert_eq!(
            open_store.as_ref().unwrap().read("test1", None)?,
            Some("test_value1".to_string())
        );

        Storage::delete_db_files(open_store.unwrap())?;
        Ok(())
    }

    // Opening when DB doesn't exist should fail (since open expects existing DB).
    #[test]
    fn test_open_inexistent_storage() -> Result<(), StorageError> {
        let path = &temp_storage();

        let config = StorageConfig {
            path: path.to_string_lossy().to_string(),
            password: Some(Secret::from("password")),
        };
        let open_store = Storage::open(&config);
        assert!(open_store.is_err());
        Ok(())
    }

    // Test keys() returns all keys in the DB.
    #[test]
    fn test_keys() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        store.write("test2", "test_value2", None)?;
        store.write("test3", "test_value3", None)?;
        store.write("tes4", "test_value4", None)?;

        let keys = store.keys(None)?;
        assert_eq!(keys.len(), 4);
        assert!(keys.contains(&"test1".to_string()));
        assert!(keys.contains(&"test2".to_string()));
        assert!(keys.contains(&"test3".to_string()));
        assert!(keys.contains(&"tes4".to_string()));

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test starting a transaction, transactional writes, and committing makes them visible.
    #[test]
    fn test_transaction_commit() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store.write("test1", "test_value1", Some(transaction_id))?;
        store.write("test2", "test_value2", Some(transaction_id))?;
        // Not yet committed: reads must return None
        assert_eq!(store.read("test1", None)?, None);
        assert_eq!(store.read("test2", None)?, None);
        store.commit_transaction(transaction_id)?;

        assert_eq!(store.read("test1", None)?, Some("test_value1".to_string()));
        assert_eq!(store.read("test2", None)?, Some("test_value2".to_string()));

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test that rolling back a transaction discards transactional writes.
    #[test]
    fn test_transaction_rollback() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store.write("test1", "test_value1", Some(transaction_id))?;
        store.write("test2", "test_value2", Some(transaction_id))?;
        assert_eq!(store.read("test1", None)?, None);
        assert_eq!(store.read("test2", None)?, None);
        store.rollback_transaction(transaction_id)?;

        assert_eq!(store.read("test1", None)?, None);
        assert_eq!(store.read("test2", None)?, None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test transactional delete: deletion inside a transaction does not apply until commit.
    #[test]
    fn test_transactional_delete() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        let transaction_id = store.begin_transaction();
        store.delete("test1", Some(transaction_id))?;
        // Still visible before commit
        assert_eq!(
            store.read("test1", None).unwrap(),
            Some("test_value1".to_string())
        );
        store.commit_transaction(transaction_id).unwrap();

        // Gone after commit
        assert_eq!(store.read("test1", None).unwrap(), None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Ensure that uncommitted transactions in a second transaction don't affect visibility.
    #[test]
    fn test_non_commited_transactions_should_not_appear() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store
            .write("test1", "test_value1", Some(transaction_id))
            .unwrap();
        store
            .write("test2", "test_value2", Some(transaction_id))
            .unwrap();
        store.commit_transaction(transaction_id).unwrap();

        let second_transaction_id = store.begin_transaction();
        store
            .write("test3", "test_value3", Some(second_transaction_id))
            .unwrap();

        // test3 is not visible because the second transaction is not committed
        assert_eq!(
            store.read("test1", None).unwrap(),
            Some("test_value1".to_string())
        );
        assert_eq!(
            store.read("test2", None).unwrap(),
            Some("test_value2".to_string())
        );
        assert_eq!(store.read("test3", None).unwrap(), None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test encryption: create encrypted storage and verify set/get serializing via KeyValueStore.
    #[test]
    fn test_encrypt_and_decrypt() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(true)?;
        store.set("test1", "test_value1", None)?;
        let data = store.get::<String, String>("test1".to_string(), None)?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value1");

        store.set("test1", "test_value2", None)?;
        let data = store.get::<String, String>("test1".to_string(), None)?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value2");

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test backup writes a backup file and a dek file to disk.
    #[test]
    fn test_backup() -> Result<(), StorageError> {
        let (backup_path, dek_path) = temp_backup();
        let password = Secret::from("password");
        let (_, _, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        store.write("test2", "test_value2", None)?;
        store.backup(&backup_path, &dek_path, password)?;
        assert!(backup_path.exists());
        assert!(dek_path.exists());

        Ok(())
    }

    // Test restore_backup imports data from a backup into a fresh DB.
    #[test]
    fn test_restore_backup() -> Result<(), StorageError> {
        let (backup_path, dek_path) = temp_backup();
        let password = Secret::from("password");
        let (_, config, store) = create_path_and_storage(false)?;
        store.write("test1", "test_value1", None)?;
        store.write("test2", "test_value2", None)?;
        store.backup(&backup_path, &dek_path, password.clone())?;

        // Remove original DB and create a new empty one, then restore
        Storage::delete_db_files(store)?;
        let store = Storage::new(&config)?;
        store.restore_backup(&backup_path, &dek_path, password)?;

        assert_eq!(store.read("test1", None)?, Some("test_value1".to_string()));
        assert_eq!(store.read("test2", None)?, Some("test_value2".to_string()));

        Storage::delete_db_files(store)?;
        fs::remove_file(backup_path)?;
        fs::remove_file(dek_path)?;
        Ok(())
    }

    // Test backing up more than 1000 entries to ensure the batching logic works.
    #[test]
    fn test_more_than_1000_values_to_backup() -> Result<(), StorageError> {
        let quantity = 1500;
        let (backup_path, dek_path) = temp_backup();
        let password = Secret::from("password");
        let (_, config, store) = create_path_and_storage(false)?;
        for i in 0..quantity {
            store.write(&format!("test{}", i), &format!("test_value{}", i), None)?;
        }
        store.backup(&backup_path, &dek_path, password.clone())?;
        assert!(backup_path.exists());

        Storage::delete_db_files(store)?;

        let store = Storage::new(&config)?;
        store.restore_backup(&backup_path, &dek_path, password)?;

        for i in 0..quantity {
            assert_eq!(
                store.read(&format!("test{}", i), None)?,
                Some(format!("test_value{}", i).to_string())
            );
        }

        Storage::delete_db_files(store)?;
        fs::remove_file(backup_path)?;
        fs::remove_file(dek_path)?;
        Ok(())
    }

    // Test changing the DB password re-encrypts the DEK and the new password can open the DB.
    #[test]
    fn test_change_password() -> Result<(), StorageError> {
        let (path, _, store) = create_path_and_storage(true)?;
        store.set("test1", "test_value1", None)?;

        store.change_password(Secret::from("password"), Secret::from("new_password"))?;

        drop(store);

        // Open with new password and verify data is accessible
        let store = Storage::new_with_policy(
            &StorageConfig {
                path: path.to_string_lossy().to_string(),
                password: Some(Secret::from("new_password")),
            },
            Some(PasswordPolicyConfig {
                min_length: 1,
                min_number_of_special_chars: 0,
                min_number_of_uppercase: 0,
                min_number_of_digits: 0,
            }),
        )?;

        assert_eq!(
            store.get::<String, String>("test1".to_string(), None)?,
            Some("test_value1".to_string())
        );
        Storage::delete_db_files(store)?;

        Ok(())
    }

    // Test KeyValueStore remove() saves deletion (non-transactional).
    #[test]
    fn test_remove_value() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.set("test", "test_value", None)?;
        assert_eq!(store.get("test", None)?, Some("test_value".to_string()));
        store.remove("test", None)?;
        assert_eq!(store.get::<&str, String>("test", None)?, None);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test global (single shared) transaction commit: using None transaction_id respects global tx state.
    #[test]
    fn test_global_transaction_commit() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        // Using set with trasanction_id = None should apply to the global transaction
        store.set("test1", "test_value1", None)?;
        store.set("test2", "test_value2", None)?;

        // Visible before commit
        assert_eq!(store.get("test1", None)?, Some("test_value1".to_string()));
        assert_eq!(store.get("test2", None)?, Some("test_value2".to_string()));
        store.commit_global_transaction()?;

        assert_eq!(store.get("test1", None)?, Some("test_value1".to_string()));
        assert_eq!(store.get("test2", None)?, Some("test_value2".to_string()));
        assert_eq!(store.get::<&str, String>("test3", None)?, None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test global transaction rollback discards changes made while active.
    #[test]
    fn test_global_transaction_rollback() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        store.set("test1", "test_value1", None)?;
        store.set("test2", "test_value2", None)?;

        // Changes should be visible before commit
        assert_eq!(store.get("test1", None)?, Some("test_value1".to_string()));
        assert_eq!(store.get("test2", None)?, Some("test_value2".to_string()));
        store.rollback_global_transaction()?;

        assert_eq!(store.get::<&str, String>("test1", None)?, None);
        assert_eq!(store.get::<&str, String>("test2", None)?, None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test deleting inside a global transaction behaves like transactional delete until commit.
    #[test]
    fn test_global_transactional_delete() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.set("test1", "test_value1", None)?;
        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        store.remove("test1", None)?;
        // Not visible before commit because it's part of a global transaction
        assert_eq!(store.get::<&str, String>("test1", None)?, None);
        store.rollback_global_transaction()?;

        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        store.remove("test1", None)?;

        store.commit_global_transaction()?;
        // Gone after commit
        assert_eq!(store.get::<&str, String>("test1", None)?, None);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Starting a second global transaction should return an error.
    #[test]
    fn test_global_transaction_already_active() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());

        let result = store.begin_global_transaction();
        assert!(result.is_err());
        match result {
            Err(StorageError::GlobalTransactionAlreadyActiveError) => {}
            _ => panic!("Expected GlobalTransactionAlreadyActiveError"),
        }

        store.commit_global_transaction()?;
        Ok(())
    }

    // Committing or rollbacking when no global transaction is active should return a NotFound(Transaction) error.
    #[test]
    fn test_cant_commit_or_rollback_global_transaction_if_not_active() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        assert!(!store.global_transaction_is_active());

        let result = store.commit_global_transaction();
        assert!(result.is_err());
        match result {
            Err(StorageError::NotFound(value)) => {
                assert_eq!(value, "Transaction");
            }
            _ => panic!("Expected NotFound(Transaction) error"),
        }

        let result = store.rollback_global_transaction();
        assert!(result.is_err());
        match result {
            Err(StorageError::NotFound(value)) => {
                assert_eq!(value, "Transaction");
            }
            _ => panic!("Expected NotFound(Transaction) error"),
        }

        Ok(())
    }

    // Test dropping the store rollback all transactions.
    #[test]
    fn test_drop_clears_transactions() -> Result<(), StorageError> {
        let (_, config, store) = create_path_and_storage(false)?;
        let transaction_id = store.begin_transaction();
        store
            .write("test1", "test_value1", Some(transaction_id))
            .unwrap();
        assert!(store.transactions.borrow().contains_key(&transaction_id));
        drop(store);
        // After drop, the transactions map should be cleared.

        let store = Storage::new(&config)?;
        assert_eq!(store.read("test1", None)?.is_some(), false);
        Ok(())
    }
}
