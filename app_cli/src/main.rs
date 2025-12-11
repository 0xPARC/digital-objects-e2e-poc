//! Examples of usage
//!
//! - craft new stone item:
//!   RUST_LOG=app=debug cargo run --release -p app_cli -- craft --output ./item0 --recipe stone
//! - commit the crafted item:
//!   RUST_LOG=app=debug cargo run --release -p app_cli -- commit --input ./item0
use std::{path::PathBuf, str::FromStr};

use app_cli::{Config, Recipe, commit_item, craft_item, load_item};
use clap::{Parser, Subcommand};
use common::{load_dotenv, log_init};
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{Params, RawValue, containers::Set},
};
use tracing::info;

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
        #[arg(long, value_name = "RECIPE")]
        recipe: String,
        #[arg(long = "output", value_name = "FILE")]
        outputs: Vec<PathBuf>,
        #[arg(long = "input", value_name = "FILE")]
        inputs: Vec<PathBuf>,
    },
    /// Commit a crafted item on-chain
    Commit {
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
    },
    /// Verify a committed item
    Verify {
        #[arg(long, value_name = "FILE")]
        input: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    log_init();
    load_dotenv()?;
    let cfg = Config::from_env()?;
    info!(?cfg, "Loaded config");

    let params = Params::default();

    match cli.command {
        Some(Commands::Craft {
            recipe,
            outputs,
            inputs,
        }) => {
            let recipe = Recipe::from_str(&recipe)?;
            craft_item(&params, recipe, &outputs, &inputs)?;
        }
        Some(Commands::Commit { input }) => {
            commit_item(&params, &cfg, &input).await?;
        }
        Some(Commands::Verify { input }) => {
            let crafted_item = load_item(&input)?;

            // Verify that the item exists on-blob-space:
            // first get the merkle proof of item existence from the Synchronizer
            let item = RawValue::from(crafted_item.def.item_hash(&params)?);

            // Single item => set containing one element
            // TODO: Generalise.
            let item_set_hex: String = format!("{item:#}");
            let (epoch, _): (u64, RawValue) =
                reqwest::blocking::get(format!("{}/created_items_root", cfg.sync_url,))?.json()?;
            info!("Verifying commitment of item {item:#} via synchronizer at epoch {epoch}");
            let (epoch, mtp): (u64, MerkleProof) = reqwest::blocking::get(format!(
                "{}/created_item/{}",
                cfg.sync_url,
                &item_set_hex[2..]
            ))?
            .json()?;
            info!("mtp at epoch {epoch}: {mtp:?}");

            // fetch the associated Merkle root
            let merkle_root: RawValue =
                reqwest::blocking::get(format!("{}/created_items_root/{}", cfg.sync_url, &epoch))?
                    .json()?;

            // verify the obtained merkle proof
            Set::verify(
                params.max_depth_mt_containers,
                merkle_root.into(),
                &mtp,
                &item.into(),
            )?;

            info!("Crafted item at {input:?} successfully verified!");
        }
        None => {}
    }

    Ok(())
}
