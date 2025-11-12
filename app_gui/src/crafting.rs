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
use app_cli::{Config, CraftedItem, ProductionType, Recipe, commit_item, craft_item, load_item};
use common::load_dotenv;
use craftlib::constants::COPPER_WORK;
use egui::{Color32, Frame, Label, RichText, Ui};
use enum_iterator::{Sequence, all};
use itertools::Itertools;
use lazy_static::lazy_static;
use pod2::{
    backends::plonky2::primitives::merkletree::MerkleProof,
    middleware::{
        Hash, Params, RawValue, Statement, StatementArg, TypedValue, Value, containers::Set,
    },
};
use strum::IntoStaticStr;
use tokio::runtime::Runtime;
use tracing::{error, info};

use crate::{App, Committing, ItemView, Request, Response, TaskStatus, utils::result2text};

#[derive(Debug, Clone, Copy, PartialEq, IntoStaticStr)]
pub enum Process {
    Copper,
    Tin,
    Wood,
    Bronze,
    BronzeAxe,
    Mock(&'static str),
}

#[derive(Default)]
pub struct ProcessData {
    description: &'static str,
    predicate: &'static str,
    input_facilities: &'static [&'static str],
    input_tools: &'static [&'static str],
    input_ingredients: &'static [&'static str],
    outputs: &'static [&'static str],
}

lazy_static! {
    static ref COPPER_DATA: ProcessData = ProcessData {
        description: "Copper.  Hard to find.",
        outputs: &["Copper"],
        predicate: r#"
use intro Pow(count, input, output) from 0x3493488bc23af15ac5fabe38c3cb6c4b66adb57e3898adf201ae50cc57183f65

IsCopper(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    Equal(inputs, {})
    DictContains(ingredients, "blueprint", "copper")
    Pow(3, ingredients, work)
)"#,
        ..Default::default()
    };
    static ref TIN_DATA: ProcessData = ProcessData {
        description: "Tin.  Easily available.",
        outputs: &["Tin"],
        predicate: r#"
IsTin(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    Equal(inputs, {})
    DictContains(ingredients, "blueprint", "tin")
)"#,
        ..Default::default()
    };
    static ref WOOD_DATA: ProcessData = ProcessData {
        description: "Wood.  Easily available.",
        outputs: &["Wood"],
        predicate: r#"
IsWood(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    Equal(inputs, {})
    DictContains(ingredients, "blueprint", "wood")
)"#,
        ..Default::default()
    };
    static ref BRONZE_DATA: ProcessData = ProcessData {
        description: "Bronze.  Easy to craft.",
        input_ingredients: &["Tin", "Copper"],
        outputs: &["Bronze"],
        predicate: r#"
IsBronze(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "bronze")

    // 2 ingredients
    SetInsert(s1, {}, tin)
    SetInsert(inputs, s1, copper)

    // prove the ingredients are correct.
    IsTin(tin)
    IsCopper(copper)
)"#,
        ..Default::default()
    };
    static ref BRONZE_AXE_DATA: ProcessData = ProcessData {
        description: "Bronze Axe.  Easy to craft.",
        input_ingredients: &["Wood", "Bronze"],
        outputs: &["Bronze Axe"],
        predicate: r#"
IsBronzeAxe(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "bronze-axe")

    // 2 ingredients
    SetInsert(s1, {}, wood)
    SetInsert(inputs, s1, bronze)

    // prove the ingredients are correct.
    IsWood(wood)
    IsBronze(bronze)
)"#,
        ..Default::default()
    };
    // Mock
    static ref DESTROY_DATA: ProcessData = ProcessData {
        description: "Destroy an object.",
        input_ingredients: &["Item to destroy"],
        outputs: &[],
        predicate: r#"
Destroy(void, private: ingredients, inputs, key, work) = AND(
    ItemDef(void, ingredients, inputs, key, work)
    SetInsert(inputs, {}, item)
)"#,
        ..Default::default()
    };
    static ref TOMATO_DATA: ProcessData = ProcessData {
        description: "Produces a Tomato.  Requires a Tomato Farm.",
        input_facilities: &["Tomato Farm"],
        input_ingredients: &["Tomato Seed"],
        outputs: &["Tomato"],
        predicate: r#"
IsTomato(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "tomato")

    SetInsert(s1, {}, tomato_farm)
    SetInsert(inputs, s1, tomato_seed)
    IsTomatoFarm(tomato_farm)
    IsTomatoSeed(tomato_seed)
)"#,
        ..Default::default()
    };
    static ref DIS_H2O_DATA: ProcessData = ProcessData {
        description: "Disassemble H2O into 2xH and 1xO.",
        input_ingredients: &["H2O"],
        outputs: &["H", "H", "O"],
        predicate: r#"
DisassembleH2O(items, private: ingredients, inputs, key, work) = AND(
    ItemDef(items, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint_0", "H")
    DictContains(ingredients, "blueprint_1", "H")
    DictContains(ingredients, "blueprint_2", "O")

    SetInsert(inputs, {}, h2o)
    IsH2O(h2o)
)"#,
        ..Default::default()
    };
    static ref REFINED_URANIUM_DATA: ProcessData = ProcessData {
        description: "Produces refined Uranium.  It takes about 30 minutes.",
        input_ingredients: &["Uranium"],
        outputs: &["Refined Uranium"],
        predicate: r#"
RefinedUranium(items, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "refined_uranium")

    SetInsert(inputs, {}, uranium)
    IsUranium(uranium)
    Pow(100, ingredients, work)
)"#,
        ..Default::default()
    };
    static ref STONE_DATA: ProcessData = ProcessData {
        description: "Mine a stone.  Requires a Pick Axe with >= 50% durability.",
        input_tools: &["Pick Axe"],
        outputs: &["Stone"],
        predicate: r#"
IsStone(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "stone")

    SetInsert(inputs, {}, pick_axe)
    IsPickAxe(pick_axe, durability)
    GtEq(durability, 50)
)"#,
        ..Default::default()
    };
}

impl Process {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mock("Disassemble-H2O") => "H2O",
            Self::Mock("Refine-Uranium") => "Uranium",
            Self::Mock(s) => s,
            v => self.into(),
        }
    }
    // Returns None if the Process is mock
    pub fn recipe(&self) -> Option<Recipe> {
        match self {
            Self::Copper => Some(Recipe::Copper),
            Self::Tin => Some(Recipe::Tin),
            Self::Wood => Some(Recipe::Wood),
            Self::Bronze => Some(Recipe::Bronze),
            Self::BronzeAxe => Some(Recipe::BronzeAxe),
            Self::Mock(_) => None,
        }
    }

    pub fn data(&self) -> &'static ProcessData {
        match self {
            Self::Copper => &COPPER_DATA,
            Self::Tin => &TIN_DATA,
            Self::Wood => &WOOD_DATA,
            Self::Bronze => &BRONZE_DATA,
            Self::BronzeAxe => &BRONZE_AXE_DATA,
            Self::Mock("Destroy") => &DESTROY_DATA,
            Self::Mock("Tomato") => &TOMATO_DATA,
            Self::Mock("Disassemble-H2O") => &DIS_H2O_DATA,
            Self::Mock("Refine-Uranium") => &REFINED_URANIUM_DATA,
            Self::Mock("Stone") => &STONE_DATA,
            Self::Mock(v) => unreachable!("data for mock {v}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Sequence, IntoStaticStr)]
pub enum Verb {
    Gather,
    Mine,
    Refine,
    Craft,
    Farm,
    Disassemble,
    Destroy,
}

impl Verb {
    pub fn as_str(&self) -> &'static str {
        let s: &'static str = self.into();
        s
    }
    pub fn list() -> Vec<Verb> {
        all::<Verb>().collect()
    }

    pub fn processes(&self) -> Vec<Process> {
        use Process::*;
        match self {
            Self::Mine => vec![Mock("Stone")],
            Self::Gather => vec![Copper, Tin, Wood],
            Self::Refine => vec![Mock("Refine-Uranium")],
            Self::Craft => vec![Bronze, BronzeAxe],
            Self::Farm => vec![Mock("Tomato")],
            Self::Disassemble => vec![Mock("Disassemble-H2O")],
            Self::Destroy => vec![Mock("Destroy")],
        }
    }

    pub fn default_process(&self) -> Option<Process> {
        match self {
            Self::Destroy => Some(Process::Mock("Destroy")),
            _ => None,
        }
    }

    #[allow(clippy::match_like_matches_macro)]
    pub fn hide_process(&self) -> bool {
        match self {
            Self::Destroy => true,
            _ => false,
        }
    }
}

#[derive(Default)]
pub struct Crafting {
    pub selected_verb: Option<Verb>,
    pub selected_process: Option<Process>,
    pub selected_recipe: Option<Recipe>,
    // Input index to item index
    pub input_items: HashMap<usize, usize>,
    pub output_filename: String,
    pub craft_result: Option<Result<PathBuf>>,
    pub commit_result: Option<Result<PathBuf>>,
}

impl Crafting {
    pub fn select(&mut self, process: Process) {
        if Some(process) != self.selected_process {
            let verb = self.selected_verb;
            *self = Self::default();
            self.selected_verb = verb;
            self.selected_process = Some(process);
        }
    }
}

impl App {
    // Generic ui for all verbs
    pub(crate) fn ui_craft(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let selected_verb = match self.crafting.selected_verb {
            None => return,
            Some(v) => v,
        };
        let mut selected_process = self.crafting.selected_process;
        // Block1: Verb + Process
        egui::Grid::new("verb + process").show(ui, |ui| {
            ui.heading(selected_verb.as_str());

            if !selected_verb.hide_process() {
                egui::ComboBox::from_id_salt("process selection")
                    .selected_text(selected_process.map(|r| r.as_str()).unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for process in selected_verb.processes() {
                            ui.selectable_value(
                                &mut selected_process,
                                Some(process),
                                process.as_str(),
                            );
                        }
                    });
            }
            ui.end_row();
        });
        ui.separator();
        if let Some(process) = selected_process {
            self.crafting.select(process);
        }

        if let Some(process) = self.crafting.selected_process {
            let process_data = process.data();

            // Block2: Description
            ui.heading("Description:");
            ui.add(Label::new(RichText::new(process_data.description)).wrap());
            ui.separator();

            // Block3: Configuration
            let inputs = process_data.input_ingredients;
            ui.columns_const(|[inputs_ui, outputs_ui]| {
                inputs_ui.vertical(|ui| {
                    ui.heading("Inputs");
                    egui::Grid::new("crafting inputs").show(ui, |ui| {
                        for (category, inputs) in
                            ["Production Facility", "Tools", "Ingredients"].iter().zip([
                                process_data.input_facilities,
                                process_data.input_tools,
                                process_data.input_ingredients,
                            ])
                        {
                            if inputs.is_empty() {
                                continue;
                            }
                            ui.label(format!("    {category}:"));
                            ui.end_row();
                            for (input_index, input) in inputs.iter().enumerate() {
                                ui.label(format!("        {input}:"));
                                let frame = Frame::default().inner_margin(4.0);
                                let (_, dropped_payload) =
                                    ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                                        if let Some(index) =
                                            self.crafting.input_items.get(&input_index)
                                        {
                                            self.name_with_img(
                                                ui,
                                                &self.all_items()[*index].name.to_string(),
                                            );
                                        } else {
                                            ui.label("...");
                                        }
                                    });
                                ui.end_row();
                                if let Some(index) = dropped_payload {
                                    self.crafting.input_items.insert(input_index, *index);
                                }
                            }
                        }
                    });
                });

                let outputs = process_data.outputs;
                outputs_ui.vertical(|ui| {
                    ui.heading("Outputs:");
                    ui.vertical(|ui| {
                        for output in outputs.iter() {
                            ui.horizontal(|ui| {
                                ui.label("  ");
                                self.name_with_img(ui, &output.to_string());
                            });
                        }
                    });
                });
            });

            // NOTE: If we don't show filenames in the left panel, then we shouldn't ask for a
            // filename either.
            self.crafting.output_filename =
                format!("{:?}_{}", process, self.items.len() + self.used_items.len());

            ui.separator();

            // Block4: Predicate
            let predicate = process_data.predicate.trim_start();
            ui.heading("Predicate:");
            egui::ScrollArea::vertical()
                .min_scrolled_height(200.0)
                .show(ui, |ui| {
                    ui.add(Label::new(RichText::new(predicate).monospace()).wrap());
                });

            let mut button_craft_clicked = false;
            let mut button_commit_clicked = false;
            let mut button_craft_and_commit_clicked = false;
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                if self.dev_mode {
                    ui.horizontal(|ui| {
                        button_commit_clicked = ui.button("Commit").clicked();
                        ui.label(result2text(&self.crafting.commit_result));
                    });
                    ui.horizontal(|ui| {
                        button_craft_clicked = ui.button("Craft (without Commit)").clicked();
                        ui.label(result2text(&self.crafting.craft_result));
                    });
                } else {
                    ui.horizontal(|ui| {
                        button_craft_and_commit_clicked = ui.button("Execute process").clicked();
                        ui.label(result2text(&self.crafting.commit_result));
                    });
                }
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
                            // This only goes through on non-mock processes
                            if let Some(recipe) = process.recipe() {
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

            if button_craft_and_commit_clicked {
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
                            // This only goes through on non-mock processes
                            if let Some(recipe) = process.recipe() {
                                self.task_req_tx
                                    .send(Request::CraftAndCommit {
                                        params: self.params.clone(),
                                        cfg: self.cfg.clone(),
                                        pods_path: self.cfg.pods_path.clone(),
                                        recipe,
                                        output,
                                        input_paths,
                                    })
                                    .unwrap();
                            }
                        }
                    }
                };
            }
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

    pub(crate) fn ui_danger(&self, ctx: &egui::Context, ui: &mut Ui) {
        if !self.danger {
            return;
        }
        let hover_pos = ctx.input(|input| {
            let pointer = &input.pointer;
            pointer.hover_pos()
        });
        let painter = ui.painter();

        if let Some(mousepos) = hover_pos {
            let pos = mousepos + egui::Vec2::splat(16.0);
            let rect = egui::Rect::from_min_size(pos, egui::Vec2::splat(64.0));
            egui::Image::new(egui::include_image!("../assets/water.png"))
                .corner_radius(5)
                .paint_at(ui, rect);
        }
    }
}
