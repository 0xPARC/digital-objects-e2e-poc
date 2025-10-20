use std::str::FromStr;

use alloy::primitives::Address;
use anyhow::{Context as _, Result};
use common::ProofType;

pub mod eth;

#[derive(Debug, Clone)]
pub struct Config {
    // The URL for the Ethereum RPC API
    pub rpc_url: String,
    // The path to store pods
    pub pods_path: String,
    // Ethereum private key to send txs
    pub priv_key: String,
    // The address that receives AD update via blobs
    pub to_addr: Address,
    pub tx_watch_timeout: u64,
    // set the proving system used to generate the proofs being sent to ethereum
    //   options: plonky2 / groth16
    pub proof_type: ProofType,
}

impl Config {
    #[allow(dead_code)]
    fn from_env() -> Result<Self> {
        fn var(v: &str) -> Result<String> {
            dotenvy::var(v).with_context(|| v.to_string())
        }
        Ok(Self {
            rpc_url: var("RPC_URL")?,
            pods_path: var("PODS_PATH")?,
            priv_key: var("PRIV_KEY")?,
            to_addr: Address::from_str(&var("TO_ADDR")?)?,
            tx_watch_timeout: u64::from_str(&var("TX_WATCH_TIMEOUT")?)?,
            proof_type: ProofType::from_str(&var("PROOF_TYPE")?)?,
        })
    }
}
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
#[allow(dead_code)]
fn log_init() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}
