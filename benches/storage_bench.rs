use criterion::{criterion_group, criterion_main, Criterion};
use rand::{rng, RngCore};
use std::{env, fs, path::PathBuf, time::Duration};
use storage_backend::{storage::Storage, storage_config::StorageConfig};

fn temp_storage() -> PathBuf {
    let dir = env::temp_dir();
    let mut rng = rng();
    let index = rng.next_u32();
    dir.join(format!("storage_{}.db", index))
}

fn setup_database_with_prefix_extractor(storage_path: &PathBuf) -> Storage {
    let storage_config = StorageConfig::new(storage_path.to_string_lossy().to_string(), None);
    let db = Storage::new(&storage_config).unwrap();
    write_data(&db);
    db
}

fn setup_database_without_prefix_extractor(storage_path: &PathBuf) -> Storage {
    let storage_config = StorageConfig::new(storage_path.to_string_lossy().to_string(), None);
    let db = Storage::new(&storage_config).unwrap();
    write_data(&db);
    db
}

fn write_data(db: &Storage) {
    for i in 0..1000 {
        for j in 0..100 {
            for k in 0..1000 {
                let key = format!("bitvmx/{}/topic_{}/value_{}", i, j, k);
                let value = format!("{}", k);
                db.write(&key, &value).unwrap();
            }
        }
    }
}

fn access_key_benchmark(
    c: &mut Criterion,
    storage: &Storage,
    key_to_access: &str,
    variant_name: &str,
) {
    c.bench_function(
        &format!("rocksdb get {} ({})", key_to_access, variant_name),
        |b| {
            b.iter(|| {
                let _ = storage.read(key_to_access);
            })
        },
    );
}

fn random_keys(n: usize) -> Vec<String> {
    let mut keys = Vec::with_capacity(n);
    let mut rng = rng();

    for _ in 0..n {
        let mut key;
        loop {
            let i = rng.next_u32() % 1000;
            let j = rng.next_u32() % 100;
            let k = rng.next_u32() % 1000;
            key = format!("bitvmx/{}/topic_{}/value_{}", i, j, k);
            if !keys.contains(&key) {
                break;
            }
        }

        keys.push(key);
    }

    keys
}

fn criterion_benchmark(_c: &mut Criterion) {
    let mut criterion = Criterion::default().measurement_time(Duration::from_secs(10));
    println!("Generating random keys to access");
    let keys_to_access = random_keys(1000);
    let mut i = 1;
    println!("Generating storage with prefix extractor");
    let storage_path_1 = temp_storage();
    let storage_with_prefix_extractor = setup_database_with_prefix_extractor(&storage_path_1);
    println!("Generating storage without prefix extractor");
    let storage_path_2 = temp_storage();
    let storage_without_prefix_extractor = setup_database_without_prefix_extractor(&storage_path_2);

    for key in keys_to_access {
        println!("Benchmarking key {} ({})", key, i);
        access_key_benchmark(
            &mut criterion,
            &storage_with_prefix_extractor,
            &key,
            "Variant 1",
        );
        access_key_benchmark(
            &mut criterion,
            &storage_without_prefix_extractor,
            &key,
            "Variant 2",
        );
        i += 1;
    }

    drop(storage_with_prefix_extractor);
    fs::remove_dir_all(&storage_path_1).unwrap();
    drop(storage_without_prefix_extractor);
    fs::remove_dir_all(&storage_path_2).unwrap();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
