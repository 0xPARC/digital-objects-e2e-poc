use std::{
    fs::{self},
    path::{Path, PathBuf},
    sync::RwLock,
};

use anyhow::{Result, anyhow};
use app_cli::{Config, Recipe, USED_ITEM_SUBDIR_NAME, commit_item, craft_item};
use pod2::middleware::Params;
use tokio::runtime::Runtime;

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
    Exit,
}

pub enum Response {
    Craft(Result<PathBuf>),
    Commit(Result<PathBuf>),
    CraftAndCommit(Result<PathBuf>),
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
            if let Response::Craft(Result::Err(e)) = craft(
                task_status,
                &params,
                pods_path,
                recipe,
                output.clone(),
                input_paths,
            ) {
                return Response::CraftAndCommit(Result::Err(e));
            };
            let res = commit(task_status, &params, cfg, output.clone());
            let r = match res {
                Response::Commit(result) => result,
                _ => Err(anyhow!("unexpected response")),
            };
            Response::CraftAndCommit(r)
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

    let start = std::time::Instant::now();
    let r = craft_item(params, recipe, &output, &input_paths);
    log::info!("[TIME] total Craft Item time: {:?}", start.elapsed());

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
