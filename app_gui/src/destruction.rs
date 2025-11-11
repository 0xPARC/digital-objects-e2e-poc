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
    Config, CraftedItem, ProductionType, Recipe, commit_item, craft_item, load_item, log_init,
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

use crate::{App, Committing, ItemView, Request, Response, TaskStatus, utils::result2text};

#[derive(Default)]
pub struct Destruction {
    pub item_index: Option<usize>,
    pub result: Option<Result<PathBuf>>,
}

impl App {
    // UI for the destruction of items.
    pub(crate) fn ui_destroy(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let all_items = self.all_items();
        egui::Grid::new("destruction").show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.heading("Destroy");
            ui.end_row();
        });
        ui.separator();
        let frame = Frame::default().inner_margin(4.0);
        let (_, dropped_payload) =
            ui.dnd_drop_zone::<usize, ()>(frame, |ui| match &self.destruction.item_index {
                Some(i) => {
                    if let Some(name) = self.items.get(*i).map(|item| item.name.to_string()) {
                        self.destruction.result = None;
                        ui.label(name);
                    } else {
                        self.destruction.result = Some(Err(anyhow!(
                            "Item '{}' has already been used or destroyed!",
                            all_items[*i].name
                        )));
                        ui.label("...");
                    }
                }
                _ => {
                    ui.label("...");
                }
            });
        ui.end_row();
        if let Some(i) = dropped_payload {
            self.destruction.item_index = Some(*i);
        }

        let mut button_destroy_clicked = false;

        egui::Grid::new("destruction buttons").show(ui, |ui| {
            if let (Some(i), None) = (self.destruction.item_index, &self.destruction.result) {
                button_destroy_clicked = ui.button("Destroy").clicked();

                if button_destroy_clicked {
                    let item = all_items[i].path.clone();
                    self.task_req_tx
                        .send(Request::Destroy {
                            params: self.params.clone(),
                            cfg: self.cfg.clone(),
                            item,
                        })
                        .unwrap();
                }
            }
            ui.label(result2text(&self.destruction.result));
            ui.end_row();
        });
    }
}
