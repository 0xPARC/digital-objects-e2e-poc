use std::path::{Path, PathBuf};

use anyhow::bail;
use app::{Config, eth::send_payload, log_init};
use clap::{Parser, Subcommand};
use commitlib::{ItemBuilder, ItemDef, predicates::CommitPredicates};
use common::{
    load_dotenv,
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use craftlib::{
    constants::{COPPER_BLUEPRINT, COPPER_MINING_MAX, COPPER_WORK},
    item::{CraftBuilder, MiningRecipe},
    predicates::ItemPredicates,
};
use pod2::{
    backends::plonky2::mainpod::Prover,
    frontend::{MainPod, MainPodBuilder},
    middleware::{DEFAULT_VD_SET, Params, RawValue, Value, containers::Set},
};
use pod2utils::macros::BuildContext;
use serde::{Deserialize, Serialize};
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
        #[arg(long, value_name = "VALUE")]
        key: Value,
        #[arg(long, value_name = "RECIPE")]
        recipe: String,
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
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
            output,
            key,
            recipe,
        }) => {
            craft_item(&params, key, &recipe, &output)?;
        }
        Some(Commands::Commit { input }) => {
            commit_item(&params, &cfg, &input).await?;
        }
        Some(Commands::Verify { input }) => {
            let mut file = std::fs::File::open(&input)?;
            let crafted_item: CraftedItem = serde_json::from_reader(&mut file)?;
            crafted_item.pod.pod.verify()?;
            println!("Crafted item at {input:?} successfully verified!");
            // TODO: Verify that the item exists on-chain
        }
        None => {}
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CraftedItem {
    pub pod: MainPod,
    pub def: ItemDef,
}

fn craft_item(params: &Params, key: Value, recipe: &str, output: &Path) -> anyhow::Result<()> {
    let key = key.raw();
    println!("About to mine \"{recipe}\"");
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
        unknown => bail!("Unknown recipe for \"{unknown}\""),
    };

    let commit_preds = CommitPredicates::compile(params);
    let mut batches = commit_preds.defs.batches.clone();
    let item_preds = ItemPredicates::compile(params, &commit_preds);
    batches.extend_from_slice(&item_preds.defs.batches);

    // TODO
    let vd_set = &*DEFAULT_VD_SET;

    let mut builder = MainPodBuilder::new(&Default::default(), vd_set);

    let ctx = BuildContext {
        builder: &mut builder,
        batches: &batches,
    };
    let mut item_builder = ItemBuilder::new(ctx, params);
    let st_item_def = item_builder.st_item_def(item_def.clone())?;
    let ItemBuilder { ctx, .. } = item_builder;

    let mut craft_builder = CraftBuilder::new(ctx, params);
    let st_craft = match recipe {
        "copper" => craft_builder.st_is_copper(item_def.clone(), st_item_def)?,
        unknown => unreachable!("recipe {unknown}"),
    };
    let CraftBuilder { ctx, .. } = craft_builder;
    ctx.builder.reveal(&st_craft);
    let prover = &Prover {};
    let pod = ctx.builder.prove(prover)?;
    pod.pod.verify().unwrap();

    let crafted_item = CraftedItem { pod, def: item_def };
    let mut file = std::fs::File::create(output)?;
    serde_json::to_writer(&mut file, &crafted_item)?;
    println!("Stored crafted item mined with recipe {recipe} to {output:?}");

    Ok(())
}

async fn commit_item(params: &Params, cfg: &Config, input: &Path) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(input)?;
    let crafted_item: CraftedItem = serde_json::from_reader(&mut file)?;

    let created_items: Set =
        reqwest::blocking::get(format!("{}/created_items", cfg.sync_url))?.json()?;

    let commit_preds = CommitPredicates::compile(params);
    let batches = &commit_preds.defs.batches;
    // TODO
    let vd_set = &*DEFAULT_VD_SET;

    let mut builder = MainPodBuilder::new(&Default::default(), vd_set);

    let ctx = BuildContext {
        builder: &mut builder,
        batches,
    };
    let mut item_builder = ItemBuilder::new(ctx, params);
    let st_item_def = item_builder.st_item_def(crafted_item.def.clone())?;
    let st_commit_creation = item_builder.st_commit_creation(
        crafted_item.def.clone(),
        created_items.clone(),
        st_item_def,
    )?;
    let ItemBuilder { ctx, .. } = item_builder;
    ctx.builder.reveal(&st_commit_creation);
    let prover = &Prover {};
    let pod = ctx.builder.prove(prover)?;
    pod.pod.verify().unwrap();

    let shrunk_main_pod_build = ShrunkMainPodSetup::new(params)
        .build()
        .expect("successful build");
    let shrunk_main_pod_proof = shrink_compress_pod(&shrunk_main_pod_build, pod.clone()).unwrap();

    let nullifiers = vec![]; // TODO
    let payload_bytes = Payload {
        proof: PayloadProof::Plonky2(Box::new(shrunk_main_pod_proof.clone())),
        item: RawValue::from(crafted_item.def.item_hash(params)?),
        created_items_root: RawValue::from(created_items.commitment()),
        nullifiers,
    }
    .to_bytes();

    let tx_hash = send_payload(cfg, payload_bytes).await?;

    println!("Committed item in tx={tx_hash}");

    Ok(())
}
