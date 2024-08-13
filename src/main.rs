use rust_bitvmx_storage_backend::clap::{Args, run};
use clap::Parser;

fn main() {
    let args = Args::parse();

    match run(args) {
        Ok(_) => (),
        Err(e) => println!("{}", e),
    }
}
