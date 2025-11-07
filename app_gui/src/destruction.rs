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
    // Destruction panel
    pub fn update_destruction_ui(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        egui::Grid::new("destruction").show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.vertical(|ui| {
                self.ui_destroy(ctx, ui);
                ui.end_row();
            });
        });
    }

    // UI for the destruction of items.
    fn ui_destroy(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let mut item_to_destroy = self.destruction.item_index;
        let all_items = self.all_items();
        egui::Grid::new("destruction").show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.heading("Destroy");
            ui.end_row();
        });
        ui.separator();
        egui::ComboBox::from_label("")
            .selected_text(
                item_to_destroy
                    .map(|i| all_items[i].name.clone())
                    .unwrap_or_default(),
            )
            .show_ui(ui, |ui| {
                for (i, item) in all_items.iter().enumerate() {
                    ui.selectable_value(&mut item_to_destroy, Some(i), &item.name);
                }
            });
        if let Some(i) = item_to_destroy {
            self.destruction.item_index = Some(i);
        }

        let mut button_destroy_clicked = false;
        egui::Grid::new("destruction buttons").show(ui, |ui| {
            button_destroy_clicked = ui.button("Destroy").clicked();
            ui.label(result2text(&self.destruction.result));
            ui.end_row();
        });
        if button_destroy_clicked {
            match self.destruction.item_index {
                None => {
                    self.destruction.result =
                        Some(Err(anyhow!("Please select an item to destroy.")));
                }
                Some(i) => {
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
        }
    }
}
