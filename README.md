# BitVMX Storage Backend
A Rust library for managing storage for BitVMX 

## ⚠️ Disclaimer

This library is currently under development and may not be fully stable.
It is not production-ready, has not been audited, and future updates may introduce breaking changes without preserving backward compatibility.

## ⚠️ Usage Warning
This library is not thread-safe and is limited to single threaded access due to the use of RefCell for transaction management.

## Backup Usage Disclaimer
The internal storage password and the backup password are two separate passwords, although they may be the same one. Hovewer, if you want to restore a backup you need to remember the password you used for this backup. Also, when you generate a backup, it creates two files, the dek file and the backup file. The dek file is where the random generated dek key to encrypt the bakcup file is stored, encrypted by the password you provided.

## Installation

For detailed installation instructions, environment setup, and troubleshooting, please see the [Installation Guide](INSTALLATION.md).

The installation guide covers:
- Installing RocksDB, Clang (LLVM), and compression libraries
- Setting up environment variables for macOS, Linux, and Windows
- VS Code integration for rust-analyzer
- Common troubleshooting issues and solutions


## Methods Overview
The `Storage` struct in `src/storage.rs` provides a comprehensive set of methods for managing a key-value store with optional encryption and transaction support. Below is a summary of the key methods available:

- **new**: Creates a new `Storage` instance with the specified configuration, initializing the database.

- **open**: Opens an existing `Storage` instance using the provided configuration.

- **write**: Writes a key-value pair to the database, with optional encryption.

- **read**: Reads a value associated with a key from the database, decrypting if necessary.

- **set**: Sets a key-value pair in the database, with optional transaction support.

- **get**: Retrieves a value associated with a key from the database, deserializing it into the specified type.

- **delete**: Deletes a key-value pair from the database.

- **is_empty**: Checks if the database is empty.

- **has_key**: Checks if a key exists in the database.

- **keys**: Retrieves all keys from the database.

- **partial_compare_keys**: Retrieves keys that start with the specified prefix.

- **partial_compare**: Retrieves key-value pairs where keys start with the specified prefix.

- **begin_transaction**: Begins a new transaction and returns its ID.

- **commit_transaction**: Commits the specified transaction.

- **rollback_transaction**: Rolls back the specified transaction.

- **transactional_write**: Writes a key-value pair within a transaction, with optional encryption.

- **transactional_delete**: Deletes a key-value pair within a transaction.

- **delete_db_files**: Deletes all database files at the specified path.

## Usage

To use the `Storage` struct for managing a key-value store, follow these steps:

1. **Create a StorageConfig**: 
   Define the path for your database and specify whether encryption is needed.

   ```rust
   let config = StorageConfig::new("path/to/database".to_string(), Some("encryption_key".to_string()));
   ```

2. **Initialize Storage**:
   Create a new `Storage` instance using the configuration.

   ```rust
   let storage = Storage::new(&config);
   ```

3. **Perform Operations**:
   Use the available methods to interact with the database.

   - **Write Data**:
     ```rust
     storage.write("key", "value");
     ```

   - **Read Data**:
     ```rust
     let value = storage.read("key");
     ```

   - **Get and Set Key-Value Pairs**:
     Use generic methods to get and set key-value pairs.

     ```rust
     let value: Option<YourType> = storage.get("key")?;
     storage.set("key", value, None)?;
     ```

   - **Delete Data**:
     ```rust
     storage.delete("key");
     ```

   - **Update Data**:
     Update an existing value with new data.

     ```rust
     let updates = HashMap::new();
     updates.insert("field", serde_json::json!("new_value"));
     let updated_value: YourType = storage.update("key", &updates, None)?;
     ```

   - **Check Key Existence**:
     Verify if a specific key exists in the database.

     ```rust
     let exists = storage.has_key("key")?;
     ```

4. **Check Database State**:
   Verify if the database is empty or retrieve keys.

   ```rust
   if storage.is_empty() {
   } else {
       let keys = storage.keys();
   }
   ```


5. **Transaction Management**:
     Begin a transaction, perform operations, and commit or rollback as needed.

     ```rust
     let transaction_id = storage.begin_transaction();
     storage.transactional_write("key", "value", transaction_id);
     storage.transactional_delete("key", transaction_id)?;
     storage.commit_transaction(transaction_id)?;
     storage.rollback_transaction(transaction_id)?;
     ```

6. **Advanced Operations**:

   - **Partial Key Comparison**:
     Retrieve keys or key-value pairs that start with a specific prefix.

     ```rust
     let keys_with_prefix = storage.partial_compare_keys("prefix")?;
     let key_value_pairs = storage.partial_compare("prefix")?;
     ```

   - **Open Existing Storage**:
     Open an existing storage instance.

     ```rust
     let open_storage = Storage::open(&config)?;
     ```

   - **Delete Database Files**:
     Remove all database files at a specified path.

     ```rust
     Storage::delete_db_files(&PathBuf::from("path/to/database"))?;
     ```

## Contributing
Contributions are welcome! Please open an issue or submit a pull request on GitHub.

## License

This project is licensed under the MIT License - see [LICENSE](LICENSE) file for details.

---

## 🧩 Part of the BitVMX Ecosystem

This repository is a component of the **BitVMX Ecosystem**, an open platform for disputable computation secured by Bitcoin.  
You can find the index of all BitVMX open-source components at [**FairgateLabs/BitVMX**](https://github.com/FairgateLabs/BitVMX).

---
