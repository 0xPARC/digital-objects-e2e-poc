// TODO: Remove after completing the gui_app
#![allow(unreachable_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

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

mod app;
mod crafting;
mod item_view;
mod task_system;
mod utils;

use app::*;
use crafting::*;
use item_view::*;
use task_system::*;

fn main() -> Result<()> {
    log_init();
    load_dotenv()?;
    let cfg = Config::from_env()?;
    info!(?cfg, "Loaded config");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_maximized(true),
        ..Default::default()
    };
    let params = Params::default();
    let app = Box::new(app::App::new(cfg, params)?);
    eframe::run_native(
        "PODCraft",
        options,
        Box::new(|cc| {
            // This gives us image support:
            egui_extras::install_image_loaders(&cc.egui_ctx);

            Ok(app)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    Ok(())
}

#[derive(Default)]
struct Committing {
    result: Option<Result<()>>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process task response messages
        if let Ok(res) = self.task_res_rx.try_recv() {
            match res {
                Response::Craft(r) => {
                    if let Ok(entry) = &r {
                        self.load_item(entry, false).unwrap();
                    }
                    self.refresh_items().unwrap();
                    self.crafting.input_items = HashMap::new();
                    self.crafting.commit_result = None;
                    self.crafting.craft_result = Some(r)
                }
                Response::Commit(r) => self.crafting.commit_result = Some(r),
                Response::Null => {}
            }
        }

        // Left side panel "Item list"
        egui::SidePanel::left("item list").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Digital Objects");
                if ui.button("Refresh").clicked() {
                    self.refresh_items().unwrap();
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, item) in self.items.iter().enumerate() {
                    ui.dnd_drag_source(egui::Id::new(item.name.clone()), i, |ui| {
                        ui.label(&item.name);
                    });
                }
            });
            ui.separator();
            egui::Grid::new("used items title").show(ui, |ui| {
                ui.collapsing("Used items", |ui| {
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(100.0)
                        .show(ui, |ui| {
                            for (i, item) in self.used_items.iter().enumerate() {
                                ui.dnd_drag_source(
                                    egui::Id::new(item.name.clone()),
                                    self.items.len() + i,
                                    |ui| {
                                        ui.label(&item.name);
                                    },
                                );
                            }
                        })
                })
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| self.update_item_view_ui(ui));

        // If the task is busy, display a spinner and the task name,
        // else display the action UI.
        let task_status = self.task_status.read().unwrap().clone();
        egui::SidePanel::right("actions").show(ctx, |ui| {
            if let Some(task) = task_status.busy {
                ui.horizontal_centered(|ui| {
                    ui.spinner();
                    ui.heading(task);
                });
            } else {
                self.update_action_ui(ctx, ui);
            }
            // Display window(s).
            if self.modal_new_predicates {
                self.ui_new_predicate(ctx, ui);
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&egui_glow::glow::Context>) {
        self.task_req_tx.send(Request::Exit).unwrap();
        // if the task is not busy it should terminate before 100 ms
        thread::sleep(time::Duration::from_millis(100));
    }
}

impl App {
    pub fn update_action_ui(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                for verb in Verb::list() {
                    if ui
                        .selectable_label(Some(verb) == self.crafting.selected_verb, verb.as_str())
                        .clicked()
                    {
                        self.crafting.selected_verb = Some(verb);
                        self.crafting.selected_process = verb.default_process();
                    }
                }
                if ui
                    .selectable_label(self.modal_new_predicates, "+ New Predicate")
                    .clicked()
                {
                    self.modal_new_predicates = true;
                }
            });
            ui.separator();
            self.ui_craft(ctx, ui);
        });
    }
}
