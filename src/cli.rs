use crate::{storage::Storage, storage_config::StorageConfig};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    action: Action,
}

#[derive(Parser, Debug, Clone)]
struct StorageSettings {
    #[clap(short, long, default_value = "storage.db")]
    storage_path: PathBuf,
    #[clap(short, long)]
    password: Option<String>,
}

#[derive(Parser, Debug, Clone)]
struct BackupPath {
    #[clap(short, long, default_value = "backup")]
    backup_path: PathBuf,
    #[clap(flatten)]
    storage_path: StorageSettings,
}

#[derive(Parser, Debug, Clone)]
struct StorageAndKey {
    #[clap(short, long)]
    key: String,
    #[clap(flatten)]
    storage_path: StorageSettings,
}

#[derive(Parser, Debug, Clone)]
struct StorageKeyValue {
    #[clap(short, long)]
    key: String,
    #[clap(short, long)]
    value: String,
    #[clap(flatten)]
    storage_path: StorageSettings,
}

#[derive(Subcommand, Debug)]
enum Action {
    New(StorageSettings),
    Write(StorageKeyValue),
    Read(StorageAndKey),
    Delete(StorageAndKey),
    PartialCompare(StorageAndKey),
    Contains(StorageAndKey),
    ListKeys(StorageSettings),
    Backup(BackupPath),
    RestoreBackup(BackupPath),
    Dump {
        #[clap(flatten)]
        storage_path: StorageSettings,
        #[clap(short, long, default_value = "dump.json")]
        dump_file: PathBuf,
        #[clap(short, long, default_value = "false")]
        pretty: bool,
    },
}

impl Action {
    fn get_storage_path(&self) -> &PathBuf {
        match self {
            Action::New(args) => &args.storage_path,
            Action::Write(args) => &args.storage_path.storage_path,
            Action::Read(args) => &args.storage_path.storage_path,
            Action::Delete(args) => &args.storage_path.storage_path,
            Action::PartialCompare(args) => &args.storage_path.storage_path,
            Action::Contains(args) => &args.storage_path.storage_path,
            Action::ListKeys(args) => &args.storage_path,
            Action::Backup(args) => &args.storage_path.storage_path,
            Action::RestoreBackup(args) => &args.storage_path.storage_path,
            Action::Dump { storage_path, .. } => &storage_path.storage_path,
        }
    }

    fn get_encryption_password(&self) -> Option<String> {
        match self {
            Action::New(args) => args.password.clone(),
            Action::Write(args) => args.storage_path.password.clone(),
            Action::Read(args) => args.storage_path.password.clone(),
            Action::Delete(args) => args.storage_path.password.clone(),
            Action::PartialCompare(args) => args.storage_path.password.clone(),
            Action::Contains(args) => args.storage_path.password.clone(),
            Action::ListKeys(args) => args.password.clone(),
            Action::Backup(args) => args.storage_path.password.clone(),
            Action::RestoreBackup(args) => args.storage_path.password.clone(),
            Action::Dump { storage_path, .. } => storage_path.password.clone(),
        }
    }
}

pub fn run(args: Cli) -> Result<(), String> {
    let storage = match args.action {
        Action::New(storage_settings) => {
            let path = storage_settings.storage_path.to_string_lossy().to_string();
            let password = storage_settings.password;
            let config = StorageConfig::new(path, password, None);

            Storage::new(&config).map_err(|e| e.to_string())?;
            println!("Created new storage at {:?}", storage_settings.storage_path);
            return Ok(());
        }
        _ => {
            let config = StorageConfig::new(
                args.action.get_storage_path().to_string_lossy().to_string(),
                args.action.get_encryption_password(),
                None
            );
            Storage::open(&config).map_err(|e| e.to_string())?
        }
    };

    match args.action {
        Action::New(_) => {
            eprintln!("Already handled above");
        }
        Action::Write(storage_key_value) => {
            storage
                .write(&storage_key_value.key, &storage_key_value.value)
                .map_err(|e| e.to_string())?;
            println!(
                "Wrote key {} with value {} to {:?}",
                storage_key_value.key, storage_key_value.value, storage_key_value.storage_path
            );
        }
        Action::Read(storage_and_key) => {
            match storage
                .read(&storage_and_key.key)
                .map_err(|e| e.to_string())?
            {
                Some(value) => println!(
                    "Read key {} with value {} from {:?}",
                    storage_and_key.key, value, storage_and_key.storage_path
                ),
                None => println!(
                    "Key {} not found in {:?}",
                    storage_and_key.key, storage_and_key.storage_path
                ),
            }
        }
        Action::Delete(storage_and_key) => {
            storage
                .delete(&storage_and_key.key)
                .map_err(|e| e.to_string())?;
            println!(
                "Deleted key {} from {:?}",
                storage_and_key.key, storage_and_key.storage_path
            );
        }
        Action::PartialCompare(storage_and_key) => {
            let keys = storage
                .partial_compare(&storage_and_key.key)
                .map_err(|e| e.to_string())?;
            println!(
                "Keys partially matching {} in {:?}: {:?}",
                storage_and_key.key, storage_and_key.storage_path, keys
            );
        }
        Action::Contains(storage_and_key) => {
            let contains = storage
                .has_key(&storage_and_key.key)
                .map_err(|e| e.to_string())?;
            println!(
                "Key {} {} in {:?}",
                storage_and_key.key,
                if contains { "exists" } else { "does not exist" },
                storage_and_key.storage_path
            );
        }
        Action::ListKeys(storage_settings) => {
            let keys = storage.keys().map_err(|e| e.to_string())?;
            println!("Listing keys in: {:?}", storage_settings.storage_path);
            for key in keys {
                println!("{}", key);
            }
        }
        Action::Backup(backup) => {
            storage
                .backup(&backup.backup_path)
                .map_err(|e| e.to_string())?;
            println!("Backup created at {:?}", backup.backup_path);
        }
        Action::RestoreBackup(backup) => {
            storage
                .restore_backup(&backup.backup_path)
                .map_err(|e| e.to_string())?;
            println!("Backup restored from {:?}", backup.backup_path);
        }
        Action::Dump {
            storage_path: _,
            dump_file,
            pretty,
        } => {
            let keys = storage.keys().map_err(|e| e.to_string())?;
            let mut json_map = serde_json::Map::new();
            for key in keys {
                if let Some(value) = storage.read(&key).map_err(|e| e.to_string())? {
                    let json_value: serde_json::Value =
                        serde_json::from_str(&value).map_err(|e| e.to_string())?;
                    json_map.insert(key, json_value);
                }
            }
            println!("Dumped storage content to {:?}", dump_file);
            let json_data = serde_json::Value::Object(json_map);
            let mut file = File::create(dump_file).map_err(|e| e.to_string())?;
            if pretty {
                file.write_all(
                    serde_json::to_string_pretty(&json_data)
                        .map_err(|e| e.to_string())?
                        .as_bytes(),
                )
                .map_err(|e| e.to_string())?;
            } else {
                file.write_all(json_data.to_string().as_bytes())
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}
