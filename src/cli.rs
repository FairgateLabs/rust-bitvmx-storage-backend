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
struct StoragePath {
    #[clap(short, long, default_value = "storage.db")]
    storage_path: PathBuf,
}

#[derive(Parser, Debug, Clone)]
struct StorageAndKey {
    #[clap(short, long)]
    key: String,
    #[clap(flatten)]
    storage_path: StoragePath,
}

#[derive(Parser, Debug, Clone)]
struct StorageKeyValue {
    #[clap(short, long)]
    key: String,
    #[clap(short, long)]
    value: String,
    #[clap(flatten)]
    storage_path: StoragePath,
}

#[derive(Subcommand, Debug)]
enum Action {
    New(StoragePath),
    Write(StorageKeyValue),
    Read(StorageAndKey),
    Delete(StorageAndKey),
    PartialCompare(StorageAndKey),
    Contains(StorageAndKey),
    ListKeys(StoragePath),
    Dump {
        #[clap(flatten)]
        storage_path: StoragePath,
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
            Action::Dump { storage_path, .. } => &storage_path.storage_path,
        }
    }
}

pub fn run(args: Cli) -> Result<(), String> {
    let storage = match args.action {
        Action::New(storage_path) => {
            let path = storage_path.storage_path.to_string_lossy().to_string();
            let config = StorageConfig::new(path, None);

            Storage::new(&config).map_err(|e| e.to_string())?;
            println!("Created new storage at {:?}", storage_path.storage_path);
            return Ok(());
        }
        _ => {
            let config = StorageConfig::new(
                args.action.get_storage_path().to_string_lossy().to_string(),
                None,
            );
            Storage::open(&config).map_err(|e| e.to_string())?
        }
    };

    match args.action {
        Action::New(storage_path) => {
            let config = StorageConfig::new(
                storage_path.storage_path.to_string_lossy().to_string(),
                None,
            );
            Storage::new(&config).map_err(|e| e.to_string())?;
            println!("Created new storage at {:?}", storage_path.storage_path);
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
        Action::ListKeys(_storage) => {
            let keys = storage.keys().map_err(|e| e.to_string())?;
            println!("Listing keys in: {:?}", _storage.storage_path);
            for key in keys {
                println!("{}", key);
            }
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
