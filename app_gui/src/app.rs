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
    Config, CraftedItem, Recipe, USED_ITEM_SUBDIR_NAME, commit_item, craft_item, load_item,
    log_init,
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

use crate::{
    Committing, Crafting, Destruction, ItemView, Request, Response, TaskStatus,
    task_system::handle_req,
};

#[derive(Clone)]
pub struct Item {
    pub name: String,
    pub id: Hash,
    pub crafted_item: CraftedItem,
    pub path: PathBuf,
}

pub struct App {
    pub cfg: Config,
    pub params: Params,
    pub recipes: Vec<Recipe>,
    pub items: Vec<Item>,
    pub used_items: Vec<Item>,
    pub item_view: ItemView,
    pub crafting: Crafting,
    pub committing: Committing,
    pub destruction: Destruction,
    pub task_req_tx: mpsc::Sender<Request>,
    pub task_res_rx: mpsc::Receiver<Response>,
    pub _task_handler: JoinHandle<()>,
    pub task_status: Arc<RwLock<TaskStatus>>,
    pub selected_tab: usize,
    pub modal_new_predicates: bool, // modal for writing new predicates
    pub code_editor_content: String,
    pub dev_mode: bool,
}

impl App {
    pub fn new(cfg: Config, params: Params) -> Result<Self> {
        let task_status = Arc::new(RwLock::new(TaskStatus::default()));
        let task_status_cloned = task_status.clone();
        let (req_tx, req_rx) = channel();
        let (res_tx, res_rx) = channel();
        let task_handler = thread::spawn(move || {
            let task_status = task_status_cloned;
            loop {
                match req_rx.recv() {
                    Ok(req) => {
                        if matches!(req, Request::Exit) {
                            return;
                        }
                        res_tx.send(handle_req(&task_status, req)).unwrap();
                    }
                    Err(e) => {
                        error!("channel error: {e}");
                        return;
                    }
                }
            }
        });
        let recipes = Recipe::list();
        let code: String = r#"
IsTinPremium(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "tinpremium")

    // 2 ingredients
    SetInsert(s1, {}, tin1)
    SetInsert(inputs, s1, tin2)

    // prove the ingredients are correct.
    IsTin(tin1)
    IsTin(tin2)
)"#
        .into();

        let mut app = Self {
            cfg,
            params,
            recipes,
            items: vec![],
            used_items: vec![],
            item_view: Default::default(),
            crafting: Default::default(),
            committing: Default::default(),
            destruction: Default::default(),
            task_req_tx: req_tx,
            task_res_rx: res_rx,
            _task_handler: task_handler,
            task_status,
            selected_tab: 0,
            modal_new_predicates: false,
            code_editor_content: code.clone(),
            dev_mode: false,
        };
        app.refresh_items()?;
        Ok(app)
    }

    /// returns a vector with [self.items | self.used_items]
    pub fn all_items(&self) -> Vec<Item> {
        [self.items.clone(), self.used_items.clone()].concat()
    }

    pub fn load_item(&mut self, entry: &Path, used: bool) -> Result<()> {
        log::debug!("loading {entry:?}");
        let name = entry.file_name().unwrap().to_str().unwrap().to_string();
        let crafted_item = load_item(entry)?;
        let id = Hash::from(
            crafted_item.pod.public_statements[0].args()[0]
                .literal()
                .unwrap()
                .raw(),
        );
        let item = Item {
            name,
            id,
            crafted_item,
            path: entry.to_path_buf(),
        };
        if used {
            self.used_items.push(item);
        } else {
            self.items.push(item);
        }
        self.items.sort_by_key(|item| item.name.clone());
        self.used_items.sort_by_key(|item| item.name.clone());
        Ok(())
    }

    pub fn refresh_items(&mut self) -> Result<()> {
        // create 'pods_path' & 'pods_path/used' dir in case they do not exist
        fs::create_dir_all(format!("{}/{}", &self.cfg.pods_path, USED_ITEM_SUBDIR_NAME))?;

        self.items = Vec::new();
        self.used_items = Vec::new();
        log::info!("Loading items...");
        for entry in fs::read_dir(&self.cfg.pods_path)? {
            let entry = entry?;
            // skip dirs
            if !entry.file_type()?.is_dir() {
                self.load_item(&(entry.path()), false)?;
            }
        }

        log::info!("Loading used items...");
        for entry in fs::read_dir(format!("{}/{}", &self.cfg.pods_path, USED_ITEM_SUBDIR_NAME))? {
            let entry = entry?;
            // skip dirs
            if !entry.file_type()?.is_dir() {
                self.load_item(&(entry.path()), true)?;
            }
        }
        Ok(())
    }
}
