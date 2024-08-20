use rust_bitvmx_storage_backend::cli::{Cli, run};
use clap::Parser;

fn main() {
    let args = Cli::parse();

    match run(args) {
        Ok(_) => (),
        Err(e) => println!("{}", e),
    }
}
