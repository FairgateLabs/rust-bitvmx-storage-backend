use clap::Parser;
use crate::{error::StorageError, storage::Storage};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    ///Action to perform
    #[clap(short, long)]
    action: Option<Action>,

    ///Path to the storage file
    #[clap(default_value = "storage.db")]
    storage_path: PathBuf,
}

use std::str::FromStr;

#[derive(Debug, Clone)]
enum Action {
    New,
    Write(String, String),
    Read(String),
    Delete(String),
    PartialCompare(String),
    Contains(String),
}

impl FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "new" | "n" => Ok(Action::New),
            "write" | "w" => Ok(Action::Write(String::new(), String::new())),
            "read" | "r" => Ok(Action::Read(String::new())),
            "delete" | "d" => Ok(Action::Delete(String::new())),
            "partial_compare" | "pc" => Ok(Action::PartialCompare(String::new())),
            "contains" | "c" => Ok(Action::Contains(String::new())),
            _ => Err(format!("Invalid action: {}", s)),
        }
    }
}

pub fn run(args: Cli) -> Result<(), String> {
    if args.storage_path.extension() != Some("db".as_ref()) {
        return Err(StorageError::PathError.to_string());
    }

    match args.action {
        Some(Action::New) => {
            Storage::new_with_path(&args.storage_path).map_err(|e| e.to_string())?;
            println!("Created new storage at {:?}", args.storage_path);
        }
        Some(Action::Write(key, value)) => {
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            storage.write(&key, &value).map_err(|e| e.to_string())?;
            println!("Wrote key {} with value {} to {:?}", key, value, args.storage_path);
        }
        Some(Action::Read(key)) => {
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            match storage.read(&key).map_err(|e| e.to_string())? {
                Some(value) => println!("Read key {} with value {} from {:?}", key, value, args.storage_path),
                None => println!("Key {} not found in {:?}", key, args.storage_path),
            }
        }
        Some(Action::Delete(key)) => {
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            storage.delete(&key).map_err(|e| e.to_string())?;
            println!("Deleted key {} from {:?}", key, args.storage_path);
        }
        Some(Action::PartialCompare(key)) => {
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            let keys = storage.partial_compare(&key).map_err(|e| e.to_string())?;
            println!("Keys partially matching {} in {:?}: {:?}", key, args.storage_path, keys);
        }
        Some(Action::Contains(key)) => {
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            let contains = storage.has_key(&key).map_err(|e| e.to_string())?;
            println!("Key {} {} in {:?}", key, if contains { "exists" } else { "does not exist" }, args.storage_path);
        }
        None => return Err("No action specified".to_string()),
        
    }

    Ok(())
}