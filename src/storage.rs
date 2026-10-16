use crate::{
    backup_io::{BackupFileReader, BackupFileWriter},
    dek::Dek,
    error::StorageError,
    key::StorageKey,
    password_policy::PasswordPolicy,
    storage_config::{PasswordPolicyConfig, StorageConfig},
};
use cocoon::Cocoon;
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
    dek: Option<Dek>,
    password_policy: PasswordPolicy,
    // Used by writes made outside a transaction.
    write_options: WriteOptions,
}

pub trait KeyValueStore {
    fn get<V>(
        &self,
        key: impl AsRef<StorageKey>,
        transaction_id: Option<Uuid>,
    ) -> Result<Option<V>, StorageError>
    where
        V: DeserializeOwned;

    fn set<V>(
        &self,
        key: impl AsRef<StorageKey>,
        value: V,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError>
    where
        V: Serialize;

    fn remove(
        &self,
        key: impl AsRef<StorageKey>,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError>;

    fn update<V>(
        &self,
        key: impl AsRef<StorageKey>,
        updates: &HashMap<&str, Value>,
        transaction_id: Option<Uuid>,
    ) -> Result<V, StorageError>
    where
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

    /// Repairs a RocksDB database at `path`.
    ///
    /// The database must not be open while it is being repaired, and `path` must exist.
    /// Repair succeeds even when it has to drop data, so check the report for files it could
    /// not recover.
    pub fn repair<P: AsRef<Path>>(path: P) -> Result<RepairReport, StorageError> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(StorageError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not a database directory", path.display()),
            )));
        }
        let lost_before = lost_files(path);
        rocksdb::TransactionDB::<rocksdb::SingleThreaded>::repair(&create_options(), path)?;
        let lost_files = lost_files(path)
            .into_iter()
            .filter(|file| !lost_before.contains(file))
            .collect();
        Ok(RepairReport { lost_files })
    }

    fn open_db(
        config: &StorageConfig,
        password_policy_config: Option<PasswordPolicyConfig>,
        options: &rocksdb::Options,
    ) -> Result<Storage, StorageError> {
        check_current_file(Path::new(&config.path))?;

        let db = rocksdb::TransactionDB::open(
            options,
            &rocksdb::TransactionDBOptions::default(),
            config.path.as_str(),
        )?;

        let mut write_options = WriteOptions::default();
        write_options.set_sync(config.sync_writes);

        let password_policy = if let Some(ref policy) = password_policy_config {
            PasswordPolicy::new(policy.clone())
        } else {
            PasswordPolicy::default()
        };

        let dek = if let Some(ref password) = config.password {
            if !password_policy.is_valid(password.expose_secret()) {
                return Err(StorageError::WeakPassword(password_policy));
            }
            let dek = match db
                .get(DEK_KEY)
                .map_err(|e| StorageError::ReadError(e.to_string()))?
            {
                Some(encrypted_dek) => {
                    let mut entry_cursor = Cursor::new(encrypted_dek);

                    let cocoon = Cocoon::new(password.expose_secret().as_bytes());

                    let dek = cocoon
                        .parse(&mut entry_cursor)
                        .map_err(|_| StorageError::WrongPassword)?;
                    Dek::try_from(dek)?
                }
                None => {
                    let dek = Dek::generate()?;

                    let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
                    let mut cocoon = Cocoon::new(password.expose_secret().as_bytes());
                    cocoon
                        .dump(dek.as_bytes().to_vec(), &mut entry_cursor)
                        .map_err(|error| StorageError::FailedToEncryptData { error })?;
                    let encrypted_dek = entry_cursor.into_inner();
                    db.put_opt(DEK_KEY.as_bytes(), encrypted_dek, &write_options)
                        .map_err(|_| StorageError::WriteError)?;
                    dek
                }
            };

            Some(dek)
        } else {
            None
        };

        Ok(Storage {
            db,
            transactions: RefCell::new(HashMap::new()),
            dek,
            password_policy,
            write_options,
        })
    }

    pub fn change_password(
        &self,
        old_password: Secret<String>,
        new_password: Secret<String>,
    ) -> Result<(), StorageError> {
        match &self.dek {
            Some(_) => {
                if !self.password_policy.is_valid(new_password.expose_secret()) {
                    return Err(StorageError::WeakPassword(self.password_policy.clone()));
                }
            }
            None => return Err(StorageError::NoPasswordSet),
        }

        let dek = match self
            .db
            .get(DEK_KEY)
            .map_err(|e| StorageError::ReadError(e.to_string()))?
        {
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
            .put_opt(DEK_KEY.as_bytes(), encrypted_dek, &self.write_options)
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
            let dek = Dek::try_from(dek)?;

            let mut backup_reader = BackupFileReader::new(backup_file, &dek)?;

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
        let iter = snapshot.iterator(rocksdb::IteratorMode::Start);
        let backup_file = File::create(backup_path)?;
        let mut dek_file = File::create(dek_path)?;
        let mut data_vec = Vec::new();
        let mut item_counter = 0;

        let dek = Dek::generate()?;

        let mut entry_cursor: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        let mut cocoon = Cocoon::new(password.expose_secret().as_bytes());
        cocoon
            .dump(dek.as_bytes().to_vec(), &mut entry_cursor)
            .map_err(|error| StorageError::FailedToEncryptData { error })?;
        let encrypted_dek = entry_cursor.into_inner();
        dek_file.write_all(&encrypted_dek)?;

        let mut backup_writer = BackupFileWriter::new(backup_file, &dek)?;

        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::ReadError(e.to_string()))?;
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
                    .delete_opt(key.as_bytes(), &self.write_options)
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

        if let Some(dek) = &self.dek {
            data = dek.encrypt(&data)?
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
                    .put_opt(key.as_bytes(), data, &self.write_options)
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
                    .map_err(|e| StorageError::ReadError(e.to_string()))?
            }
            None => self
                .db
                .get(key.as_bytes())
                .map_err(|e| StorageError::ReadError(e.to_string()))?,
        };

        match data {
            Some(mut data) => {
                if let Some(dek) = &self.dek {
                    data = dek.decrypt(&data)?;
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
                let first = tx.iterator(rocksdb::IteratorMode::Start).next();
                Ok(first
                    .transpose()
                    .map_err(|e| StorageError::ReadError(e.to_string()))?
                    .is_none())
            }
            None => {
                let first = self.db.iterator(rocksdb::IteratorMode::Start).next();
                Ok(first
                    .transpose()
                    .map_err(|e| StorageError::ReadError(e.to_string()))?
                    .is_none())
            }
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
        iter: I,
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

        for item in iter {
            let (k, _) = item.map_err(|e| StorageError::ReadError(e.to_string()))?;
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
        iter: I,
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

        for item in iter {
            let (k, v) = item.map_err(|e| StorageError::ReadError(e.to_string()))?;
            let k = String::from_utf8(k.to_vec()).map_err(|_| StorageError::ConversionError)?;
            if !k.starts_with(prefix) {
                break;
            }
            let v = if let Some(dek) = &self.dek {
                dek.decrypt(&v)?
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

    pub fn has_key(
        &self,
        key: impl AsRef<StorageKey>,
        transaction_id: Option<Uuid>,
    ) -> Result<bool, StorageError> {
        let key = key.as_ref().joined();
        let result = match self.effective_transaction_id(transaction_id) {
            Some(tx_id) => {
                let map = self.transactions.borrow();
                let tx = map
                    .get(&tx_id)
                    .ok_or(StorageError::NotFound("Transaction".to_string()))?;
                tx.get(key.as_bytes())
                    .map_err(|e| StorageError::ReadError(e.to_string()))?
            }
            None => self
                .db
                .get(key.as_bytes())
                .map_err(|e| StorageError::ReadError(e.to_string()))?,
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
}

impl KeyValueStore for Storage {
    fn get<V>(
        &self,
        key: impl AsRef<StorageKey>,
        transaction_id: Option<Uuid>,
    ) -> Result<Option<V>, StorageError>
    where
        V: DeserializeOwned,
    {
        let key = key.as_ref().joined();
        let value = self.read(&key, self.effective_transaction_id(transaction_id))?;

        match value {
            Some(value) => {
                let value =
                    serde_json::from_str(&value).map_err(|_| StorageError::ConversionError)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    fn set<V>(
        &self,
        key: impl AsRef<StorageKey>,
        value: V,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError>
    where
        V: Serialize,
    {
        let key = key.as_ref().joined();
        let value = serde_json::to_string(&value).map_err(|_| StorageError::ConversionError)?;

        self.write(&key, &value, self.effective_transaction_id(transaction_id))
    }

    fn remove(
        &self,
        key: impl AsRef<StorageKey>,
        transaction_id: Option<Uuid>,
    ) -> Result<(), StorageError> {
        let key = key.as_ref().joined();

        self.delete(&key, self.effective_transaction_id(transaction_id))
    }

    fn update<V>(
        &self,
        key: impl AsRef<StorageKey>,
        updates: &HashMap<&str, Value>,
        transaction_id: Option<Uuid>,
    ) -> Result<V, StorageError>
    where
        V: Serialize + DeserializeOwned + Clone,
    {
        // 1. Fetch the existing value from the database
        let value: Option<V> = self.get(key.as_ref(), transaction_id)?;

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
            self.set(key, updated_value.clone(), transaction_id)?;

            Ok(updated_value)
        } else {
            Err(StorageError::NotFound("Value".to_string()))
        }
    }
}

/// What a repair could not recover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReport {
    /// SST files that repair could not read and moved to the `lost` directory during this run.
    pub lost_files: Vec<PathBuf>,
}

// Repair moves the SST files it cannot read to `lost`. Other files there are not losses: a
// MANIFEST is replaced by a rebuilt one and a WAL is archived once converted to an SST, so
// damage inside a WAL does not show up here.
fn lost_files(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path.join("lost")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|file| file.extension().is_some_and(|ext| ext == "sst"))
        .collect()
}

fn create_options() -> rocksdb::Options {
    rocksdb::Options::default()
}

// A directory with data files but no CURRENT is a damaged database. Opening it with
// create_if_missing would either fail with an unrelated message or start a new empty database.
fn check_current_file(path: &Path) -> Result<(), StorageError> {
    if path.join("CURRENT").exists() {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Ok(());
    };
    let has_data_files = entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("MANIFEST-") || name.ends_with(".sst") || name.ends_with(".log")
    });
    if has_data_files {
        return Err(StorageError::MissingCurrent(path.display().to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::StorageKey;
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

        let config = StorageConfig::new(
            path.to_string_lossy().to_string(),
            password.map(|p| Secret::from(p)),
        );

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

    #[test]
    fn test_repair_storage() -> Result<(), StorageError> {
        let (path, config, store) = create_path_and_storage(false)?;
        store.write("test", "test_value", None)?;
        drop(store);

        Storage::repair(&path)?;

        let store = Storage::open(&config)?;
        assert_eq!(store.read("test", None)?, Some("test_value".to_string()));
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
        store.set(StorageKey::try_from("item1")?, "value1".to_string(), None)?;
        store.set(StorageKey::try_from("item2")?, "value2".to_string(), None)?;
        store.set(StorageKey::try_from("other")?, "value3".to_string(), None)?; // outside the prefix

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
        store.set(StorageKey::try_from("item2")?, "value2".to_string(), None)?;
        store.set(StorageKey::try_from("item1")?, "value1".to_string(), None)?;
        store.set(StorageKey::try_from("other")?, "value3".to_string(), None)?; // outside the prefix

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
        store.set(
            StorageKey::try_from("test1")?,
            "test_value1".to_string(),
            None,
        )?;

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
        assert!(store.has_key(StorageKey::try_from("test1")?, None)?);
        assert!(!store.has_key(StorageKey::try_from("test2")?, None)?);
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

        let config = StorageConfig::new(
            path.to_string_lossy().to_string(),
            Some(Secret::from("password")),
        );
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
        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;
        let data = store.get::<String>(StorageKey::try_from("test1")?, None)?;
        assert!(data.is_some());
        assert_eq!(data.unwrap(), "test_value1");

        store.set(StorageKey::try_from("test1")?, "test_value2", None)?;
        let data = store.get::<String>(StorageKey::try_from("test1")?, None)?;
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
        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;

        store.change_password(Secret::from("password"), Secret::from("new_password"))?;

        drop(store);

        // Open with new password and verify data is accessible
        let store = Storage::new_with_policy(
            &StorageConfig::new(
                path.to_string_lossy().to_string(),
                Some(Secret::from("new_password")),
            ),
            Some(PasswordPolicyConfig {
                min_length: 1,
                min_number_of_special_chars: 0,
                min_number_of_uppercase: 0,
                min_number_of_digits: 0,
            }),
        )?;

        assert_eq!(
            store.get::<String>(StorageKey::try_from("test1")?, None)?,
            Some("test_value1".to_string())
        );
        Storage::delete_db_files(store)?;

        Ok(())
    }

    // Test KeyValueStore remove() saves deletion (non-transactional).
    #[test]
    fn test_remove_value() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.set(StorageKey::try_from("test")?, "test_value", None)?;
        assert_eq!(
            store.get(StorageKey::try_from("test")?, None)?,
            Some("test_value".to_string())
        );
        store.remove(StorageKey::try_from("test")?, None)?;
        assert_eq!(
            store.get::<String>(StorageKey::try_from("test")?, None)?,
            None
        );
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
        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;
        store.set(StorageKey::try_from("test2")?, "test_value2", None)?;

        // Visible before commit
        assert_eq!(
            store.get(StorageKey::try_from("test1")?, None)?,
            Some("test_value1".to_string())
        );
        assert_eq!(
            store.get(StorageKey::try_from("test2")?, None)?,
            Some("test_value2".to_string())
        );
        store.commit_global_transaction()?;

        assert_eq!(
            store.get(StorageKey::try_from("test1")?, None)?,
            Some("test_value1".to_string())
        );
        assert_eq!(
            store.get(StorageKey::try_from("test2")?, None)?,
            Some("test_value2".to_string())
        );
        assert_eq!(
            store.get::<String>(StorageKey::try_from("test3")?, None)?,
            None
        );

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test global transaction rollback discards changes made while active.
    #[test]
    fn test_global_transaction_rollback() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;
        store.set(StorageKey::try_from("test2")?, "test_value2", None)?;

        // Changes should be visible before commit
        assert_eq!(
            store.get(StorageKey::try_from("test1")?, None)?,
            Some("test_value1".to_string())
        );
        assert_eq!(
            store.get(StorageKey::try_from("test2")?, None)?,
            Some("test_value2".to_string())
        );
        store.rollback_global_transaction()?;

        assert_eq!(
            store.get::<String>(StorageKey::try_from("test1")?, None)?,
            None
        );
        assert_eq!(
            store.get::<String>(StorageKey::try_from("test2")?, None)?,
            None
        );

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Test deleting inside a global transaction behaves like transactional delete until commit.
    #[test]
    fn test_global_transactional_delete() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;
        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;
        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        store.remove(StorageKey::try_from("test1")?, None)?;
        // Not visible before commit because it's part of a global transaction
        assert_eq!(
            store.get::<String>(StorageKey::try_from("test1")?, None)?,
            None
        );
        store.rollback_global_transaction()?;

        store.begin_global_transaction()?;
        assert!(store.global_transaction_is_active());
        store.remove(StorageKey::try_from("test1")?, None)?;

        store.commit_global_transaction()?;
        // Gone after commit
        assert_eq!(
            store.get::<String>(StorageKey::try_from("test1")?, None)?,
            None
        );

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

    // Test that StorageKey and &StorageKey can be used with get/set/remove/has_key.
    #[test]
    fn test_storage_key_works_with_get_set_and_partial_compare() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;

        let key = StorageKey::new(["program", "123", "state"])?;
        store.set(&key, "running".to_string(), None)?;
        assert_eq!(
            store.get::<String>(&key, None)?,
            Some("running".to_string())
        );
        assert!(store.has_key(&key, None)?);

        let scan_key = StorageKey::new(["program", "123"])?;
        assert_eq!(
            store.partial_compare(&scan_key.to_scan_prefix(), None)?,
            vec![("program/123/state".to_string(), "\"running\"".to_string())]
        );

        store.remove(&key, None)?;
        assert!(!store.has_key(key, None)?);

        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Regression test for the prefix-scan ambiguity a raw starts_with-based
    // scan has: a scan for "program/123" would also match "program/1234/...".
    // StorageKey::to_scan_prefix appends a trailing separator to rule that out.
    #[test]
    fn test_storage_key_scan_prefix_does_not_match_sibling_ids() -> Result<(), StorageError> {
        let (_, _, store) = create_path_and_storage(false)?;

        store.set(
            StorageKey::new(["program", "123", "state"])?,
            "a".to_string(),
            None,
        )?;
        store.set(
            StorageKey::new(["program", "1234", "state"])?,
            "b".to_string(),
            None,
        )?;

        let scan_prefix = StorageKey::new(["program", "123"])?.to_scan_prefix();
        let keys = store.partial_compare_keys(&scan_prefix, None)?;
        assert_eq!(keys, vec!["program/123/state".to_string()]);

        Storage::delete_db_files(store)?;
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

    // Helper: create a storage with the given sync_writes setting, optionally encrypted.
    // Also returns the options it was opened with, which collect RocksDB statistics.
    fn create_storage_with_sync(
        sync_writes: bool,
        is_encrypted: bool,
    ) -> Result<(Storage, rocksdb::Options), StorageError> {
        let config = StorageConfig::new(
            temp_storage().to_string_lossy().to_string(),
            is_encrypted.then(|| Secret::from("password".to_string())),
        )
        .with_sync_writes(sync_writes);
        let mut options = create_options();
        options.create_if_missing(true);
        options.enable_statistics();
        let policy = PasswordPolicyConfig {
            min_length: 1,
            min_number_of_special_chars: 0,
            min_number_of_uppercase: 0,
            min_number_of_digits: 0,
        };
        let storage = Storage::open_db(&config, Some(policy), &options)?;
        Ok((storage, options))
    }

    // Helper: how many times RocksDB has synced the WAL to disk for the storage opened with options.
    fn wal_syncs(options: &rocksdb::Options) -> u64 {
        options.get_ticker_count(rocksdb::statistics::Ticker::WalFileSynced)
    }

    // sync_writes is off unless set, both from a file and from new().
    #[test]
    fn test_sync_writes_defaults_to_false() {
        let config: StorageConfig = serde_json::from_str(r#"{"path": "some/path"}"#).unwrap();
        assert!(!config.sync_writes);
        assert!(!StorageConfig::new("some/path".to_string(), None).sync_writes);
    }

    // Without sync_writes, writes outside a transaction do not wait for the disk.
    #[test]
    fn test_direct_writes_do_not_sync_by_default() -> Result<(), StorageError> {
        let (store, options) = create_storage_with_sync(false, false)?;
        let before = wal_syncs(&options);

        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;
        store.remove(StorageKey::try_from("test1")?, None)?;

        assert_eq!(wal_syncs(&options), before);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // With sync_writes, every write and delete outside a transaction syncs the WAL.
    #[test]
    fn test_sync_writes_syncs_each_direct_write() -> Result<(), StorageError> {
        let (store, options) = create_storage_with_sync(true, false)?;
        let before = wal_syncs(&options);

        store.set(StorageKey::try_from("test1")?, "test_value1", None)?;
        store.set(StorageKey::try_from("test2")?, "test_value2", None)?;
        store.remove(StorageKey::try_from("test1")?, None)?;

        assert_eq!(wal_syncs(&options), before + 3);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // With sync_writes, the DEK written when an encrypted storage is created is synced too.
    #[test]
    fn test_sync_writes_syncs_dek_on_creation() -> Result<(), StorageError> {
        let (unsynced, unsynced_options) = create_storage_with_sync(false, true)?;
        let (synced, synced_options) = create_storage_with_sync(true, true)?;

        assert_eq!(wal_syncs(&synced_options), wal_syncs(&unsynced_options) + 1);
        Storage::delete_db_files(unsynced)?;
        Storage::delete_db_files(synced)?;
        Ok(())
    }

    // With sync_writes, the DEK rewritten by change_password is synced.
    #[test]
    fn test_sync_writes_syncs_change_password() -> Result<(), StorageError> {
        let (store, options) = create_storage_with_sync(true, true)?;
        let before = wal_syncs(&options);

        store.change_password(Secret::from("password"), Secret::from("new_password"))?;

        assert_eq!(wal_syncs(&options), before + 1);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Transaction commits sync whether or not sync_writes is set.
    #[test]
    fn test_transaction_commit_syncs_without_sync_writes() -> Result<(), StorageError> {
        let (store, options) = create_storage_with_sync(false, false)?;
        let before = wal_syncs(&options);

        let transaction_id = store.begin_transaction();
        store.set(
            StorageKey::try_from("test1")?,
            "test_value1",
            Some(transaction_id),
        )?;
        store.commit_transaction(transaction_id)?;

        assert_eq!(wal_syncs(&options), before + 1);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Writes entries, flushes them to an SST file and damages that file's first data block.
    // Returns the config to reopen the storage.
    fn create_storage_with_corrupted_sst() -> Result<StorageConfig, StorageError> {
        let (path, config, store) = create_path_and_storage(false)?;
        for i in 0..3 {
            store.write(&format!("key{i}"), "test_value", None)?;
        }
        drop(store);
        // Reopening with a plain DB replays the WAL and flushes it to an SST file.
        let db = rocksdb::DB::open_default(&path)?;
        db.flush()?;
        drop(db);

        let sst = fs::read_dir(&path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|p| p.extension().is_some_and(|ext| ext == "sst"))
            .expect("flush should have created an SST file");
        let mut bytes = fs::read(&sst)?;
        for byte in &mut bytes[8..16] {
            *byte ^= 0xFF;
        }
        fs::write(&sst, bytes)?;

        Ok(config)
    }

    fn assert_read_error<T: std::fmt::Debug>(result: Result<T, StorageError>) {
        match result {
            Err(StorageError::ReadError(message)) => assert!(
                message.contains("Corruption"),
                "RocksDB message should be kept, got: {message}"
            ),
            other => panic!("expected ReadError, got {other:?}"),
        }
    }

    // A damaged block surfaces as a read error that keeps the RocksDB message.
    #[test]
    fn test_read_error_keeps_rocksdb_message() -> Result<(), StorageError> {
        let store = Storage::open(&create_storage_with_corrupted_sst()?)?;

        assert_read_error(store.read("key0", None));
        assert_read_error(store.has_key(StorageKey::try_from("key0")?, None));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Listings return the iterator error instead of stopping at the damaged block.
    #[test]
    fn test_listings_return_iterator_errors() -> Result<(), StorageError> {
        let store = Storage::open(&create_storage_with_corrupted_sst()?)?;

        assert_read_error(store.keys(None));
        assert_read_error(store.partial_compare_keys("key", None));
        assert_read_error(store.partial_compare("key", None));
        assert_read_error(store.is_empty(None));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // A backup of a damaged database fails instead of writing a truncated file.
    #[test]
    fn test_backup_fails_on_iterator_error() -> Result<(), StorageError> {
        let store = Storage::open(&create_storage_with_corrupted_sst()?)?;
        let (backup_path, dek_path) = temp_backup();

        assert_read_error(store.backup(&backup_path, &dek_path, Secret::from("ABC123!!!abcdef")));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Writes a key and flushes it to an SST file, so the data does not depend on the WAL.
    fn create_flushed_storage() -> Result<(PathBuf, StorageConfig), StorageError> {
        let (path, config, store) = create_path_and_storage(false)?;
        store.write("key0", "test_value", None)?;
        drop(store);
        let db = rocksdb::DB::open_default(&path)?;
        db.flush()?;
        drop(db);
        Ok((path, config))
    }

    fn assert_missing_current(result: Result<Storage, StorageError>) {
        match result.err() {
            Some(StorageError::MissingCurrent(_)) => {}
            other => panic!("expected MissingCurrent, got {other:?}"),
        }
    }

    // Both opening and creating report the missing CURRENT, with the WAL still around.
    #[test]
    fn test_missing_current_is_reported() -> Result<(), StorageError> {
        let (path, config) = create_flushed_storage()?;
        fs::remove_file(path.join("CURRENT"))?;

        assert_missing_current(Storage::open(&config));
        assert_missing_current(Storage::new(&config));
        fs::remove_dir_all(path)?;
        Ok(())
    }

    // With only SST files left, creating must not start a new empty database over them.
    #[test]
    fn test_missing_current_with_only_sst_files_is_reported() -> Result<(), StorageError> {
        let (path, config) = create_flushed_storage()?;
        for entry in fs::read_dir(&path)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "CURRENT" || name.starts_with("MANIFEST-") || name.ends_with(".log") {
                fs::remove_file(entry.path())?;
            }
        }

        assert_missing_current(Storage::new(&config));
        fs::remove_dir_all(path)?;
        Ok(())
    }

    // An existing directory without database files is not a damaged database.
    #[test]
    fn test_new_storage_in_empty_directory() -> Result<(), StorageError> {
        let path = temp_storage();
        fs::create_dir_all(&path)?;
        let config = StorageConfig::new(path.to_string_lossy().to_string(), None);

        let store = Storage::new(&config)?;
        assert!(store.is_empty(None)?);
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Repair rebuilds the missing CURRENT and keeps the data.
    #[test]
    fn test_repair_recovers_missing_current() -> Result<(), StorageError> {
        let (path, config) = create_flushed_storage()?;
        fs::remove_file(path.join("CURRENT"))?;

        Storage::repair(&path)?;

        let store = Storage::open(&config)?;
        assert_eq!(store.read("key0", None)?, Some("test_value".to_string()));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Repair of a healthy database keeps its data and reports nothing lost.
    #[test]
    fn test_repair_report_is_empty_for_healthy_database() -> Result<(), StorageError> {
        let (path, config) = create_flushed_storage()?;

        let report = Storage::repair(&path)?;

        assert!(report.lost_files.is_empty());
        let store = Storage::open(&config)?;
        assert_eq!(store.read("key0", None)?, Some("test_value".to_string()));
        Storage::delete_db_files(store)?;
        Ok(())
    }

    // Repair succeeds on a damaged SST but the report lists the file it had to drop.
    #[test]
    fn test_repair_report_lists_lost_sst() -> Result<(), StorageError> {
        let config = create_storage_with_corrupted_sst()?;
        let path = PathBuf::from(&config.path);

        let report = Storage::repair(&path)?;

        assert!(!report.lost_files.is_empty());
        assert!(report
            .lost_files
            .iter()
            .any(|file| file.extension().is_some_and(|ext| ext == "sst")));
        assert!(report
            .lost_files
            .iter()
            .all(|file| file.starts_with(path.join("lost"))));
        fs::remove_dir_all(path)?;
        Ok(())
    }

    // Files left in `lost` by an earlier repair are not reported again.
    #[test]
    fn test_repair_report_only_lists_files_lost_in_this_run() -> Result<(), StorageError> {
        let config = create_storage_with_corrupted_sst()?;
        let path = PathBuf::from(&config.path);
        assert!(!Storage::repair(&path)?.lost_files.is_empty());

        let report = Storage::repair(&path)?;

        assert!(report.lost_files.is_empty());
        fs::remove_dir_all(path)?;
        Ok(())
    }

    // Repair must not create a database at a path that does not exist.
    #[test]
    fn test_repair_fails_for_missing_path() {
        let path = temp_storage();

        assert!(matches!(
            Storage::repair(&path),
            Err(StorageError::IoError(_))
        ));
        assert!(!path.exists());
    }

    type Damage = fn(&Path);

    fn manifest_file(path: &Path) -> PathBuf {
        fs::read_dir(path)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|file| {
                file.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("MANIFEST-"))
            })
            .expect("database should have a MANIFEST")
    }

    // Damage to CURRENT or the MANIFEST is reported when opening, and repair rebuilds both
    // from the SST files without losing data.
    #[test]
    fn test_damaged_metadata_fails_to_open_and_repair_recovers() -> Result<(), StorageError> {
        let damages: Vec<(&str, Damage)> = vec![
            ("empty CURRENT", |p| {
                fs::write(p.join("CURRENT"), "").unwrap()
            }),
            ("garbage CURRENT", |p| {
                fs::write(p.join("CURRENT"), "garbage\n").unwrap()
            }),
            ("CURRENT without MANIFEST", |p| {
                fs::write(p.join("CURRENT"), "MANIFEST-999999\n").unwrap()
            }),
            ("deleted MANIFEST", |p| {
                fs::remove_file(manifest_file(p)).unwrap()
            }),
            ("corrupted MANIFEST", |p| {
                let manifest = manifest_file(p);
                let mut bytes = fs::read(&manifest).unwrap();
                for byte in &mut bytes[8..40] {
                    *byte ^= 0xFF;
                }
                fs::write(manifest, bytes).unwrap();
            }),
        ];

        for (name, damage) in damages {
            let (path, config) = create_flushed_storage()?;
            damage(&path);

            assert!(
                matches!(
                    Storage::open(&config).err(),
                    Some(StorageError::CreationError(_))
                ),
                "{name}: opening should fail"
            );
            assert!(
                Storage::repair(&path)?.lost_files.is_empty(),
                "{name}: nothing is lost"
            );
            let store = Storage::open(&config)?;
            assert_eq!(
                store.read("key0", None)?,
                Some("test_value".to_string()),
                "{name}: data should survive repair"
            );
            Storage::delete_db_files(store)?;
        }
        Ok(())
    }
}
