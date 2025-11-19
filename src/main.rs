use clap::Parser;
mod cli;
use cli::{run, Cli};

fn main() {
    let args = Cli::parse();

    match run(args) {
        Ok(_) => (),
        Err(e) => println!("{}", e),
    }
}
