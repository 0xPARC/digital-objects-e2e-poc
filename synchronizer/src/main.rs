#![allow(clippy::uninlined_format_args)]
use std::{
    collections::{HashMap, HashSet},
    fs::{File, create_dir_all, read_dir, rename},
    io,
    io::{Read, Write},
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use alloy::{
    consensus::Transaction,
    eips as alloy_eips,
    eips::eip4844::kzg_to_versioned_hash,
    network as alloy_network,
    primitives::{Address, B256},
    providers as alloy_provider,
};
use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use anyhow::{Context, Result, anyhow, bail};
use backoff::ExponentialBackoffBuilder;
use chrono::{DateTime, Utc};
use commitlib::predicates::CommitPredicates;
use common::{
    ProofType, load_dotenv,
    payload::{Payload, PayloadProof},
    shrink::ShrunkMainPodSetup,
};
use plonky2::plonk::proof::CompressedProofWithPublicInputs;
use pod2::{
    backends::plonky2::{
        basetypes::DEFAULT_VD_SET,
        mainpod::calculate_statements_hash,
        serialization::{CommonCircuitDataSerializer, VerifierCircuitDataSerializer},
    },
    cache,
    cache::CacheEntry,
    middleware::{
        CommonCircuitData, CustomPredicateRef, EMPTY_VALUE, Hash, Params, RawValue, Statement,
        Value, VerifierCircuitData, containers::Set,
    },
};
use synchronizer::{
    bytes_from_simple_blob,
    clients::beacon::{
        self, BeaconClient,
        types::{Blob, BlockHeader, BlockId},
    },
};
use tokio::{runtime::Runtime, time::sleep};
use tracing::{debug, info, trace};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub mod endpoints;

pub fn cache_get_shrunk_main_pod_circuit_data(
    params: &Params,
) -> CacheEntry<(CommonCircuitDataSerializer, VerifierCircuitDataSerializer)> {
    cache::get("shrunk_main_pod_circuit_data", &params, |params| {
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(params)
            .build()
            .expect("successful build");
        let verifier = shrunk_main_pod_build.circuit_data.verifier_data();
        let common = shrunk_main_pod_build.circuit_data.common;
        (
            CommonCircuitDataSerializer(common),
            VerifierCircuitDataSerializer(verifier),
        )
    })
    .expect("cache ok")
}

#[derive(Clone, Debug)]
pub struct Config {
    // The URL for the Beacon API
    pub beacon_url: String,
    // The URL for the Ethereum RPC API
    pub rpc_url: String,
    // The path to the ad blob storage directory
    pub blobs_path: String,
    // The slot where the DO updates begins
    pub do_genesis_slot: u32,
    // The address that receives DO update via blobs
    pub to_addr: Address,
    // Max Beacon API + RPC requests per second
    pub request_rate: u64,
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
            beacon_url: var("BEACON_URL")?,
            rpc_url: var("RPC_URL")?,
            blobs_path: var("BLOBS_PATH")?,
            do_genesis_slot: u32::from_str(&var("DO_GENESIS_SLOT")?)?,
            to_addr: Address::from_str(&var("TO_ADDR")?)?,
            request_rate: u64::from_str(&var("REQUEST_RATE")?)?,
            proof_type: ProofType::from_str(&var("PROOF_TYPE")?)?,
        })
    }
}

#[derive(Debug)]
struct Node {
    cfg: Config,
    #[allow(dead_code)]
    params: Params,
    vds_root: Hash,
    beacon_cli: BeaconClient,
    rpc_cli: RootProvider,
    common_circuit_data: CommonCircuitData,
    verifier_circuit_data: VerifierCircuitData,
    pred_commit_creation: CustomPredicateRef,
    // Mutable state
    epoch: Mutex<u64>,
    created_items_roots: Mutex<Vec<RawValue>>,
    created_items: RwLock<Set>,
    nullifiers: RwLock<HashSet<RawValue>>,
}

impl Node {
    async fn new(cfg: Config) -> Result<Self> {
        let http_cli = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()?;

        let exp_backoff = Some(ExponentialBackoffBuilder::default().build());
        let beacon_cli_cfg = beacon::Config {
            base_url: cfg.beacon_url.clone(),
            exp_backoff,
        };
        let beacon_cli = BeaconClient::try_with_client(http_cli, beacon_cli_cfg)?;
        let rpc_cli = RootProvider::<Ethereum>::new_http(cfg.rpc_url.parse()?);

        let params = Params::default();
        let commit_predicates = CommitPredicates::compile(&params);
        let vds_root = DEFAULT_VD_SET.root();
        info!("Loading circuit data...");
        let (common_circuit_data, verifier_circuit_data) =
            &*cache_get_shrunk_main_pod_circuit_data(&params);

        let created_items = Set::new(params.max_depth_mt_containers, HashSet::new()).unwrap();
        let nullifiers = HashSet::new();
        Ok(Self {
            cfg,
            beacon_cli,
            rpc_cli,
            params,
            vds_root,
            common_circuit_data: (**common_circuit_data).clone(),
            verifier_circuit_data: (**verifier_circuit_data).clone(),
            pred_commit_creation: commit_predicates.commit_creation,
            epoch: Mutex::new(0),
            // initialize the `created_items_root` with 0x00... root, so that
            // when new items are crafted from scratch, their
            // `payload.created_items_root` (which is 0x00... since it is a
            // from-scratch item) is accepted as a "valid" one, since it appears
            // at the `created_items_root`.
            created_items_roots: Mutex::new(vec![EMPTY_VALUE]),
            created_items: RwLock::new(created_items),
            nullifiers: RwLock::new(nullifiers),
        })
    }

    fn slot_dir(&self, slot: u32) -> PathBuf {
        let slot_hi = slot / 1_000_000;
        let slot_mid = (slot - slot_hi * 1_000_000) / 1_000;
        let slot_lo = slot - slot_hi * 1_000_000 - slot_mid * 1_000;
        let slot_dir: PathBuf = [
            &self.cfg.blobs_path,
            &format!("{:03}", slot_hi),
            &format!("{:03}", slot_mid),
            &format!("{:03}", slot_lo),
        ]
        .iter()
        .collect();
        slot_dir
    }

    async fn load_blobs_disk(&self, slot: u32) -> Result<HashMap<B256, Blob>> {
        let slot_dir = self.slot_dir(slot);
        let rd = match read_dir(&slot_dir) {
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    return Ok(HashMap::new());
                } else {
                    return Err(e.into());
                }
            }
            Ok(rd) => rd,
        };
        debug!("loading blobs of slot {} from {:?}", slot, slot_dir);
        let mut blobs = HashMap::new();
        for entry in rd {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name = file_name.to_str().unwrap_or("");
            if file_name.starts_with("blob-") && file_name.ends_with(".cbor") {
                let file_path = slot_dir.join(file_name);
                let mut file = File::open(&file_path)?;
                let mut data_cbor = Vec::new();
                file.read_to_end(&mut data_cbor)?;
                let blob: Blob = minicbor_serde::from_slice(&data_cbor)?;
                let versioned_hash = kzg_to_versioned_hash(blob.kzg_commitment.as_ref());
                blobs.insert(versioned_hash, blob);
            }
        }
        Ok(blobs)
    }

    async fn store_blobs_disk(&self, slot: u32, blobs: &HashMap<B256, Blob>) -> Result<()> {
        let slot_dir = self.slot_dir(slot);
        debug!("storing blobs of slot {} to {:?}", slot, slot_dir);
        create_dir_all(&slot_dir)?;
        for (vh, blob) in blobs {
            let name = format!("blob-{}.cbor", vh);
            let blob_path = slot_dir.join(&name);
            let blob_path_tmp = slot_dir.join(format!("{}.tmp", name));
            let mut file_tmp = File::create(&blob_path_tmp)?;
            let blob_cbor = minicbor_serde::to_vec(blob)?;
            file_tmp.write_all(&blob_cbor)?;
            rename(blob_path_tmp, blob_path)?;
        }
        Ok(())
    }

    // Checks that the blobs contain all the blobs identified by `versioned_hashes`.  If some are
    // missing, return the versioned_hash of the first missing one.
    fn validate_blobs(blobs: &HashMap<B256, Blob>, versioned_hashes: &[B256]) -> Option<B256> {
        for vh in versioned_hashes.iter() {
            if !blobs.contains_key(vh) {
                return Some(*vh);
            }
        }
        None
    }

    async fn get_blobs(&self, slot: u32, versioned_hashes: &[B256]) -> Result<HashMap<B256, Blob>> {
        let blobs = self.load_blobs_disk(slot).await?;
        if Self::validate_blobs(&blobs, versioned_hashes).is_some() {
            let blobs = self.beacon_cli.get_blobs(slot.into()).await?;
            debug!("got {} DO blobs from beacon_cli", blobs.len());
            let blobs: HashMap<_, _> = blobs
                .into_iter()
                .filter_map(|blob| {
                    let versioned_hash = kzg_to_versioned_hash(blob.kzg_commitment.as_ref());
                    versioned_hashes
                        .contains(&versioned_hash)
                        .then_some((versioned_hash, blob))
                })
                .collect();
            if let Some(vh) = Self::validate_blobs(&blobs, versioned_hashes) {
                return Err(anyhow!("Blob {} not found in beacon_cli response", vh));
            }
            self.store_blobs_disk(slot, &blobs).await?;
            Ok(blobs)
        } else {
            Ok(blobs)
        }
    }

    async fn process_beacon_block_header(
        &self,
        beacon_block_header: &BlockHeader,
    ) -> Result<Option<()>> {
        let beacon_block_root = beacon_block_header.root;
        let slot = beacon_block_header.slot;

        let beacon_block = match self
            .beacon_cli
            .get_block(BlockId::Hash(beacon_block_root))
            .await?
        {
            Some(block) => block,
            None => {
                debug!("slot {} has empty block", slot);
                return Ok(None);
            }
        };
        let execution_payload = match beacon_block.execution_payload {
            Some(payload) => payload,
            None => {
                debug!("slot {} has no execution payload", slot);
                return Ok(None);
            }
        };
        debug!(
            "slot {} has execution block {} at height {}",
            slot, execution_payload.block_hash, execution_payload.block_number
        );

        info!(
            "processing slot {} from {}",
            slot,
            DateTime::<Utc>::from_timestamp_secs(execution_payload.timestamp as i64)
                .unwrap_or_default(),
        );

        let has_kzg_blob_commitments = match beacon_block.blob_kzg_commitments {
            Some(commitments) => !commitments.is_empty(),
            None => false,
        };
        if !has_kzg_blob_commitments {
            debug!("slot {} has no blobs", slot);
            return Ok(None);
        }

        let execution_block_hash = execution_payload.block_hash;

        let execution_block_id = alloy_eips::eip1898::BlockId::Hash(execution_block_hash.into());
        let execution_block = self
            .rpc_cli
            .get_block(execution_block_id)
            .full()
            .await?
            .with_context(|| format!("Execution block {execution_block_hash} not found"))?;

        let indexed_do_blob_txs: Vec<_> = match execution_block.transactions.as_transactions() {
            Some(txs) => txs
                .iter()
                .enumerate()
                .filter(|(_index, tx)| {
                    tx.inner.blob_versioned_hashes().is_some()
                        && tx.as_recovered().to() == Some(self.cfg.to_addr)
                })
                .collect(),
            None => {
                return Err(anyhow!(
                    "Consensus block {beacon_block_root} has blobs but the execution block doesn't have txs"
                ));
            }
        };

        if indexed_do_blob_txs.is_empty() {
            return Ok(None);
        }

        let txs_blobs_vhs: Vec<B256> = indexed_do_blob_txs
            .iter()
            .flat_map(|(_, tx)| {
                tx.as_recovered()
                    .blob_versioned_hashes()
                    .expect("tx has blobs")
            })
            .cloned()
            .collect();
        let blobs = self.get_blobs(slot, &txs_blobs_vhs).await?;

        for (_tx_index, tx) in indexed_do_blob_txs {
            let tx = tx.as_recovered();
            let hash = tx.hash();
            let from = tx.signer();
            let to = tx.to();
            let tx_blobs: Vec<_> = tx
                .blob_versioned_hashes()
                .expect("tx has blobs")
                .iter()
                .map(|blob_versioned_hash| &blobs[blob_versioned_hash])
                .collect();
            trace!(?hash, ?from, ?to);

            for blob in tx_blobs.iter() {
                match self.process_do_blob(blob).await {
                    Ok(_) => {
                        info!("Valid do_blob at slot {}, blob_index {}!", slot, blob.index);
                    }
                    Err(e) => {
                        info!("Invalid do_blob: {:?}", e);
                        continue;
                    }
                };
            }
        }
        Ok(Some(()))
    }

    async fn process_do_blob(&self, blob: &Blob) -> Result<()> {
        let bytes =
            bytes_from_simple_blob(blob.blob.inner()).context("Invalid byte encoding in blob")?;
        let payload = Payload::from_bytes(&bytes, &self.common_circuit_data)?;

        let mut epoch = self.epoch.lock().expect("lock");
        let mut created_items_roots = self.created_items_roots.lock().expect("lock");

        // Check the proof is using an official createdItems set
        if !created_items_roots.contains(&payload.created_items_root) {
            bail!(
                "created_items_root {} not in created_items_roots",
                payload.created_items_root
            );
        }

        // Check that output is unique
        if self
            .created_items
            .read()
            .expect("rlock")
            .contains(&Value::from(payload.item))
        {
            bail!("item {} exists in created_items", payload.item);
        }

        // Check that inputs are unique
        {
            // The nullifiers read lock is dropped at the end of this block
            let nullifiers = self.nullifiers.read().expect("rlock");
            for nullifier in &payload.nullifiers {
                if nullifiers.contains(nullifier) {
                    bail!("nullifier {} exists in nullifiers", nullifier);
                }
            }
        }

        let nullifiers_set = Value::from(
            Set::new(
                self.params.max_depth_mt_containers,
                HashSet::from_iter(payload.nullifiers.iter().map(|r| Value::from(*r))),
            )
            .unwrap(),
        );
        let st_commit_creation = Statement::Custom(
            self.pred_commit_creation.clone(),
            vec![
                Value::from(payload.item),
                nullifiers_set,
                Value::from(payload.created_items_root),
            ],
        );

        // Check the proof and ignore invalid ones
        self.verify_shrunk_main_pod(payload.proof, st_commit_creation)?;

        // Register nullifiers
        {
            let mut nullifiers = self.nullifiers.write().expect("wlock");
            for nullifier in &payload.nullifiers {
                nullifiers.insert(*nullifier);
            }
        }
        // Register item
        self.created_items
            .write()
            .expect("wlock")
            .insert(&Value::from(payload.item))
            .unwrap();

        *epoch += 1;
        created_items_roots.push(RawValue::from(
            self.created_items.read().expect("rlock").commitment(),
        ));
        Ok(())
    }

    fn verify_shrunk_main_pod(&self, proof: PayloadProof, st: Statement) -> Result<()> {
        let sts_hash = calculate_statements_hash(&[st.into()], &self.params);
        let public_inputs = [sts_hash.0, self.vds_root.0].concat();
        let shrunk_main_pod_proof = match proof {
            PayloadProof::Plonky2(proof) => proof,
            PayloadProof::Groth16(_) => todo!(),
        };
        let proof_with_pis = CompressedProofWithPublicInputs {
            proof: *shrunk_main_pod_proof,
            public_inputs,
        };
        let proof = proof_with_pis
            .decompress(
                &self.verifier_circuit_data.verifier_only.circuit_digest,
                &self.common_circuit_data,
            )
            .unwrap();
        self.verifier_circuit_data.verify(proof)
    }
}

fn log_init() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    log_init();
    load_dotenv()?;
    let cfg = Config::from_env()?;
    info!(?cfg, "Loaded config");

    if cfg.proof_type == ProofType::Groth16 {
        // initialize groth16 memory with the vk
        common::groth::load_vk()?;
    }

    let node = Arc::new(Node::new(cfg).await?);

    let spec = node.beacon_cli.get_spec().await?;
    info!(?spec, "Beacon spec");
    let head = node
        .beacon_cli
        .get_block_header(BlockId::Head)
        .await?
        .expect("head is not None");
    info!(?head, "Beacon head");

    {
        let node = node.clone();
        std::thread::spawn(move || -> Result<_, std::io::Error> {
            Runtime::new().map(|rt| {
                rt.block_on(async {
                    let routes = endpoints::routes(node);
                    warp::serve(routes).run(([0, 0, 0, 0], 8001)).await
                })
            })
        });
    }
    info!("Started HTTP server");

    let mut slot = node.cfg.do_genesis_slot;
    loop {
        debug!("checking slot {}", slot);
        let some_beacon_block_header = if slot <= head.slot {
            node.beacon_cli
                .get_block_header(BlockId::Slot(slot))
                .await?
        } else {
            // TODO: Be more fancy and replace this with a stream from an event subscription to
            // Beacon Headers
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                let head = node
                    .beacon_cli
                    .get_block_header(BlockId::Head)
                    .await?
                    .expect("head is not None");
                if head.slot > slot {
                    debug!(
                        "head is {}, slot {} was skipped, retrieving...",
                        head.slot, slot
                    );
                    break node
                        .beacon_cli
                        .get_block_header(BlockId::Slot(slot))
                        .await?;
                } else if head.slot == slot {
                    break Some(head);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        };
        let beacon_block_header = match some_beacon_block_header {
            Some(block) => block,
            None => {
                debug!("slot {} has empty block", slot);
                slot += 1;
                continue;
            }
        };

        node.process_beacon_block_header(&beacon_block_header)
            .await?;

        if node.cfg.request_rate != 0 {
            let requests = 5;
            let delay_ms = 1000 * requests / node.cfg.request_rate;
            sleep(Duration::from_millis(delay_ms)).await;
        }

        slot += 1;
    }
}
