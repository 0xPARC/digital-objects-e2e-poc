#![allow(clippy::uninlined_format_args)]
use std::{str::FromStr, sync::Arc};

use alloy::primitives::Address;
use anyhow::{Context as _, Result};
use common::{
    ProofType,
    shrink::{ShrunkMainPodBuild, ShrunkMainPodSetup},
};
use pod2::{
    backends::plonky2::basetypes::DEFAULT_VD_SET,
    middleware::{Params, VDSet},
};
use tracing::{info, warn};

pub mod endpoints;
pub mod eth;

#[derive(Debug, Clone)]
pub struct Config {
    // The URL for the Ethereum RPC API
    pub rpc_url: String,
    // The path to the sqlite database (it will be a file)
    pub sqlite_path: String,
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
    fn from_env() -> Result<Self> {
        fn var(v: &str) -> Result<String> {
            dotenvy::var(v).with_context(|| v.to_string())
        }
        Ok(Self {
            rpc_url: var("RPC_URL")?,
            sqlite_path: var("AD_SERVER_SQLITE_PATH")?,
            pods_path: var("PODS_PATH")?,
            priv_key: var("PRIV_KEY")?,
            to_addr: Address::from_str(&var("TO_ADDR")?)?,
            tx_watch_timeout: u64::from_str(&var("TX_WATCH_TIMEOUT")?)?,
            proof_type: ProofType::from_str(&var("PROOF_TYPE")?)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PodConfig {
    #[allow(dead_code)] // TODO: Remove after these are put into use.
    params: Params,
    #[allow(dead_code)] // TODO: Remove after these are put into use.
    vd_set: VDSet,
}

pub struct Context {
    pub cfg: Config,
    pub pod_config: PodConfig,
    pub shrunk_main_pod_build: ShrunkMainPodBuild,
}

impl Context {
    pub fn new(
        cfg: Config,
        pod_config: PodConfig,
        shrunk_main_pod_build: ShrunkMainPodBuild,
    ) -> Self {
        Self {
            cfg,
            pod_config,
            shrunk_main_pod_build,
        }
    }
}

use tracing_subscriber::{EnvFilter, fmt, prelude::*};
fn log_init() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    // If a thread panics we have a bug, so we exit the entire process instead of staying in a
    // crashed state.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic(info);
        std::process::exit(1);
    }));

    log_init();
    common::load_dotenv()?;
    let cfg = Config::from_env()?;
    info!(?cfg, "Loaded config");

    // initialize pod data
    let params = Params::default();
    info!("Prebuilding circuits to calculate vd_set...");
    let vd_set = &*DEFAULT_VD_SET;
    info!("vd_set calculation complete");
    let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build()?;
    let pod_config = PodConfig {
        params,
        vd_set: vd_set.clone(),
    };

    if cfg.proof_type == ProofType::Groth16 {
        // initialize groth16 memory
        warn!(
            "WARNING: loading Groth16 artifacts, please wait till the pk & vk are loaded (>30s) and the server is running"
        );
        common::groth::init()?;
    }

    let ctx = Arc::new(Context::new(cfg, pod_config, shrunk_main_pod_build));

    let routes = endpoints::routes(ctx.clone());

    info!("server at http://0.0.0.0:8000");
    warp::serve(routes).run(([0, 0, 0, 0], 8000)).await;

    Ok(())
}
