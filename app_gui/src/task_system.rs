use std::{
    collections::HashMap,
    fmt::{self, Write},
    fs::{self},
    mem,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        mpsc::{self, channel},
    },
    thread::{self, JoinHandle},
    time,
};

use anyhow::{Result, anyhow};
use app_cli::{
    Config, CraftedItem, Recipe, USED_ITEM_SUBDIR_NAME, commit_item, craft_item, destroy_item,
    load_item, log_init,
};
use common::load_dotenv;
use egui::{Color32, Frame, Label, RichText, Ui};
use itertools::Itertools;
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{
        Hash, Params, RawValue, Statement, StatementArg, TypedValue, Value, containers::Set,
    },
};
use tokio::runtime::Runtime;
use tracing::{error, info};

#[derive(Default, Clone)]
pub struct TaskStatus {
    pub busy: Option<String>,
}

pub enum Request {
    Craft {
        params: Params,
        pods_path: String,
        recipe: Recipe,
        output: PathBuf,
        input_paths: Vec<PathBuf>,
    },
    Commit {
        params: Params,
        cfg: Config,
        input: PathBuf,
    },
    CraftAndCommit {
        params: Params,
        cfg: Config,
        pods_path: String,
        recipe: Recipe,
        output: PathBuf,
        input_paths: Vec<PathBuf>,
    },
    Destroy {
        params: Params,
        cfg: Config,
        item: PathBuf,
    },
    Exit,
}

pub enum Response {
    Craft(Result<PathBuf>),
    Commit(Result<PathBuf>),
    CraftAndCommit(Result<PathBuf>),
    Destroy(Result<PathBuf>),
    Null,
}

fn set_busy_task(task_status: &RwLock<TaskStatus>, task: &str) {
    let mut task_status = task_status.write().unwrap();
    task_status.busy = Some(task.to_string());
}
pub fn handle_req(task_status: &RwLock<TaskStatus>, req: Request) -> Response {
    match req {
        Request::Craft {
            params,
            pods_path,
            recipe,
            output,
            input_paths,
        } => craft(task_status, &params, pods_path, recipe, output, input_paths),
        Request::Commit { params, cfg, input } => commit(task_status, &params, cfg, input),
        Request::CraftAndCommit {
            params,
            cfg,
            pods_path,
            recipe,
            output,
            input_paths,
        } => {
            craft(
                task_status,
                &params,
                pods_path,
                recipe,
                output.clone(),
                input_paths,
            );
            commit(task_status, &params, cfg, output)
        }
        Request::Destroy { params, cfg, item } => {
            set_busy_task(task_status, "Destroying");

            Runtime::new().unwrap();
            let rt = Runtime::new().unwrap();
            let r = rt.block_on(async { destroy_item(&params, &cfg, &item).await });
            task_status.write().unwrap().busy = None;
            Response::Destroy(r.map(|_| item))
        }
        Request::Exit => Response::Null,
    }
}

fn craft(
    task_status: &RwLock<TaskStatus>,
    params: &Params,
    pods_path: String,
    recipe: Recipe,
    output: PathBuf,
    input_paths: Vec<PathBuf>,
) -> Response {
    set_busy_task(task_status, "Crafting");

    let r = craft_item(params, recipe, &output, &input_paths);

    // move the files of the used inputs into the `used` subdir
    let used_path = Path::new(&pods_path).join(USED_ITEM_SUBDIR_NAME);
    for input in input_paths {
        let parent_path = input.parent().unwrap();
        // if original file is not in 'used' subdir, move it there, ignore if it already is
        // in that subdir
        if parent_path != used_path {
            fs::rename(
                input.clone(),
                format!(
                    "{}/{}/{}",
                    parent_path.display(),
                    USED_ITEM_SUBDIR_NAME,
                    input.file_name().unwrap().display()
                ),
            )
            .unwrap();
        }
    }

    task_status.write().unwrap().busy = None;
    Response::Craft(r.map(|_| output))
}
fn commit(
    task_status: &RwLock<TaskStatus>,
    params: &Params,
    cfg: Config,
    input: PathBuf,
) -> Response {
    set_busy_task(task_status, "Committing");

    Runtime::new().unwrap();
    let rt = Runtime::new().unwrap();
    let r = rt.block_on(async { commit_item(params, &cfg, &input).await });
    task_status.write().unwrap().busy = None;
    Response::Commit(r.map(|_| input))
}
