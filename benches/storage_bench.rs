use criterion::{criterion_group, criterion_main, Criterion};
use rust_bitvmx_storage_backend::storage::{Storage, get_prefix_extractor};
use rocksdb::Options;
use rand::{thread_rng, RngCore};
use std::{env, path::PathBuf, time::Duration};

fn temp_storage() -> PathBuf {
    let dir = env::temp_dir();
    let mut rng = thread_rng();
    let index = rng.next_u32();
    dir.join(format!("storage_{}.db", index))
}

fn setup_database_with_prefix_extractor() -> Storage {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.set_prefix_extractor(get_prefix_extractor());

    let db = Storage::new_with_path_and_option(&temp_storage(), opts).unwrap();

    write_data(&db);

    db
}

fn setup_database_without_prefix_extractor() -> Storage {
    let mut opts = Options::default();
    opts.create_if_missing(true);

    let db = Storage::new_with_path_and_option(&temp_storage(), opts).unwrap();


    write_data(&db);

    db
}

fn write_data(db: &Storage) {
    for i in 0..1000 {
        for j in 0..100{
            for k in 0..1000 {
                let key = format!("bitvmx/{}/topic_{}/value_{}", i,j,k);
                let value = format!("{}", k);
                db.write(&key, &value).unwrap();
            }
        } 
    }
}

fn access_key_benchmark(c: &mut Criterion, db_setup: fn() -> Storage, key_to_access: &str, variant_name: &str) {
    let db = db_setup();

    c.bench_function(&format!("rocksdb get {} ({})", key_to_access, variant_name), |b| {
        b.iter(|| {
            let _ = db.read(key_to_access);
        })
    });

    drop(db);
}

fn random_keys(n: usize) -> Vec<String> {
    let mut keys = Vec::with_capacity(n);
    let mut rng = thread_rng();
    

    for _ in 0..n {
        let mut key;
        loop {
            let i = rng.next_u32() % 1000;
            let j = rng.next_u32() % 100;
            let k = rng.next_u32() % 1000;
            key = format!("bitvmx/{}/topic_{}/value_{}", i,j,k);
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

    for key in keys_to_access {
        access_key_benchmark(&mut criterion, setup_database_with_prefix_extractor, &key,"Variant 1");
        access_key_benchmark(&mut criterion, setup_database_without_prefix_extractor,  &key,"Variant 2");
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);