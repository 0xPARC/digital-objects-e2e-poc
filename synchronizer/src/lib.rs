#![allow(clippy::uninlined_format_args)]

pub mod clients;

use alloy::{
    eips::eip4844::FIELD_ELEMENT_BYTES_USIZE,
    rpc::types::beacon::sidecar::{BeaconBlobBundle, BlobData},
    transports::http::reqwest,
};
use anyhow::{Result, anyhow};

#[allow(dead_code)]
pub(crate) async fn get_blobs(beacon_url: &str, block_id: u64) -> Result<Vec<BlobData>> {
    let req_url = format!("{}/eth/v1/beacon/blob_sidecars/{}", beacon_url, block_id);
    let resp = reqwest::get(req_url).await?.text().await?;
    let blob_bundle: BeaconBlobBundle = serde_json::from_str(&resp)?;
    Ok(blob_bundle.data)
}

/// Extracts bytes from a blob in the 'simple' encoding.
pub fn bytes_from_simple_blob(blob_bytes: &[u8]) -> Result<Vec<u8>> {
    // Blob = [0x00] ++ 8_BYTE_LEN ++ [0x00,...,0x00] ++ X.
    let data_len = u64::from_be_bytes(std::array::from_fn(|i| blob_bytes[1 + i])) as usize;

    // Sanity check: Blob must be able to accommodate the specified data length.
    let max_data_len =
        (blob_bytes.len() / FIELD_ELEMENT_BYTES_USIZE - 1) * (FIELD_ELEMENT_BYTES_USIZE - 1);
    if data_len > max_data_len {
        return Err(anyhow!(
            "Given blob of length {} cannot accommodate {} bytes.",
            blob_bytes.len(),
            data_len
        ));
    }

    Ok(blob_bytes
        .chunks(FIELD_ELEMENT_BYTES_USIZE)
        .skip(1)
        .flat_map(|chunk| chunk[1..].to_vec())
        .take(data_len)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ignore]
    #[tokio::test]
    async fn test_get_blobs() -> Result<()> {
        let beacon_url = "https://ethereum-beacon-api.publicnode.com";
        let block_id = 11111111;
        let _blobs = get_blobs(beacon_url, block_id).await?;
        // println!("{:?}", _blobs); // commented out since it prints more than 10k lines
        Ok(())
    }
}
