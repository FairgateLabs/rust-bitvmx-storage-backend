use std::{fs, path::Path, process::Command};
use storage_backend::{
    key::StorageKey,
    storage::{KeyValueStore, Storage},
    storage_config::StorageConfig,
};

const BIN: &str = env!("CARGO_BIN_EXE_rust-bitvmx-storage-backend");

fn temp_path(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("cli_repair_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

// Writes a key and flushes it to an SST file.
fn create_flushed_storage(path: &Path) {
    let config = StorageConfig::new(path.to_string_lossy().to_string(), None);
    let storage = Storage::new(&config).unwrap();
    storage
        .set(StorageKey::try_from("key0").unwrap(), "test_value", None)
        .unwrap();
    drop(storage);
    let db = rocksdb::DB::open_default(path).unwrap();
    db.flush().unwrap();
}

fn repair(path: &Path) -> std::process::Output {
    Command::new(BIN)
        .args(["repair", "--storage-path"])
        .arg(path)
        .output()
        .unwrap()
}

#[test]
fn repair_exits_zero_when_nothing_is_lost() {
    let path = temp_path("healthy");
    create_flushed_storage(&path);

    let output = repair(&path);

    assert_eq!(output.status.code(), Some(0));
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn repair_exits_two_and_lists_lost_files() {
    let path = temp_path("lost");
    create_flushed_storage(&path);
    let sst = fs::read_dir(&path)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|ext| ext == "sst"))
        .unwrap();
    let mut bytes = fs::read(&sst).unwrap();
    for byte in &mut bytes[8..16] {
        *byte ^= 0xFF;
    }
    fs::write(&sst, bytes).unwrap();

    let output = repair(&path);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(".sst"), "stderr: {stderr}");
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn failing_repair_exits_one() {
    let path = temp_path("missing");

    let output = repair(&path);

    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
}
