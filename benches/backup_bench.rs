use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::{rng, RngCore};
use std::{env, path::PathBuf};
use storage_backend::{error::StorageError, storage::Storage, storage_config::StorageConfig};

fn temp_storage() -> PathBuf {
    let dir = env::temp_dir();
    let mut rang = rng();
    let index = rang.next_u32();
    dir.join(format!("storage_{}.db", index))
}

fn backup_temp_storage() -> PathBuf {
    let dir = env::temp_dir();
    let mut rang = rng();
    let index = rang.next_u32();
    dir.join(format!("backup_{}", index))
}

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
        password,
    };
    let storage = Storage::new(&config)?;

    Ok((path.clone(), config, storage))
}

fn delete_storage(path: &PathBuf, storage: Storage) -> Result<(), StorageError> {
    drop(storage);
    Storage::delete_db_files(path)?;
    Ok(())
}

fn write_db(storage: &Storage, number_of_items: usize) {
    let tx = storage.begin_transaction();
    for i in 0..number_of_items {
        storage
            .transactional_write(&format!("key_{}", i), &format!("value_{}", i), tx)
            .unwrap();
    }
    storage.commit_transaction(tx).unwrap();
}

fn bench_create_storage(c: &mut Criterion) {
    let mut group = c.benchmark_group("backup");
    let number_of_items = 1_000_000;
    let (path, _, storage) = create_path_and_storage(false).unwrap();

    group.sample_size(10).bench_function(
        BenchmarkId::new("create_storage", number_of_items),
        |b| {
            b.iter(|| {
                write_db(&storage, number_of_items);
            });
        },
    );

    delete_storage(&path, storage).unwrap();
    group.finish();
}

fn bench_create_backup(c: &mut Criterion) {
    let mut group = c.benchmark_group("backup");
    let number_of_items = 1_000_000;
    let backup_path = backup_temp_storage();

    let (storage_path, _, storage) = create_path_and_storage(false).unwrap();
    write_db(&storage, number_of_items);

    group
        .sample_size(10)
        .bench_function(BenchmarkId::new("create_backup", number_of_items), |b| {
            b.iter(|| {
                storage.backup(backup_path.clone()).unwrap();
            });
        });

    delete_storage(&storage_path, storage).unwrap();
    Storage::delete_backup_file(backup_path).unwrap();
    group.finish();
}

fn bench_restore_backup(c: &mut Criterion) {
    let mut group = c.benchmark_group("backup");
    let number_of_items = 1_000_000;
    let backup_path = backup_temp_storage();

    let (storage_path, _, storage) = create_path_and_storage(false).unwrap();
    write_db(&storage, number_of_items);
    storage.backup(backup_path.clone()).unwrap();
    delete_storage(&storage_path, storage).unwrap();
    let (path, _, store) = create_path_and_storage(false).unwrap();

    group.sample_size(10).bench_function(
        BenchmarkId::new("restore_backup", number_of_items),
        |b| {
            b.iter(|| {
                store.restore_backup(&backup_path).unwrap();
            });
        },
    );

    delete_storage(&path, store).unwrap();
    Storage::delete_backup_file(backup_path).unwrap();
    group.finish();
}

criterion_group!(
    benches,
    bench_create_storage,
    bench_create_backup,
    bench_restore_backup
);
criterion_main!(benches);
