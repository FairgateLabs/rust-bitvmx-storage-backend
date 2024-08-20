use clap::Parser;
use crate::{error::StorageError, storage::Storage};
use std::{path::PathBuf, str::FromStr};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[clap(short, long)]
    action: Option<Action>,

    #[clap(index = 1)]
    key: Option<String>,

    #[clap(index = 2)]
    value: Option<String>,

    #[clap(index = 3, default_value = "storage.db")]
    storage_path: PathBuf,
}

#[derive(Debug, Clone)]
enum Action {
    New,
    Write,
    Read,
    Delete,
    PartialCompare,
    Contains,
}

impl FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "new" | "n" => Ok(Action::New),
            "write" | "w" => Ok(Action::Write),
            "read" | "r" => Ok(Action::Read),
            "delete" | "d" => Ok(Action::Delete),
            "partial_compare" | "pc" => Ok(Action::PartialCompare),
            "contains" | "c" => Ok(Action::Contains),
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
        Some(Action::Write) => {
            let key = args.key.ok_or("Key is required for write action")?;
            let value = args.value.ok_or("Value is required for write action")?;
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            storage.write(&key, &value).map_err(|e| e.to_string())?;
            println!("Wrote key {} with value {} to {:?}", key, value, args.storage_path);
        }
        Some(Action::Read) => {
            let key = args.key.ok_or("Key is required for read action")?;
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            match storage.read(&key).map_err(|e| e.to_string())? {
                Some(value) => println!("Read key {} with value {} from {:?}", key, value, args.storage_path),
                None => println!("Key {} not found in {:?}", key, args.storage_path),
            }
        }
        Some(Action::Delete) => {
            let key = args.key.ok_or("Key is required for delete action")?;
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            storage.delete(&key).map_err(|e| e.to_string())?;
            println!("Deleted key {} from {:?}", key, args.storage_path);
        }
        Some(Action::PartialCompare) => {
            let key = args.key.ok_or("Key is required for partialcompare action")?;
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            let keys = storage.partial_compare(&key).map_err(|e| e.to_string())?;
            println!("Keys partially matching {} in {:?}: {:?}", key, args.storage_path, keys);
        }
        Some(Action::Contains) => {
            let key = args.key.ok_or("Key is required for contains action")?;
            let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
            let contains = storage.has_key(&key).map_err(|e| e.to_string())?;
            println!("Key {} {} in {:?}", key, if contains { "exists" } else { "does not exist" }, args.storage_path);
        }
        None => return Err("No action specified".to_string()),
    }

    Ok(())
}