use std::{
    array, fmt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use alloy::primitives::Address;
use anyhow::{Context as _, Result, anyhow, bail};
use commitlib::{ItemBuilder, ItemDef, predicates::CommitPredicates};
use common::{
    payload::{Payload, PayloadProof},
    set_from_value,
    shrink::{ShrunkMainPodSetup, shrink_compress_pod},
};
use craftlib::{
    constants::{
        AXE_BLUEPRINT, AXE_MINING_MAX, AXE_WORK, STONE_BLUEPRINT, STONE_MINING_MAX, WOOD_BLUEPRINT,
        WOOD_MINING_MAX, WOOD_WORK, WOODEN_AXE_BLUEPRINT, WOODEN_AXE_MINING_MAX, WOODEN_AXE_WORK,
    },
    item::{CraftBuilder, MiningRecipe},
    powpod::PowPod,
    predicates::ItemPredicates,
};
use plonky2::field::types::Field;
use pod2::{
    backends::plonky2::mainpod::Prover,
    frontend::{MainPod, MainPodBuilder},
    middleware::{
        CustomPredicateBatch, DEFAULT_VD_SET, F, Params, Pod, RawValue, VDSet, containers::Set,
    },
};
use pod2utils::macros::BuildContext;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::eth::send_payload;

pub mod eth;

pub const USED_ITEM_SUBDIR_NAME: &str = "used";

#[derive(Clone, Debug)]
pub struct Config {
    // The URL for the Beacon API
    pub beacon_url: String,
    // The URL for the Ethereum RPC API
    pub rpc_url: String,
    // Ethereum private key to send txs
    pub priv_key: String,
    // The URL for the Synchronizer API
    pub sync_url: String,
    // The path to the pod storage directory
    pub pods_path: String,
    // The address that receives DO update via blobs
    pub to_addr: Address,
    pub tx_watch_timeout: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        fn var(v: &str) -> Result<String> {
            dotenvy::var(v).with_context(|| v.to_string())
        }
        Ok(Self {
            beacon_url: var("BEACON_URL")?,
            rpc_url: var("RPC_URL")?,
            priv_key: var("PRIV_KEY")?,
            sync_url: var("SYNC_URL")?,
            pods_path: var("PODS_PATH")?,
            to_addr: Address::from_str(&var("TO_ADDR")?)?,
            tx_watch_timeout: u64::from_str(&var("TX_WATCH_TIMEOUT")?)?,
        })
    }
}

pub fn load_item(input: &Path) -> anyhow::Result<CraftedItem> {
    let mut file = std::fs::File::open(input)?;
    let crafted_item: CraftedItem = serde_json::from_reader(&mut file)?;
    crafted_item.pod.pod.verify()?;
    Ok(crafted_item)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CraftedItem {
    pub pod: MainPod,
    pub def: ItemDef,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Recipe {
    Stone,
    Wood,
    Axe,
    WoodenAxe,
}
impl Recipe {
    pub fn list() -> Vec<Recipe> {
        vec![Recipe::Stone, Recipe::Wood, Recipe::Axe, Recipe::WoodenAxe]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProductionType {
    Mine,
    Craft,
}
impl fmt::Display for ProductionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            ProductionType::Mine => "Mine",
            ProductionType::Craft => "Craft",
        };
        write!(f, "{text}")
    }
}

impl Recipe {
    pub fn production_type(&self) -> ProductionType {
        match self {
            Self::Stone => ProductionType::Mine,
            Self::Wood => ProductionType::Mine,
            Self::Axe => ProductionType::Craft,
            Self::WoodenAxe => ProductionType::Craft,
        }
    }
}
impl FromStr for Recipe {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stone" => Ok(Self::Stone),
            "wood" => Ok(Self::Wood),
            "axe" => Ok(Self::Axe),
            "wooden-axe" => Ok(Self::WoodenAxe),
            _ => Err(anyhow!("unknown recipe {s}")),
        }
    }
}

impl fmt::Display for Recipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::Stone => write!(f, "stone"),
            Self::Wood => write!(f, "wood"),
            Self::Axe => write!(f, "axe"),
            Self::WoodenAxe => write!(f, "wooden-axe"),
        }
    }
}

fn rand_raw_value() -> RawValue {
    let mut rng = StdRng::from_os_rng();
    RawValue(array::from_fn(|_| F::from_noncanonical_u64(rng.next_u64())))
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
        recipe: Recipe,
        item_def: ItemDef,
        input_item_pods: Vec<MainPod>,
        pow_pod: Option<PowPod>,
    ) -> anyhow::Result<MainPod> {
        let prover = &Prover {};
        let mut builder = MainPodBuilder::new(&self.params, &self.vd_set);
        let mut item_builder =
            ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);

        let mut sts_input_item_key = Vec::new();
        let mut sts_input_craft = Vec::new();
        for input_item_pod in input_item_pods {
            let st_item_key = input_item_pod.pod.pub_statements()[0].clone();
            sts_input_item_key.push(st_item_key);
            let st_craft = input_item_pod.pod.pub_statements()[3].clone();
            sts_input_craft.push(st_craft);
            item_builder.ctx.builder.add_pod(input_item_pod);
        }

        let (st_nullifiers, _nullifiers) = if sts_input_item_key.is_empty() {
            item_builder.st_nullifiers(sts_input_item_key).unwrap()
        } else {
            // The default params don't have enough custom statement verifications to fit
            // everything in a single pod, so we split it in two.
            let (st_nullifiers, nullifiers) =
                item_builder.st_nullifiers(sts_input_item_key).unwrap();
            item_builder.ctx.builder.reveal(&st_nullifiers);
            // Propagate sts_input_craft for use in st_craft
            for st_input_craft in &sts_input_craft {
                item_builder.ctx.builder.reveal(st_input_craft);
            }

            info!("Proving nullifiers_pod...");
            let nullifiers_pod = builder.prove(prover).unwrap();
            nullifiers_pod.pod.verify().unwrap();
            builder = MainPodBuilder::new(&self.params, &self.vd_set);
            item_builder =
                ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);
            item_builder.ctx.builder.add_pod(nullifiers_pod);
            (st_nullifiers, nullifiers)
        };

        let mut item_builder =
            ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);
        let st_item_def = item_builder.st_item_def(item_def.clone()).unwrap();
        let st_item_key = item_builder.st_item_key(st_item_def.clone()).unwrap();

        let mut craft_builder =
            CraftBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);
        let st_craft = match recipe {
            Recipe::Stone => {
                // unwrap safe since if we're at Stone, pow_pod is Some
                let pow_pod = pow_pod.unwrap();
                let st_pow = pow_pod.pub_statements()[0].clone();
                let main_pow_pod = MainPod {
                    pod: Box::new(pow_pod.clone()),
                    public_statements: pow_pod.pub_statements(),
                    params: craft_builder.params.clone(),
                };
                craft_builder.ctx.builder.add_pod(main_pow_pod);
                craft_builder.st_is_stone(item_def, st_item_def.clone(), st_pow)?
            }
            Recipe::Wood => craft_builder.st_is_wood(item_def, st_item_def.clone())?,
            Recipe::Axe => craft_builder.st_is_axe(
                item_def,
                st_item_def.clone(),
                sts_input_craft[0].clone(),
                sts_input_craft[1].clone(),
            )?,
            Recipe::WoodenAxe => craft_builder.st_is_wooden_axe(
                item_def,
                st_item_def.clone(),
                sts_input_craft[0].clone(),
                sts_input_craft[1].clone(),
            )?,
        };

        builder.reveal(&st_item_key); // 0: Required for consuming via Nullifiers
        builder.reveal(&st_item_def); // 1: Required for committing via CommitCreation
        builder.reveal(&st_nullifiers); // 2: Required for committing via CommitCreation
        builder.reveal(&st_craft); // 3: App layer predicate

        info!("Proving item_pod");
        let start = std::time::Instant::now();
        let item_key_pod = builder.prove(prover).unwrap();
        log::info!("[TIME] pod proving time: {:?}", start.elapsed());
        item_key_pod.pod.verify().unwrap();

        Ok(item_key_pod)
    }

    fn make_commitment_pod(
        &self,
        crafted_item: CraftedItem,
        created_items: Set,
    ) -> anyhow::Result<MainPod> {
        let mut builder = MainPodBuilder::new(&self.params, &self.vd_set);
        builder.add_pod(crafted_item.pod.clone());

        let mut item_builder =
            ItemBuilder::new(BuildContext::new(&mut builder, &self.batches), &self.params);
        let st_item_def = crafted_item.pod.public_statements[1].clone();
        let st_nullifiers = crafted_item.pod.public_statements[2].clone();
        let st_commit_creation = item_builder.st_commit_creation(
            crafted_item.def.clone(),
            st_nullifiers,
            created_items.clone(),
            st_item_def,
        )?;
        builder.reveal(&st_commit_creation);
        let prover = &Prover {};
        info!("Proving commit_pod...");
        let pod = builder.prove(prover)?;
        pod.pod.verify().unwrap();

        Ok(pod)
    }
}

pub fn craft_item(
    params: &Params,
    recipe: Recipe,
    output: &Path,
    inputs: &[PathBuf],
) -> anyhow::Result<()> {
    let vd_set = DEFAULT_VD_SET.clone();
    let key = rand_raw_value();
    info!("About to craft \"{recipe}\" with key {key:#}");
    let (item_def, input_items, pow_pod) = match recipe {
        Recipe::Stone => {
            if !inputs.is_empty() {
                bail!("{recipe} takes 0 inputs");
            }
            let mining_recipe = MiningRecipe::new(STONE_BLUEPRINT.to_string(), &[]);
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, STONE_MINING_MAX)?
                .unwrap();

            let start = std::time::Instant::now();
            let pow_pod = PowPod::new(
                params,
                vd_set.clone(),
                3, // num_iters
                RawValue::from(ingredients_def.dict(params)?.commitment()),
            )?;
            log::info!("[TIME] PowPod proving time: {:?}", start.elapsed());
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: pow_pod.output,
                },
                vec![],
                Some(pow_pod),
            )
        }
        Recipe::Wood => {
            if !inputs.is_empty() {
                bail!("{recipe} takes 0 inputs");
            }
            let mining_recipe = MiningRecipe::new(WOOD_BLUEPRINT.to_string(), &[]);
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, WOOD_MINING_MAX)?
                .unwrap();
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: WOOD_WORK,
                },
                vec![],
                None,
            )
        }
        Recipe::Axe => {
            if inputs.len() != 2 {
                bail!("{recipe} takes 2 inputs");
            }
            let wood = load_item(&inputs[0])?;
            let stone = load_item(&inputs[1])?;
            let mining_recipe = MiningRecipe::new(
                AXE_BLUEPRINT.to_string(),
                &[wood.def.item_hash(params)?, stone.def.item_hash(params)?],
            );
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, AXE_MINING_MAX)?
                .unwrap();
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: AXE_WORK,
                },
                vec![wood, stone],
                None,
            )
        }
        Recipe::WoodenAxe => {
            if inputs.len() != 2 {
                bail!("{recipe} takes 2 inputs");
            }
            let wood1 = load_item(&inputs[0])?;
            let wood2 = load_item(&inputs[1])?;
            let mining_recipe = MiningRecipe::new(
                WOODEN_AXE_BLUEPRINT.to_string(),
                &[wood1.def.item_hash(params)?, wood2.def.item_hash(params)?],
            );
            let ingredients_def = mining_recipe
                .do_mining(params, key, 0, WOODEN_AXE_MINING_MAX)?
                .unwrap();
            (
                ItemDef {
                    ingredients: ingredients_def.clone(),
                    work: WOODEN_AXE_WORK,
                },
                vec![wood1, wood2],
                None,
            )
        }
    };

    let helper = Helper::new(params.clone(), vd_set);
    let input_item_pods: Vec<_> = input_items.iter().map(|item| &item.pod).cloned().collect();
    let pod = helper.make_item_pod(recipe, item_def.clone(), input_item_pods, pow_pod)?;

    let crafted_item = CraftedItem { pod, def: item_def };
    let mut file = std::fs::File::create(output)?;
    serde_json::to_writer(&mut file, &crafted_item)?;
    info!("Stored crafted item mined with recipe {recipe} to {output:?}");

    Ok(())
}

pub async fn commit_item(params: &Params, cfg: &Config, input: &Path) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(input)?;
    let crafted_item: CraftedItem = serde_json::from_reader(&mut file)?;

    let created_items: Set =
        reqwest::blocking::get(format!("{}/created_items", cfg.sync_url))?.json()?;

    let helper = Helper::new(params.clone(), DEFAULT_VD_SET.clone());

    let pod = helper.make_commitment_pod(crafted_item.clone(), created_items.clone())?;

    let shrunk_main_pod_build = ShrunkMainPodSetup::new(params)
        .build()
        .expect("successful build");
    let shrunk_main_pod_proof = shrink_compress_pod(&shrunk_main_pod_build, pod.clone()).unwrap();

    let st_commit_creation = pod.public_statements[0].clone();
    let nullifier_set = set_from_value(&st_commit_creation.args()[1].literal()?)?;
    let nullifiers: Vec<RawValue> = nullifier_set.set().iter().map(|v| v.raw()).collect();
    let payload_bytes = Payload {
        proof: PayloadProof::Plonky2(Box::new(shrunk_main_pod_proof.clone())),
        item: RawValue::from(crafted_item.def.item_hash(params)?),
        created_items_root: RawValue::from(created_items.commitment()),
        nullifiers,
    }
    .to_bytes();

    let tx_hash = send_payload(cfg, payload_bytes).await?;

    info!("Committed item in tx={tx_hash}");

    Ok(())
}

pub async fn destroy_item(_params: &Params, _cfg: &Config, item: &PathBuf) -> anyhow::Result<()> {
    // TODO: Nullify
    let (file_name, parent_dir) = item
        .file_name()
        .and_then(|name| Some((name.display(), item.parent()?.display())))
        .ok_or(anyhow!("Item at {} is not a file.", item.display()))?;
    let used_item = PathBuf::from(format!("{parent_dir}/{USED_ITEM_SUBDIR_NAME}/{file_name}"));
    std::fs::rename(item, used_item)?;
    info!("Destroyed item at {}", item.display());

    Ok(())
}
