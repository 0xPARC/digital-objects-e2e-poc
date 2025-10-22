use std::str::FromStr;

use alloy::primitives::Address;
use anyhow::{Context as _, Result};
use common::ProofType;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

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
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}
