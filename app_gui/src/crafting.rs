use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};
use app_cli::Recipe;
use egui::{Frame, ImageSource, Label, RichText, Ui};
use enum_iterator::{Sequence, all};
use lazy_static::lazy_static;
use strum::IntoStaticStr;

use crate::{App, Request, utils::result2text};

#[derive(Debug, Clone, Copy, PartialEq, IntoStaticStr)]
pub enum Process {
    Stone,
    Wood,
    Axe,
    WoodenAxe,
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
    reconf_action: &'static [&'static str],
}

lazy_static! {
    static ref STONE_DATA: ProcessData = ProcessData {
        description: "Stone.  Hard to find.",
        outputs: &["Stone"],
        predicate: r#"
use intro Pow(count, input, output) from 0x3493488bc23af15ac5fabe38c3cb6c4b66adb57e3898adf201ae50cc57183f65

IsStone(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    Equal(inputs, {})
    DictContains(ingredients, "blueprint", "stone")
    Pow(3, ingredients, work)
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
    static ref AXE_DATA: ProcessData = ProcessData {
        description: "Axe.  Easy to craft.",
        input_ingredients: &["Wood", "Stone"],
        outputs: &["Axe"],
        predicate: r#"
IsAxe(item, private: ingredients, inputs, key, work, s1, wood, stone) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "axe")
    Equal(work, {})

    // 2 ingredients
    SetInsert(s1, {}, wood)
    SetInsert(inputs, s1, stone)

    // prove the ingredients are correct.
    IsWood(wood)
    IsStone(stone)
)"#,
        ..Default::default()
    };
    static ref WOODEN_AXE_DATA: ProcessData = ProcessData {
        description: "Wooden Axe.  Easy to craft.",
        input_ingredients: &["Wood", "Wood"],
        outputs: &["WoodenAxe"],
        predicate: r#"
IsWoodenAxe(item, private: ingredients, inputs, key, work, s1, wood1, wood2) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "wooden-axe")
    Equal(work, 0)

    // 2 ingredients
    SetInsert(s1, {}, wood1)
    SetInsert(inputs, s1, wood2)

    // prove the ingredients are correct.
    IsWood(wood1)
    IsWood(wood2)
)"#,
        ..Default::default()
    };
    // Mock
    static ref DESTROY_DATA: ProcessData = ProcessData {
        description: "Destroy an object.",
        input_ingredients: &["Item to destroy"],
        outputs: &[],
        predicate: r#"
Destroy(batch, private: ingredients, inputs, key, work, item) = AND(
    BatchDef(batch, ingredients, inputs, key, work)
    Equal(batch, {})
    SetInsert(inputs, {}, item)
)"#,
        ..Default::default()
    };
    static ref TOMATO_DATA: ProcessData = ProcessData {
        description: "Produces a Tomato.  Requires farm level 1.",
        input_facilities: &["Farm level 1"],
        input_ingredients: &["Tomato Seed"],
        outputs: &["Tomato"],
        predicate: r#"
TomatoRecipe(batch, farm_level, ingredients, inputs, key, work, private: s1, tomato_farm, tomato_seed) = AND(
    BatchDef(batch, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "tomato")
    Equal(work, {})

    SetInsert(s1, {}, tomato_farm)
    SetInsert(inputs, s1, tomato_seed)

    IsTomatoSeed(tomato_seed)
    IsFarm(tomato_farm, farm_level)
    Ge(level, 1)
)

IsTomato(item, private: batch, ingredients, inputs, key, work, farm_level) = AND(
    TomatoRecipe(batch, farm_level, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "tomato")
)
    
UsedFarm(item, level, private: batch, ingredients, inputs, key, work) = AND(
    TomatoRecipe(batch, level ingredients, inputs, key, work)
    ItemInBatch(item, batch, "farm")
)

IsFarm(item, level, private: batch ingredients, inputs, key, work) = OR(
    UsedFarm(item, level)
    NewFarm(item, level)
)"#,
        ..Default::default()
    };
    static ref STEEL_SWORD_DATA: ProcessData = ProcessData {
        description: "Produces a steel sword.  Requires a forge.",
        input_facilities: &["Forge"],
        input_ingredients: &["Steel", "Steel", "Wood"],
        outputs: &["Steel Sword"],
        predicate: r#"
SteelSwordRecipe(batch, ingredients, inputs, key, work, forge, steel1, steel2, wood, s1, s2, s3, s4) = AND(
    BatchDef(batch, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "steel sword")
    Equal(work, {})

    SetInsert(s1, {}, forge)
    SetInsert(s2, s1, steel1)
    SetInsert(s3, s2, steel2)
    SetInsert(s4, s3, wood)
    SetInsert(inputs, s4, forge)
    
    IsForge(forge)
    IsSteel(steel1)
    IsSteel(steel2)
    IsWood(wood)
)

IsSteelSword(item, private: batch, ingredients, inputs, key, work) = AND(
   SteelSwordRecipe(batch, ingredients, inputs, key, work)
   ItemInBatch(item, batch, "steel sword")
)

UsedForge(item, private: batch, ingredients, inputs, key, work) = AND(
    SteelSwordRecipe(batch, new_durability, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "forge")
)

IsForge(item, private: batch ingredients, inputs, key, work) = OR(
    UsedForge(item)
    NewForge(item)
)"#,
        ..Default::default()
    };
    static ref DIS_H2O_DATA: ProcessData = ProcessData {
        description: "Disassemble H2O into 2xH and 1xO.",
        input_ingredients: &["H2O"],
        outputs: &["H", "H", "O"],
        predicate: r#"
DisassembleH2O(batch, ingredients, inputs, key, work) = AND(
    ItemDef(items, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint_0", "H")
    DictContains(ingredients, "blueprint_1", "H")
    DictContains(ingredients, "blueprint_2", "O")
    Equal(work, {})

    SetInsert(inputs, {}, h2o)
    IsH2O(h2o)
)

IsH0(item, private: batch, ingredients, inputs, key, work) = AND(
    DisassembleH2O(batch, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "0")
)
IsH1(item, private: batch, ingredients, inputs, key, work) = AND(
    DisassembleH2O(batch, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "1")
)
IsH(item) = OR(
    IsH0(item)
    IsH1(item)
)
    
IsO(item, private: batch, ingredients, inputs, key, work) = AND(
    DisassembleH2O(batch, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "2")
)"#,
        ..Default::default()
    };
    static ref REFINED_URANIUM_DATA: ProcessData = ProcessData {
        description: "Produces refined Uranium.  It takes about 30 minutes.",
        input_ingredients: &["Uranium"],
        outputs: &["Refined Uranium"],
        predicate: r#"
IsRefinedUranium(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "refined_uranium")

    SetInsert(inputs, {}, uranium)
    IsUranium(uranium)
    Pow(100, ingredients, work)
)"#,
        ..Default::default()
    };
    static ref COAL_DATA: ProcessData = ProcessData {
        description: "Mine coal.  Requires a Pick Axe with >= 50% durability, and consumes 1% of it",
        input_tools: &["Pick Axe"],
        outputs: &["Coal"],
        predicate: r#"
CoalMiningRecipe(batch, new_durability, ingredients, inputs, key, work) = AND(
    BatchDef(batch, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "coal")
    SetInsert(inputs, {}, pick_axe)
    IsPickAxe(pick_axe, durability)
    GtEq(durability, 50)
    SumOf(new_durability, durability, -1)
    Equal(work, 0)
)

IsCoal(item, private: ingredients, inputs, key, work) = AND(
    CoalMiningRecipe(batch, new_durability, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "coal")
)

UsedPickAxe(item, new_durability, private: ingredients, inputs, key, work) = AND(
    CoalMiningRecipe(batch, new_durability, ingredients, inputs, key, work)
    ItemInBatch(item, batch, "pickaxe")
)

IsPickAxe(item, durability, private: ingredients, inputs, key, work) = OR(
    UsedPickAxe(item, durability)
    NewPickAxe(item, durability)
)"#,
        ..Default::default()
    };
    #[derive(Debug)]
    static ref INNER_LINES: String = {
        let mut tree_house_is_wood_lines = String::new();
        for i in 0..N_WOODS {
            tree_house_is_wood_lines.push_str(&format!(
                "\n    SetInsert(inputs{}, {}, wood{i})\n    IsWood(wood{i})",
                if i==(N_WOODS-1) { String::from("") } else { i.to_string() },
                if i==0 { String::from("{}") } else { format!("inputs{}", i-1)}
            ));
        }
        format!(r#"
IsTreeHouse(item, private: ingredients, inputs, key, work) = AND(
    ItemDef(item, ingredients, inputs, key, work)
    DictContains(ingredients, "blueprint", "wood")
    Equal(work, {{}})

    {tree_house_is_wood_lines}
)"#)
    };
    static ref TREE_HOUSE_DATA: ProcessData = ProcessData {
        description: "Produces a Tree House.",
        input_facilities: &[],
        input_ingredients: &["Wood";N_WOODS],
        outputs: &["Tree House"],
        predicate: &INNER_LINES,
        ..Default::default()
    };
    static ref RECONF_RUBIKS_CUBE: ProcessData = ProcessData {
        description: "Move layers of a Rubik's Cube.",
        input_ingredients: &["Rubik's Cube"],
        reconf_action: &["U", "D", "R", "L", "F", "B", "Uw", "Dw", "Rw", "Lw", "Fw", "Bw", "x", "y", "z", "M", "E", "S"],
        predicate: r#"

// [...]

MoveLeft(new, old, op) = AND(
    Equal(op.name, "U")
    RotateLayer(new.faces, old.faces, 0, "left")
)

// [...]

MovedRubiksCube(new, old, op) = OR(
    MoveLeft(new, old, op)
    MoveRight(new, old, op)
    MoveUp(new, old, op)
    MoveDown(new, old, op)
)"#,
        ..Default::default()
    };
    static ref RECONF_DECK_CARDS: ProcessData = ProcessData {
        description: "Rearrange a Deck of Cards.",
        input_ingredients: &["Deck of Cards"],
        reconf_action: &["Rotate Clockwise", "Rotate Counter-Clockwise", "Random Shuffle"],
        predicate: r#"

// [...]

RotateClockwise(new, old, op) = AND(
    Equal(op.name, "rotate-clockwise")
    Equal(new.cards[0], old.cards[51])
    Equal(new.cards[1], old.cards[0])
    Equal(new.cards[2], old.cards[1])
    // [...]
    Equal(new.cards[51], old.cards[50])
)

// [...]

RearrangedDeckOfCards(new, old, op) = OR(
    RotateClockwise(new, old, op)
    RotateCounterClockwise(new, old, op)
    RandomShuffle(new, old, op)
)"#,
        ..Default::default()
    };
    static ref RECONF_REFRIGERATOR: ProcessData = ProcessData {
        description: "Rearrange the contents of a Refrigerator.",
        input_ingredients: &["Refrigerator"],
        reconf_action: &["Open in Layout Editor"],
        predicate: r#"
// [...]

RearrangedRefrigerator(new, old, op) = AND(
    Equal(new.objects, old.objects)
    NoOverlap(new.objects, new.positions)
)"#,
        ..Default::default()
    };
    static ref RECONF_FARM_LVL_1: ProcessData = ProcessData {
        description: "Maintain a Farm.",
        input_ingredients: &["Farm Level 1"],
        reconf_action: &["Fertilize", "Till"],
        predicate: r#"
// [...]

Fertilize(new, old, op) = AND(
    Equal(op.name, "fertilize")
    DictUpdate(new, old, "fertilized", true)
)

// [...]

MaintainedFarm(new, old, op) = OR(
    Fertilize(new, old, op)
    Till(new, old, op)
)"#,
        ..Default::default()
    };
}
const N_WOODS: usize = 100;

impl Process {
    #[allow(clippy::let_and_return)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mock(s) => {
                let s = s.strip_prefix("Disassemble-").unwrap_or(s);
                let s = s.strip_prefix("Refine-").unwrap_or(s);
                let s = s.strip_prefix("Reconfigure-").unwrap_or(s);
                s
            }
            v => v.into(),
        }
    }
    // Returns None if the Process is mock
    pub fn recipe(&self) -> Option<Recipe> {
        match self {
            Self::Stone => Some(Recipe::Stone),
            Self::Wood => Some(Recipe::Wood),
            Self::Axe => Some(Recipe::Axe),
            Self::WoodenAxe => Some(Recipe::WoodenAxe),
            Self::Mock(_) => None,
        }
    }

    pub fn data(&self) -> &'static ProcessData {
        match self {
            Self::Stone => &STONE_DATA,
            Self::Wood => &WOOD_DATA,
            Self::Axe => &AXE_DATA,
            Self::WoodenAxe => &WOODEN_AXE_DATA,
            Self::Mock("Destroy") => &DESTROY_DATA,
            Self::Mock("Tomato") => &TOMATO_DATA,
            Self::Mock("Steel Sword") => &STEEL_SWORD_DATA,
            Self::Mock("Disassemble-H2O") => &DIS_H2O_DATA,
            Self::Mock("Refine-Uranium") => &REFINED_URANIUM_DATA,
            Self::Mock("Coal") => &COAL_DATA,
            Self::Mock("Reconfigure-Rubik's Cube") => &RECONF_RUBIKS_CUBE,
            Self::Mock("Reconfigure-Deck of Cards") => &RECONF_DECK_CARDS,
            Self::Mock("Reconfigure-Refrigerator") => &RECONF_REFRIGERATOR,
            Self::Mock("Reconfigure-Farm Level 1") => &RECONF_FARM_LVL_1,
            Self::Mock("Tree House") => &TREE_HOUSE_DATA,
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
    Produce,
    Reconfigure,
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
            Self::Mine => vec![Mock("Coal")],
            Self::Gather => vec![Stone, Wood],
            Self::Refine => vec![Mock("Refine-Uranium")],
            Self::Reconfigure => vec![
                Mock("Reconfigure-Rubik's Cube"),
                Mock("Reconfigure-Deck of Cards"),
                Mock("Reconfigure-Refrigerator"),
                Mock("Reconfigure-Farm Level 1"),
            ],
            Self::Craft => vec![Axe, WoodenAxe, Mock("Tree House")],
            Self::Produce => vec![Mock("Tomato"), Mock("Steel Sword")],
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
    pub selected_action: Option<&'static str>,
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
        let mut button_craft_clicked = false;
        let mut button_commit_clicked = false;
        let mut button_craft_and_commit_clicked = false;

        let selected_verb = match self.crafting.selected_verb {
            None => return,
            Some(v) => v,
        };
        let mut selected_process = self.crafting.selected_process;
        // Block1: Verb + Process
        // egui::Grid::new("verb + process").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading(format!("{}:", selected_verb.as_str()));

            if !selected_verb.hide_process() {
                egui::ComboBox::from_id_salt("process selection")
                    .selected_text(selected_process.map(|r| r.as_str()).unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for process in selected_verb
                            .processes()
                            .into_iter()
                            .filter(|p| self.mock_mode || (p.recipe().is_some()))
                        {
                            ui.selectable_value(
                                &mut selected_process,
                                Some(process),
                                process.as_str(),
                            );
                        }
                    });
            }
            if let Some(process) = selected_process {
                self.crafting.select(process);
                if process.recipe().is_none() {
                    ui.colored_label(egui::Color32::from_rgb(81, 77, 188), "(mock)");
                }
            }

            // Button for Execute process
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                        let button = ui.add_enabled(
                            self.crafting.selected_process.is_some(),
                            egui::Button::new(egui::RichText::new("Execute process").size(15.0)),
                        );
                        button_craft_and_commit_clicked = button.clicked();
                        ui.label(result2text(&self.crafting.commit_result));
                    });
                }
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let mut selected_action = self.crafting.selected_action;
        if let Some(process) = self.crafting.selected_process {
            let process_data = process.data();

            // Block2: Description
            ui.heading("Description:");
            ui.add(Label::new(RichText::new(process_data.description)).wrap());
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Block3: Configuration
            let inputs = process_data.input_ingredients;
            ui.columns_const(|[inputs_ui, outputs_ui]| {
                inputs_ui.heading("Inputs:");
                egui::ScrollArea::vertical()
                    .id_salt("inputs scroll")
                    .max_height(256.0)
                    .show(inputs_ui, |ui| {
                        ui.vertical(|ui| {
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
                    });

                let outputs = process_data.outputs;
                outputs_ui.heading("Outputs:");
                egui::ScrollArea::vertical()
                    .id_salt("outputs scroll")
                    .max_height(256.0)
                    .show(outputs_ui, |ui| {
                        ui.vertical(|ui| {
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
            });

            if !process_data.reconf_action.is_empty() {
                ui.add_space(8.0);
                ui.heading("State:");
                ui.label("[load object first]");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading("Action:");
                    egui::ComboBox::from_id_salt("reconf action")
                        .selected_text(selected_action.unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for action in process_data.reconf_action {
                                ui.selectable_value(&mut selected_action, Some(action), *action);
                            }
                        });
                });
            }

            self.crafting.selected_action = selected_action;

            // NOTE: If we don't show filenames in the left panel, then we shouldn't ask for a
            // filename either.
            if self.crafting.output_filename.is_empty() {
                self.crafting.output_filename =
                    format!("{:?}_{}", process, self.items.len() + self.used_items.len());
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Block4: Predicate
            let predicate = process_data.predicate.trim_start();
            ui.heading("Predicate:");
            egui::ScrollArea::vertical()
                .id_salt("predicate scroll")
                .max_height(512.0)
                .show(ui, |ui| {
                    Frame::NONE
                        .fill(if ctx.theme() == egui::Theme::Dark {
                            egui::Color32::from_gray(20)
                        } else if ctx.theme() == egui::Theme::Light {
                            egui::Color32::from_gray(240)
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Vec2::splat(8.0))
                        .show(ui, |ui| {
                            ui.add(Label::new(RichText::new(predicate).monospace()).wrap());
                        });
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
                    self.crafting.commit_result = Some(Err(anyhow!("Please enter a filename.")));
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
                            self.crafting.commit_result =
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

    pub(crate) fn ui_new_predicate(&mut self, ctx: &egui::Context) {
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

    pub(crate) fn ui_cursor<'a>(
        &self,
        ctx: &egui::Context,
        ui: &mut Ui,
        flag: bool,
        img: impl Into<ImageSource<'a>>,
    ) {
        if !flag {
            return;
        }
        let hover_pos = ctx.input(|input| {
            let pointer = &input.pointer;
            pointer.hover_pos()
        });

        if let Some(mousepos) = hover_pos {
            let pos = mousepos + egui::Vec2::splat(16.0);
            let rect = egui::Rect::from_min_size(pos, egui::Vec2::splat(64.0));
            egui::Image::new(img).corner_radius(5).paint_at(ui, rect);
        }
    }
}
