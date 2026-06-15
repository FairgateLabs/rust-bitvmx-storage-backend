# AGENTS.md

Guidance for coding agents working in this repository.

## Project overview

- This is a Rust 2021 crate named `rust-bitvmx-storage-backend`.
- The library target is `storage_backend`; the CLI binary is implemented via `src/main.rs` and `src/cli.rs`.
- Core functionality lives in `src/storage.rs`: a RocksDB-backed key-value store with optional encryption, transactions, backup/restore, password changes, and prefix queries.
- Public modules are declared in `src/lib.rs`:
  - `error` (`StorageError`)
  - `password_policy` (`PasswordPolicy`)
  - `storage` (`Storage`, `KeyValueStore`)
  - `storage_config` (`StorageConfig`, `PasswordPolicyConfig`)
- `src/backup_io.rs` is crate-private and wraps `age` stream encryption/decryption for backup files.
- Benchmarks live in `benches/backup_bench.rs` and use Criterion.

## Build and test commands

Run these from the repository root:

```sh
cargo fmt
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo bench --bench backup_bench
```

Notes:
- RocksDB/LLVM system dependencies may be required before builds work. See `INSTALLATION.md`.
- If RocksDB is installed system-wide, keep `ROCKSDB_LIB_DIR`, `ROCKSDB_INCLUDE_DIR`, and `LIBCLANG_PATH` configured as documented in `INSTALLATION.md`.
- Benchmarks write large temporary RocksDB databases and backup files; do not run them unless requested.

## Coding conventions

- Keep code formatted with `cargo fmt`.
- Prefer returning `StorageError` variants over panics or string errors in library code.
- Keep the library API generic over `serde::Serialize` / `serde::de::DeserializeOwned` where existing traits use those bounds.
- Use `redact::Secret<String>` for passwords and avoid logging or printing secret values.
- Do not expose `backup_io` publicly unless there is a deliberate API decision to do so.
- Preserve the CLI's Clap-based structure in `src/cli.rs` when adding commands.

## Storage and transaction caveats

- `Storage` is intentionally single-threaded. It uses `RefCell` for transaction management and stores RocksDB transactions with an unsafe lifetime extension. Do not add `Send`/`Sync` assumptions or multi-threaded access without redesigning transaction handling.
- There are two transaction modes:
  - explicit transactions created with `begin_transaction()` and passed as `Some(Uuid)`
  - a global transaction created with `begin_global_transaction()` and used by operations whose transaction argument is `None`
- Always commit or roll back transactions in new code and tests.
- Encrypted storage stores a DEK under the internal key `DEK`; take care not to treat it as normal user data.

## Backup and password caveats

- Storage encryption passwords and backup passwords are separate concepts.
- Backups create two files: the encrypted backup and a DEK file encrypted with the backup password. Restore requires both files and the backup password.
- Backup passwords are validated with the active `PasswordPolicy`; tests that use simple passwords may need an explicit relaxed `PasswordPolicyConfig` if validation is involved.

## Testing guidance

- Unit tests are currently colocated in `src/storage.rs` and use random paths under the system temp directory.
- Clean up temporary RocksDB directories with `Storage::delete_db_files(storage)` when possible.
- Clean up backup/dek files created during tests.
- Prefer focused tests for storage semantics: encryption on/off, transaction commit/rollback, global transaction behavior, prefix iteration, backup/restore, and password policy errors.

## Repository hygiene

- Do not commit `target/`, temporary RocksDB directories, backup/dek artifacts, or benchmark output.
- `Cargo.lock` exists in this checkout, but `.gitignore` also contains a generated Cargo.lock ignore comment. Check project policy before changing lockfile handling.
