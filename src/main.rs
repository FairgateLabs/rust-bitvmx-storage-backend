use clap::Parser;
use std::process::ExitCode;
mod cli;
#[cfg(feature = "ui")]
mod tui;
use cli::{run, Cli};

fn main() -> ExitCode {
    let args = Cli::parse();

    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}
