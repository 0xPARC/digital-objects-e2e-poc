use std::{
    fs::{File, create_dir_all, rename},
    io::{Read, Write},
    path::Path,
};

use anyhow::Result;
use pod2::frontend::MainPod;

// TODO: Make async
pub fn store_pod(path: &Path, name: &str, pod: &MainPod) -> Result<()> {
    create_dir_all(path)?;
    let file_path = path.join(format!("{name}.pod2.json"));
    let file_path_tmp = path.join(format!("{name}.pod2.json.tmp"));
    let mut file_tmp = File::create(&file_path_tmp)?;
    let pod_json = serde_json::to_string(pod)?;
    file_tmp.write_all(pod_json.as_bytes())?;
    rename(file_path_tmp, file_path)?;
    Ok(())
}

// TODO: Make async
pub fn load_pod(path: &Path, name: &str) -> Result<MainPod> {
    let file_path = path.join(format!("{name}.pod2.json"));
    let mut file = File::open(&file_path)?;
    let mut pod_json = Vec::new();
    file.read_to_end(&mut pod_json)?;
    let pod: MainPod = serde_json::from_slice(&pod_json)?;
    Ok(pod)
}
