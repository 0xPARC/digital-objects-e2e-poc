use std::{
    array, fmt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use alloy::primitives::Address;
use anyhow::{Context as _, Result, anyhow};
use commitlib::{ItemBuilder, ItemDef, predicates::CommitPredicates};
use pod2::{
    backends::plonky2::{mainpod::Prover, primitives::merkletree::MerkleProof},
    frontend::{MainPod, MainPodBuilder},
    middleware::{
        CustomPredicateBatch, DEFAULT_VD_SET, F, Params, Pod, RawValue, VDSet, containers::Set,
    },
};
use serde::{Deserialize, Serialize};
use tracing_subscriber::{EnvFilter, prelude::*};

pub mod eth;

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

pub fn log_init() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
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
    Copper,
    Tin,
    Bronze,
}

impl FromStr for Recipe {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "copper" => Ok(Self::Copper),
            "tin" => Ok(Self::Tin),
            "bronze" => Ok(Self::Bronze),
            _ => Err(anyhow!("unknown recipe {s}")),
        }
    }
}

impl fmt::Display for Recipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Self::Copper => write!(f, "copper"),
            Self::Tin => write!(f, "tin"),
            Self::Bronze => write!(f, "bronze"),
        }
    }
}
