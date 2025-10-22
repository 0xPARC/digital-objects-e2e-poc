use std::{
    io::{Read, Write},
    ops::Range,
    path::PathBuf,
};

use clap::{Parser, Subcommand};
use commitlib::{ItemDef, build_st_item_def, predicates::CommitPredicates};
use craftlib::{item::MiningRecipe, predicates::ItemPredicates};
use pod2::{
    backends::plonky2::mainpod::Prover,
    frontend::{MainPod, MainPodBuilder},
    middleware::{DEFAULT_VD_SET, Params, Value},
};
use pod2utils::macros::BuildContext;

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
        blueprint: String,
        start_seed: i64,
        end_seed: i64,
        mining_range_min: u64,
        mining_range_max: u64,
        work: Value, // TODO: Add more flags to define the crafting
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
            blueprint,
            start_seed,
            end_seed,
            mining_range_min,
            mining_range_max,
            work,
        }) => {
            let item_pod = craft_item(
                &params,
                key,
                &blueprint,
                start_seed,
                end_seed,
                mining_range_min..mining_range_max,
                work,
            )?;
            let serialised_item_pod = serde_json::to_string(&item_pod)?;
            let mut file = std::fs::File::create(&output)?;
            file.write_all(serialised_item_pod.as_bytes())?;
            println!("Wrote item with blueprint {blueprint} to {output:?}!");
        }
        Some(Commands::Commit { input }) => {
            println!("TODO: commit item found at {input:?}");
        }
        Some(Commands::Verify { input }) => {
            let mut file = std::fs::File::open(&input)?;
            let mut serialised_item_pod = String::new();
            file.read_to_string(&mut serialised_item_pod)?;
            let item_pod: MainPod = serde_json::from_str(&serialised_item_pod)?;
            item_pod.pod.verify()?;
            println!("Item at {input:?} successfully verified!");
        }
        None => {}
    }

    Ok(())
}

fn craft_item(
    params: &Params,
    key: Value,
    blueprint: &str,
    start_seed: i64,
    end_seed: i64,
    mining_range: Range<u64>,
    work: Value,
) -> anyhow::Result<MainPod> {
    let commit_preds = CommitPredicates::compile(params);
    let mut batches = commit_preds.defs.batches.clone();
    let item_preds = ItemPredicates::compile(params, &commit_preds);
    batches.extend_from_slice(&item_preds.defs.batches);

    let prover = &Prover {};

    // TODO
    let vd_set = &*DEFAULT_VD_SET;

    // Mine with selected key.
    let key = key.raw();
    let mining_recipe = MiningRecipe::new_no_inputs(blueprint.to_string());
    let ingredients_def = mining_recipe
        .do_mining(params, key, start_seed..end_seed, mining_range)?
        .unwrap();

    let item_def = ItemDef {
        ingredients: ingredients_def.clone(),
        work: work.raw(),
    };

    // Create a POD with a single item definition.
    let mut builder = MainPodBuilder::new(&Default::default(), vd_set);
    let mut ctx = BuildContext {
        builder: &mut builder,
        batches: &batches,
    };
    build_st_item_def(&mut ctx, params, item_def.clone())?;
    Ok(builder.prove(prover)?)
}
