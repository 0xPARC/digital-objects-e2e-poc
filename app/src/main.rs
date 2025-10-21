use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Craft an item locally
    Craft {
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
        // TODO: Add more flags to define the crafting
    },
    /// Commit a crafted item on-chain
    Commit {
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
        // TODO: Add more flags maybe
    },
    /// Verify a committed item
    Verify {
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
        // TODO: Add more flags maybe
    },
}

fn main() {
    let cli = Cli::parse();

    // TODO: Read config from env file

    match &cli.command {
        Some(Commands::Craft { output }) => {
            println!("TODO: craft item and store at {:?}", output);
        }
        Some(Commands::Commit { input }) => {
            println!("TODO: commit item found at {:?}", input);
        }
        Some(Commands::Verify { input }) => {
            println!("TODO: verify item found at {:?}", input);
        }
        None => {}
    }
}
