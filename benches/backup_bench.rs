use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use rand::{rng, RngCore};
use storage_backend::{error::StorageError, storage::Storage, storage_config::StorageConfig};
use std::{env, path::PathBuf};

fn temp_storage() -> PathBuf {
    let dir = env::temp_dir();
    let mut rang = rng();
    let index = rang.next_u32();
    dir.join(format!("storage_{}.db", index))
}

fn create_path_and_storage(
    is_encrypted: bool,
) -> Result<(PathBuf, StorageConfig, Storage), StorageError> {
    let path = &temp_storage();

    let encrypt = if is_encrypted {
        Some("password".to_string())
    } else {
        None
    };

    let config = StorageConfig {
        path: path.to_string_lossy().to_string(),
        encrypt,
    };
    let storage = Storage::new(&config)?;

    Ok((path.clone(), config, storage))
}

fn delete_storage(
    path: &PathBuf,
    backup_path: Option<PathBuf>,
    storage: Storage,
) -> Result<(), StorageError> {
    drop(storage);
    Storage::delete_db_files(path)?;
    Storage::delete_backup_file(backup_path)?;
    Ok(())
}

fn write_db(storage: &Storage, number_of_items: usize) {
    for i in 0..number_of_items {
        storage.write(&format!("key_{}", i), &format!("value_{}", i)).unwrap();
    }
    
}

fn bench_create_a_storage(c: &mut Criterion) {
    let mut group = c.benchmark_group("storage");
    let number_of_items = 1_000_000;
    let (path, _, storage) = create_path_and_storage(false).unwrap();

    group.bench_function(BenchmarkId::new("create_storage", number_of_items), |b| {
        b.iter(|| {
            write_db(&storage, number_of_items);
        });
    });

    delete_storage(&path, None, storage).unwrap();

    group.finish();
}

fn bench_create_backup(c: &mut Criterion) {
    let mut group = c.benchmark_group("backup");
    let number_of_items = 1_000_000;
    let backup_path = temp_storage();

    let (storage_path, _, storage) = create_path_and_storage(false).unwrap();
    write_db(&storage, number_of_items);

    group.bench_function(BenchmarkId::new("create_backup", number_of_items), |b| {
        b.iter(|| {
            storage.backup(backup_path.clone()).unwrap();
        });
    });

    delete_storage(&storage_path, Some(backup_path), storage).unwrap();
    group.finish();
}

fn bench_restore_backup(c: &mut Criterion) {
    let mut group = c.benchmark_group("backup");
    let number_of_items = 1_000_000;
    let backup_path = temp_storage();

    let (storage_path, config, storage) = create_path_and_storage(false).unwrap();
    write_db(&storage, number_of_items);
    storage.backup(backup_path.clone()).unwrap();
    delete_storage(&storage_path, None, storage).unwrap();
    let storage = Storage::new(&config).unwrap();
    
    group.bench_function(BenchmarkId::new("restore_backup", number_of_items), |b| {
        b.iter(|| {
            storage.restore_backup(&backup_path).unwrap();
        });
    });

    delete_storage(&storage_path, Some(backup_path), Storage::open(&config).unwrap()).unwrap();

    group.finish();
}

criterion_group!(benches, bench_create_a_storage, bench_create_backup, bench_restore_backup);
criterion_main!(benches);