use alloy::{
    consensus::{SidecarBuilder, SimpleCoder},
    eips::eip4844::DATA_GAS_PER_BLOB,
    network::{TransactionBuilder, TransactionBuilder4844},
    primitives::{Address, TxHash},
    providers::{Provider, ProviderBuilder},
    rpc::types::{TransactionReceipt, TransactionRequest},
    signers::local::PrivateKeySigner,
};
use anyhow::{Result, anyhow};
use tokio::time::{Duration, sleep};
use tracing::{debug, info};

use crate::Config;

/// send the given byte-array into an EIP-4844 transaction (in a blob)
pub async fn send_payload(cfg: &Config, b: Vec<u8>) -> Result<TxHash> {
    if cfg.priv_key.is_empty() {
        // test mode, return a mock tx_hash
        return Ok(TxHash::from([0u8; 32]));
    }
    // send the pod2 proof into a tx blob
    let signer: PrivateKeySigner = cfg.priv_key.parse()?;
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect(&cfg.rpc_url)
        .await?;
    let latest_block = provider.get_block_number().await?;
    info!("Latest block number: {latest_block}");

    let sender = signer.address();
    let receiver = Address::from([0x42; 20]);
    debug!("{}", sender);
    debug!("{}", receiver);

    let sidecar: SidecarBuilder<SimpleCoder> = SidecarBuilder::from_slice(&b);
    let sidecar = sidecar.build()?;

    let (receipt, tx_hash) = send_tx(cfg, provider, sender, receiver, sidecar).await?;

    info!(
        "Transaction included in block {}",
        receipt.block_number.expect("Failed to get block number")
    );

    if receipt.from != sender {
        return Err(anyhow!(
            "receipt.from: {} != sender: {}",
            receipt.from,
            sender
        ));
    }
    let receipt_to = receipt.to.ok_or(anyhow!("expected receipt.to"))?;
    if receipt_to != receiver {
        return Err(anyhow!(
            "receipt.to: {} != receiver: {}",
            receipt_to,
            receiver
        ));
    }
    let blob_gas_used = receipt
        .blob_gas_used
        .ok_or(anyhow!("expected EIP-4844 tx"))?;
    if blob_gas_used != DATA_GAS_PER_BLOB {
        return Err(anyhow!(
            "blob_gas_used: {} != DATA_GAS_PER_BLOB: {}",
            blob_gas_used,
            DATA_GAS_PER_BLOB
        ));
    }

    Ok(tx_hash)
}

#[allow(clippy::too_many_arguments)]
async fn send_tx(
    cfg: &Config,
    provider: impl alloy::providers::Provider + 'static,
    sender: Address,
    receiver: Address,
    sidecar: alloy::eips::eip4844::BlobTransactionSidecar,
) -> Result<(TransactionReceipt, TxHash)> {
    let fees = provider.estimate_eip1559_fees().await?;
    let blob_base_fee = provider.get_blob_base_fee().await?;
    // for a new tx, increase gas price by 10% to reduce the chances of the
    // nodes rejecting it (in practice increase it by 11% to ensure it passes
    // the miner filter)
    let mut fee_percentage: u128 = 111;
    let nonce = provider.get_transaction_count(sender).latest().await?;
    let mut tx_hash_prev = None;
    let tx_hash = loop {
        let tx = TransactionRequest::default()
            .with_max_fee_per_gas(fees.max_fee_per_gas * fee_percentage / 100)
            .with_max_priority_fee_per_gas(fees.max_priority_fee_per_gas * fee_percentage / 100)
            .with_max_fee_per_blob_gas(blob_base_fee * fee_percentage / 100)
            .with_to(receiver)
            .with_nonce(nonce)
            .with_blob_sidecar(sidecar.clone());

        debug!(
            max_fee_per_gas = tx.max_fee_per_gas.unwrap(),
            max_priority_fee_per_gas = tx.max_priority_fee_per_gas.unwrap(),
            max_fee_per_blob_gas = tx.max_fee_per_blob_gas.unwrap()
        );

        let send_tx_result = provider.send_transaction(tx).await;
        let pending_tx_result = match send_tx_result {
            Ok(pending_tx_result) => pending_tx_result,
            Err(e) => {
                if e.to_string().contains("Too Many Requests") {
                    // NOTE: this assumes we're using infura for the rpc_url
                    return Err(anyhow!("rpc-error: {}", e));
                }
                if e.to_string().contains("nonce too low") {
                    break tx_hash_prev.expect("resend tx with more gas");
                }

                info!("send tx err: {}", e);
                info!("sending tx again with 2x gas price in 10s");
                sleep(Duration::from_secs(10)).await;

                fee_percentage *= 2;
                continue;
            }
        };

        let tx_hash = *pending_tx_result.tx_hash();
        info!(
            "watching pending tx {}, timeout of {}",
            tx_hash, cfg.tx_watch_timeout
        );
        tx_hash_prev = Some(tx_hash);
        let pending_tx_result = pending_tx_result
            .with_timeout(Some(std::time::Duration::from_secs(cfg.tx_watch_timeout)))
            .watch()
            .await;

        let tx_hash = match pending_tx_result {
            Ok(pending_tx) => pending_tx,
            Err(e) => {
                if e.to_string().contains("Too Many Requests") {
                    panic!("error: {}", e);
                }

                info!("wait tx err: {}", e);
                info!("sending tx again with 2x gas price in 2s");
                sleep(Duration::from_secs(2)).await;

                fee_percentage *= 2;
                continue;
            }
        };
        info!("Pending transaction... tx hash: {}", tx_hash);
        break tx_hash;
    };
    let receipt = provider.get_transaction_receipt(tx_hash).await?;
    Ok((receipt.expect("tx exists"), tx_hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    // this test is mostly to check the send_payload method isolated from the
    // rest of the AD server logic.
    // To run it:
    // RUST_LOG=ad_server=debug cargo test --release -p ad-server test_tx -- --nocapture --ignored
    #[ignore]
    #[tokio::test]
    async fn test_tx() -> anyhow::Result<()> {
        crate::log_init();
        common::load_dotenv()?;
        let cfg = Config::from_env()?;
        println!("Loaded config: {:?}", cfg);

        let tx_hash = send_payload(&cfg, b"test".to_vec()).await?;
        dbg!(tx_hash);

        Ok(())
    }
}
