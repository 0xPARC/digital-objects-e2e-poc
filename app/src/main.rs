use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::bail;
use clap::{Parser, Subcommand};
use commitlib::{ItemDef, predicates::CommitPredicates};
use craftlib::{
    constants::{COPPER_BLUEPRINT, COPPER_MINING_MAX, COPPER_WORK},
    item::{MiningRecipe, prove_copper},
    predicates::ItemPredicates,
};
use pod2::{
    backends::plonky2::mainpod::Prover,
    frontend::MainPod,
    middleware::{DEFAULT_VD_SET, Params, Value},
};

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
        key: Value,
        recipe: String,
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // TODO: Read config from env file
    // NOTE from @arnaucube: this is already available at the main branch at the common crate:
    // https://github.com/0xPARC/digital-objects-e2e-poc/blob/main/common/src/lib.rs#L22 (and used
    // at the eth test), for the synchronizer would be mainly porting the Config struct from the
    // blob-e2e-poc (removing unused fields like dbpath)

    // TODO
    let params = Params::default();

    match cli.command {
        Some(Commands::Craft {
            output,
            key,
            recipe,
        }) => {
            craft_item(&params, key, &recipe, &output)?;
        }
        Some(Commands::Commit { input }) => {
            println!("TODO: commit item found at {input:?}");
        }
        Some(Commands::Verify { input }) => {
            let mut file = std::fs::File::open(&input)?;
            let crafting_pod: MainPod = serde_json::from_reader(&mut file)?;
            crafting_pod.pod.verify()?;
            println!("Item at {input:?} successfully verified!");
            // TODO: Verify that the item exists on-chain
        }
        None => {}
    }

    Ok(())
}

fn craft_item(params: &Params, key: Value, recipe: &str, output: &Path) -> anyhow::Result<()> {
    let key = key.raw();
    println!("About to mine \"{}\"", recipe);
    let item_def = match recipe {
        "copper" => {
            let mining_recipe = MiningRecipe::new_no_inputs(COPPER_BLUEPRINT.to_string());
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, COPPER_MINING_MAX)?
                .unwrap();

            ItemDef {
                ingredients: ingredients_def.clone(),
                work: COPPER_WORK,
            }
        }
        unknwon => bail!("Unknwon recipe for \"{}\"", unknwon),
    };

    let commit_preds = CommitPredicates::compile(params);
    let mut batches = commit_preds.defs.batches.clone();
    let item_preds = ItemPredicates::compile(params, &commit_preds);
    batches.extend_from_slice(&item_preds.defs.batches);

    let prover = &Prover {};

    // TODO
    let vd_set = &*DEFAULT_VD_SET;

    let crafting_pod = match recipe {
        "copper" => prove_copper(item_def, &batches, params, prover, vd_set)?,
        unknwon => unreachable!("recipe {}", unknwon),
    };

    let serialised_item_pod = serde_json::to_string(&crafting_pod)?;
    let mut file = std::fs::File::create(&output)?;
    file.write_all(serialised_item_pod.as_bytes())?;
    println!("Wrote item with recipe {recipe} to {output:?}!");

    Ok(())
}
