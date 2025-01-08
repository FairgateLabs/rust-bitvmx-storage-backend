use clap::Parser;
use storage_backend::cli::{run, Cli};

fn main() {
    let args = Cli::parse();

    match run(args) {
        Ok(_) => (),
        Err(e) => println!("{}", e),
    }
}
