use rust_bitvmx_storage_backend::{storage::Storage, error::StorageError};
use std::path::PathBuf;
use clap::{Parser, ArgAction};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// create a new storage file
    #[arg(short, long, action = ArgAction::SetTrue)]
    new: bool,
    
    /// Write a value from the storage file
    #[arg(short, long, num_args = 2, value_names = &["KEY", "VALUE_TO_STORE"])]
    write: Option<Vec<String>>,

    /// Does a partial comparison of the key and returns all the keys that start with the given key
    #[arg(short, long, value_names = &["KEY"])]
    partial_compare: Option<String>,

    /// Read a value from the storage file
    #[arg(short, long, value_names = &["KEY"])]
    read: Option<String>,

    /// Delete a value from the storage file
    #[arg(short, long, value_names = &["KEY"])]
    delete: Option<String>,

    ///Find if the key is already in the storage file
    #[arg(short, long, value_names = &["KEY"])]
    contains: Option<String>,

    ///Path to the storage file
    #[clap(default_value = "storage.db")]
    storage_path: PathBuf,
}

fn overlapping_arguments(args: &Args) -> bool {
    let mut count = 0;
    if args.new { count += 1; }
    if args.write.is_some() { count += 1; }
    if args.read.is_some() { count += 1; }
    if args.delete.is_some() { count += 1; }
    if args.partial_compare.is_some() { count += 1; }
    if args.contains.is_some() { count += 1; }

    count > 1
}

fn run(args: Args) -> Result<(), String>{
    if args.storage_path.extension() != Some("db".as_ref()) {
        return Err(StorageError::PathError.to_string());
    }

    if overlapping_arguments(&args) {
        return Err("Cannot use multiple arguments at the same time".to_string());
    }

    if args.new {
        if args.storage_path == PathBuf::from("storage.db") {
            Storage::new().map_err(|e| e.to_string())?;
        } else {
            Storage::new_with_path(&args.storage_path).map_err(|e| e.to_string())?;    
        }

    } else if let Some(write) = args.write {
        let key = write[0].as_str();
        let value = write[1].as_str();

        let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
        if storage.has_key(key).map_err(|e| e.to_string())? {
            return Err("Key already exists".to_string());
        } 

        storage.write(key, value).map_err(|e| e.to_string())?;

    } else if let Some(read) = args.read {
        let key = read.as_str();
        let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
        
        if storage.is_empty() {
            return Err("Storage file is empty".to_string());
        }
        
        match storage.read(key) {
            Ok(Some(value)) => println!("{}", value),
            Ok(None) => println!("Key not found"),
            Err(e) => return Err(e.to_string()),     
        }

    } else if let Some(delete) = args.delete {
        let key = delete.as_str();
        let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
        storage.delete(key).map_err(|e| e.to_string())?;
        println!("Key deleted correctly");

    } else if let Some(partial_compare) = args.partial_compare {
        let key = partial_compare.as_str();
        let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
        storage.partial_compare(key).map_err(|e| e.to_string())? 
            .iter()
            .for_each(|(key, value)| println!("Key: {}, Value: {}", key, value)); 

    } else if let Some(contains) = args.contains {
        let key = contains.as_str();
        let storage = Storage::open(&args.storage_path).map_err(|e| e.to_string())?;
        match storage.has_key(key).map_err(|e| e.to_string())? {
            true => println!("Key exists"),
            false => println!("Key does not exist"),
        }

    } else {
        return Err("No action provided".to_string());
    }

    Ok(())
}

fn main() {
    let args = Args::parse();

    match run(args) {
        Ok(_) => (),
        Err(e) => println!("{}", e),
    }
}