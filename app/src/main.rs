//! Examples of usage
//!
//! - craft new copper item:
//!   RUST_LOG=app=debug cargo run --release -p app -- craft --output ./item0 --key key0 --recipe copper
//! - commit the crafted item:
//!   RUST_LOG=app=debug cargo run --release -p app -- commit --input ./item0

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use alloy::network::any;
use anyhow::bail;
use app::{Config, eth::send_payload, log_init};
use clap::{Parser, Subcommand};
use commitlib::{IngredientsDef, ItemBuilder, ItemDef, predicates::CommitPredicates};
use common::{
    load_dotenv,
    payload::{Payload, PayloadProof},
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use craftlib::{
    constants::{
        BRONZE_BLUEPRINT, BRONZE_MINING_MAX, BRONZE_WORK, COPPER_BLUEPRINT, COPPER_MINING_MAX,
        COPPER_WORK, TIN_BLUEPRINT, TIN_MINING_MAX, TIN_WORK,
    },
    item::{CraftBuilder, MiningRecipe},
    predicates::ItemPredicates,
};
use pod2::{
    backends::plonky2::{
        mainpod::Prover, mock::mainpod::MockProver, primitives::merkletree::MerkleProof,
    },
    frontend::{MainPod, MainPodBuilder},
    middleware::{
        CustomPredicateBatch, DEFAULT_VD_SET, MainPodProver, Params, RawValue, VDSet, Value,
        containers::Set,
    },
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
        #[arg(long = "input", value_name = "FILE")]
        inputs: Vec<PathBuf>,
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
            key,
            recipe,
            output,
            inputs,
        }) => {
            craft_item(&params, key, &recipe, &output, &inputs)?;
        }
        Some(Commands::Commit { input }) => {
            commit_item(&params, &cfg, &input).await?;
        }
        Some(Commands::Verify { input }) => {
            let crafted_item = load_item(&input)?;

            // Verify that the item exists on-blob-space:
            // first get the merkle proof of item existence from the Synchronizer
            let item = RawValue::from(crafted_item.def.item_hash(&params)?);
            let item_hex: String = format!("{item:#}");
            let (epoch, mtp): (u64, MerkleProof) = reqwest::blocking::get(format!(
                "{}/created_item/{}",
                cfg.sync_url,
                &item_hex[2..]
            ))?
            .json()?;
            println!("mtp at epoch {epoch}: {mtp:?}");

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

            println!("Crafted item at {input:?} successfully verified!");
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

struct Helper {
    params: Params,
    vd_set: VDSet,
    batches: Vec<Arc<CustomPredicateBatch>>,
}

impl Helper {
    fn new(params: Params, vd_set: VDSet) -> Self {
        let commit_preds = CommitPredicates::compile(&params);
        let mut batches = commit_preds.defs.batches.clone();
        let item_preds = ItemPredicates::compile(&params, &commit_preds);
        batches.extend_from_slice(&item_preds.defs.batches);
        Self {
            params,
            vd_set,
            batches,
        }
    }

    fn make_item_pod(
        &self,
        ingredients_def: IngredientsDef,
        input_item_key_pods: Vec<MainPod>,
    ) -> anyhow::Result<MainPod> {
        let prover = &Prover {};
        let mut builder = MainPodBuilder::new(&self.params, &self.vd_set);
        let mut item_builder =
            ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);

        let mut sts_item_key = Vec::new();
        for input_item_key_pod in input_item_key_pods {
            let st_item_key = input_item_key_pod.pod.pub_statements()[0].clone();
            sts_item_key.push(st_item_key);
            item_builder.ctx.builder.add_pod(input_item_key_pod);
        }

        let item_def = ItemDef {
            ingredients: ingredients_def,
            work: Value::from(42).raw(),
        };
        let (st_nullifiers, _nullifiers) = if sts_item_key.is_empty() {
            item_builder.st_nullifiers(sts_item_key).unwrap()
        } else {
            // The default params don't have enough custom statement verifications to fit
            // everything in a single pod, so we split it in two.
            let (st_nullifiers, nullifiers) = item_builder.st_nullifiers(sts_item_key).unwrap();
            item_builder.ctx.builder.reveal(&st_nullifiers);

            println!("Proving nullifiers_pod...");
            let nullifiers_pod = builder.prove(prover).unwrap();
            nullifiers_pod.pod.verify().unwrap();
            builder = MainPodBuilder::new(&self.params, &self.vd_set);
            item_builder =
                ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);
            item_builder.ctx.builder.add_pod(nullifiers_pod);
            (st_nullifiers, nullifiers)
        };

        // TODO: expose the statements required for latter committing and consuming.  For example
        // expose:
        // - IsCopper
        // - ItemKey
        let mut builder = MainPodBuilder::new(&self.params, &self.vd_set);
        let mut item_builder =
            ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);
        let st_item_def = item_builder.st_item_def(item_def).unwrap();
        let st_item_key = item_builder.st_item_key(st_item_def).unwrap();
        item_builder.ctx.builder.reveal(&st_item_key);

        println!("Proving item_key_pod for...");
        let item_key_pod = builder.prove(prover).unwrap();
        item_key_pod.pod.verify().unwrap();

        Ok(item_key_pod)
    }
}

fn load_item(input: &Path) -> anyhow::Result<CraftedItem> {
    let mut file = std::fs::File::open(&input)?;
    let crafted_item: CraftedItem = serde_json::from_reader(&mut file)?;
    crafted_item.pod.pod.verify()?;
    Ok(crafted_item)
}

fn craft_item(
    params: &Params,
    key: Value,
    recipe: &str,
    output: &Path,
    inputs: &[PathBuf],
) -> anyhow::Result<()> {
    println!("inputs: {:?}", inputs);
    let key = key.raw();
    println!("About to mine \"{recipe}\"");
    let (item_def, input_items) = match recipe {
        "copper" => {
            if inputs.len() != 0 {
                bail!("{recipe} takes 0 inputs");
            }
            let mining_recipe = MiningRecipe::new(COPPER_BLUEPRINT.to_string(), &[]);
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, COPPER_MINING_MAX)?
                .unwrap();
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: COPPER_WORK,
                },
                vec![],
            )
        }
        "tin" => {
            if inputs.len() != 0 {
                bail!("{recipe} takes 0 inputs");
            }
            let mining_recipe = MiningRecipe::new(TIN_BLUEPRINT.to_string(), &[]);
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, TIN_MINING_MAX)?
                .unwrap();
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: TIN_WORK,
                },
                vec![],
            )
        }
        "bronze" => {
            if inputs.len() != 2 {
                bail!("{recipe} takes 2 inputs");
            }
            let tin = load_item(&inputs[0])?;
            let copper = load_item(&inputs[1])?;
            let mining_recipe = MiningRecipe::new(
                BRONZE_BLUEPRINT.to_string(),
                &[tin.def.item_hash(params)?, copper.def.item_hash(params)?],
            );
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, BRONZE_MINING_MAX)?
                .unwrap();
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: BRONZE_WORK,
                },
                vec![tin, copper],
            )
        }
        unknown => bail!("Unknown recipe for \"{unknown}\""),
    };

    let helper = Helper::new(params.clone(), *DEFAULT_VD_SET);
    let input_item_pods: Vec<_> = input_items.iter().map(|item| &item.pod).cloned().collect();
    let item_pod = helper.make_item_pod(item_def.ingredients, input_item_pods)?;

    let commit_preds = CommitPredicates::compile(params);
    let mut batches = commit_preds.defs.batches.clone();
    let item_preds = ItemPredicates::compile(params, &commit_preds);
    batches.extend_from_slice(&item_preds.defs.batches);

    // TODO
    let vd_set = &*DEFAULT_VD_SET;

    let mut builder = MainPodBuilder::new(&Default::default(), vd_set);

    let mut item_builder = ItemBuilder::new(BuildContext::new(&mut builder, &batches), params);
    let st_item_def = item_builder.st_item_def(item_def.clone())?;

    let mut craft_builder = CraftBuilder::new(BuildContext::new(&mut builder, &batches), params);
    let st_craft = match recipe {
        "copper" => craft_builder.st_is_copper(item_def.clone(), st_item_def)?,
        unknown => unreachable!("recipe {unknown}"),
    };
    builder.reveal(&st_craft);
    let prover = &Prover {};
    let pod = builder.prove(prover)?;
    pod.pod.verify().unwrap();

    // TODO: In CraftedItem we should store everything required to
    // - Commit the item
    // - Craft a new item that consumes is so that the crafted item can later be committed
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

    let mut item_builder = ItemBuilder::new(BuildContext::new(&mut builder, batches), params);
    let st_item_def = item_builder.st_item_def(crafted_item.def.clone())?;
    let (st_nullifier, _) = item_builder.st_nullifiers(vec![])?;
    let st_commit_creation = item_builder.st_commit_creation(
        crafted_item.def.clone(),
        st_nullifier,
        created_items.clone(),
        st_item_def,
    )?;
    builder.reveal(&st_commit_creation);
    let prover = &Prover {};
    let pod = builder.prove(prover)?;
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
