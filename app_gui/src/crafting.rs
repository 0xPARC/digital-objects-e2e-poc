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
pub struct Crafting {
    pub selected_recipe: Option<Recipe>,
    // Input index to item index
    pub input_items: HashMap<usize, usize>,
    pub output_filename: String,
    pub craft_result: Option<Result<PathBuf>>,
    pub commit_result: Option<Result<PathBuf>>,
}

impl Crafting {
    pub fn select(&mut self, recipe: Recipe) {
        if Some(recipe) != self.selected_recipe {
            *self = Self::default();
            self.selected_recipe = Some(recipe);
        }
    }
}

pub fn recipe_inputs(r: &Recipe) -> Vec<Recipe> {
    match r {
        Recipe::Bronze => vec![Recipe::Tin, Recipe::Copper],
        _ => vec![],
    }
}
pub fn recipe_statement(r: &Recipe) -> &'static str {
    match r {
        Recipe::Copper => {
            r#"
use intro Pow(count, input, output) from 0x3493488bc23af15ac5fabe38c3cb6c4b66adb57e3898adf201ae50cc57183f65

IsCopper(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    Equal(inputs, {})
    DictContains(ingredients, "blueprint", "copper")
    Pow(3, ingredients, work)
)"#
        }
        Recipe::Tin => {
            r#"
IsTin(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    Equal(inputs, {})
    DictContains(ingredients, "blueprint", "tin")
)"#
        }
        Recipe::Bronze => {
            r#"
IsBronze(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "bronze")

    // 2 ingredients
    SetInsert(s1, {}, tin)
    SetInsert(inputs, s1, copper)

    // Recursively prove the ingredients are correct.
    IsTin(tin)
    IsCopper(copper)
)"#
        }
    }
}

impl App {
    // Crafting panel
    // UI for producing new items through Mine & Craft
    pub(crate) fn ui_produce(
        &mut self,
        ctx: &egui::Context,
        ui: &mut Ui,
        production_type: ProductionType,
    ) {
        let mut selected_recipe = self.crafting.selected_recipe;
        egui::Grid::new("mine title").show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.heading(production_type.to_string());
            ui.end_row();
        });
        ui.separator();
        egui::ComboBox::from_id_salt("items to mine/craft")
            .selected_text(selected_recipe.map(|r| r.to_string()).unwrap_or_default())
            .show_ui(ui, |ui| {
                for recipe in &self.recipes {
                    if recipe.production_type() == production_type {
                        ui.selectable_value(
                            &mut selected_recipe,
                            Some(*recipe),
                            recipe.to_string(),
                        );
                    }
                }
            });
        if let Some(selected_recipe) = selected_recipe {
            self.crafting.select(selected_recipe);
        }

        if let Some(recipe) = self.crafting.selected_recipe {
            let inputs = recipe_inputs(&recipe);
            if !inputs.is_empty() {
                ui.heading("Inputs:");
            }
            egui::Grid::new("crafting inputs").show(ui, |ui| {
                for (input_index, input) in inputs.iter().enumerate() {
                    ui.label(format!("{input}:"));
                    let frame = Frame::default().inner_margin(4.0);
                    let (_, dropped_payload) = ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                        if let Some(index) = self.crafting.input_items.get(&input_index) {
                            ui.label(self.all_items()[*index].name.to_string());
                        } else {
                            ui.label("...");
                        }
                    });
                    ui.end_row();
                    if let Some(index) = dropped_payload {
                        self.crafting.input_items.insert(input_index, *index);
                    }
                }
            });

            ui.horizontal(|ui| {
                ui.label("filename:");
                // suggest a default name for the file
                self.crafting.output_filename = format!(
                    "{}/{:?}_{}",
                    self.cfg.pods_path,
                    recipe,
                    self.items.len() + self.used_items.len()
                );
                ui.text_edit_singleline(&mut self.crafting.output_filename);
            });

            let mut button_craft_clicked = false;
            let mut button_commit_clicked = false;
            egui::Grid::new("crafting buttons").show(ui, |ui| {
                button_craft_clicked = ui.button("Craft").clicked();
                ui.label(result2text(&self.crafting.craft_result));
                ui.end_row();
                button_commit_clicked = ui.button("Commit").clicked();
                ui.label(result2text(&self.crafting.commit_result));
                ui.end_row();
            });
            if button_craft_clicked {
                if self.crafting.output_filename.is_empty() {
                    self.crafting.craft_result = Some(Err(anyhow!("Please enter a filename.")));
                } else {
                    let output =
                        Path::new(&self.cfg.pods_path).join(&self.crafting.output_filename);
                    let input_paths = (0..inputs.len())
                        .map(|i| {
                            self.crafting
                                .input_items
                                .get(&i)
                                .map(|i| self.all_items()[*i].path.clone())
                        })
                        .collect::<Option<Vec<_>>>();

                    match input_paths {
                        None => {
                            self.crafting.craft_result =
                                Some(Err(anyhow!("Please provide all inputs.")))
                        }
                        Some(input_paths) => {
                            self.task_req_tx
                                .send(Request::Craft {
                                    params: self.params.clone(),
                                    pods_path: self.cfg.pods_path.clone(),
                                    recipe,
                                    output,
                                    input_paths,
                                })
                                .unwrap();
                        }
                    }
                };
            }

            if button_commit_clicked {
                if self.crafting.output_filename.is_empty() {
                    self.crafting.commit_result = Some(Err(anyhow!("Please enter a filename.")));
                } else {
                    let input = Path::new(&self.cfg.pods_path).join(&self.crafting.output_filename);
                    self.task_req_tx
                        .send(Request::Commit {
                            params: self.params.clone(),
                            cfg: self.cfg.clone(),
                            input,
                        })
                        .unwrap();
                }
            }

            ui.heading("Predicate:");
            egui::ScrollArea::vertical()
                .min_scrolled_height(200.0)
                .show(ui, |ui| {
                    ui.separator();
                    let s = recipe_statement(&recipe);
                    ui.add(Label::new(RichText::new(s).monospace()).wrap());
                });
        }
    }

    pub(crate) fn ui_new_predicate(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let language: String = "js".to_string();

        if self.modal_new_predicates {
            let theme = egui_extras::syntax_highlighting::CodeTheme::default();
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                let mut layout_job = egui_extras::syntax_highlighting::highlight(
                    ui.ctx(),
                    ui.style(),
                    &theme,
                    buf.as_str(),
                    &language,
                );
                layout_job.wrap.max_width = wrap_width;
                ui.fonts_mut(|f| f.layout_job(layout_job))
            };

            egui::Window::new("New Predicate")
                .collapsible(true)
                .movable(true)
                .resizable([true, true])
                .title_bar(true)
                .open(&mut self.modal_new_predicates)
                .show(ctx, |ui| {
                    let size = egui::vec2(ui.available_width(), 200.0);
                    ui.add_sized(
                        size,
                        egui::TextEdit::multiline(&mut self.code_editor_content)
                            .font(egui::TextStyle::Monospace)
                            .code_editor()
                            .desired_rows(10)
                            .lock_focus(true)
                            .desired_width(f32::INFINITY)
                            .layouter(&mut layouter),
                    );

                    egui::Grid::new("modal btns").show(ui, |ui| {
                        ui.add_enabled(false, egui::Button::new("Create!"));
                    });
                });
        }
    }
}
