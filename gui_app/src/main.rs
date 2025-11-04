use std::{
    collections::HashMap,
    fs::{self},
    io,
    path::PathBuf,
};

use anyhow::{Result, anyhow};
use app::{Config, CraftedItem, Recipe, load_item, log_init};
use common::load_dotenv;
use eframe::egui;
use tracing::info;

fn main() -> Result<()> {
    log_init();
    load_dotenv()?;
    let cfg = Config::from_env()?;
    info!(?cfg, "Loaded config");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default(),
        ..Default::default()
    };
    let app = Box::new(App::new(cfg)?);
    eframe::run_native(
        "PODCraft",
        options,
        Box::new(|cc| {
            // This gives us image support:
            egui_extras::install_image_loaders(&cc.egui_ctx);

            Ok(app)
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[derive(Default)]
struct ItemView {
    selected_item: Option<usize>,
    verify_result: Option<Result<()>>,
}

impl ItemView {
    fn select(&mut self, index: usize) {
        if Some(index) != self.selected_item {
            self.selected_item = Some(index);
            self.verify_result = None;
        }
    }
}

#[derive(Default)]
struct Crafting {
    selected_recipe: Option<Recipe>,
    // Input index to item index
    input_items: HashMap<usize, usize>,
}

impl Crafting {
    fn select(&mut self, recipe: Recipe) {
        if Some(recipe) != self.selected_recipe {
            self.selected_recipe = Some(recipe);
            self.input_items = HashMap::new();
        }
    }
}

struct App {
    cfg: Config,
    items: Vec<(String, CraftedItem)>,
    item_view: ItemView,
    crafting: Crafting,
}

impl App {
    fn refresh_items(&mut self) -> Result<()> {
        log::info!("Loading items...");
        let mut entries = fs::read_dir(&self.cfg.pods_path)?
            .map(|res| res.map(|e| e.path()))
            .collect::<Result<Vec<_>, io::Error>>()?;
        entries.sort();
        let mut items = Vec::new();
        for entry in entries {
            let name = entry.file_name().unwrap().to_str().unwrap().to_string();
            log::debug!("loading {entry:?}");
            let item = load_item(&entry)?;
            items.push((name, item));
        }
        self.items = items;
        Ok(())
    }

    fn new(cfg: Config) -> Result<Self> {
        let mut app = Self {
            cfg,
            items: vec![],
            item_view: Default::default(),
            crafting: Default::default(),
        };
        app.refresh_items()?;
        Ok(app)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let frame = egui::Frame::default().inner_margin(4.0);
        egui::SidePanel::left("item list").show(ctx, |ui| {
            ui.heading("Item list");
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                // for (i, (name, _)) in self.items.iter().enumerate() {
                //     ui.selectable_value(&mut selected_item, Some(i), name);
                // }
                // ui.separator();
                for (i, (name, _)) in self.items.iter().enumerate() {
                    ui.dnd_drag_source(egui::Id::new(name), i, |ui| {
                        ui.label(name);
                    });
                }
            });
        });

        let item = self.item_view.selected_item.map(|i| &self.items[i]);
        let mut selected_recipe = self.crafting.selected_recipe;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns_const(|[item_view_ui, crafting_ui]| {
                item_view_ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.heading("Item: ");
                        let (_, dropped_payload) = ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                            if let Some((name, _item)) = item {
                                ui.heading(format!("{name}"));
                            } else {
                                ui.heading("...");
                            }
                        });
                        if let Some(selected_item) = dropped_payload {
                            self.item_view.select(*selected_item);
                        }
                    });
                    ui.separator();

                    if let Some((_name, item)) = item {
                        ui.horizontal(|ui| {
                            if ui.button("Verify").clicked() {
                                let result = item.pod.pod.verify();
                                // TODO: Verify commit on-chain via synchronizer
                                self.item_view.verify_result =
                                    Some(result.map_err(|e| anyhow!("{e}")));
                            }
                            ui.label(format!("{:?}", self.item_view.verify_result));
                        });
                        ui.heading("Statements:");
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let sts = &item.pod.public_statements;
                            ui.separator();
                            for st in sts {
                                ui.add(egui::Label::new(format!("{st}")).wrap());
                                ui.separator();
                            }
                        });
                    }
                });

                crafting_ui.vertical(|ui| {
                    ui.heading("Crafting");
                    ui.separator();
                    egui::ComboBox::from_label("")
                        .selected_text(selected_recipe.map(|r| r.to_string()).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for recipe in [Recipe::Copper, Recipe::Tin, Recipe::Bronze] {
                                ui.selectable_value(
                                    &mut selected_recipe,
                                    Some(recipe),
                                    recipe.to_string(),
                                );
                            }
                        });
                    if let Some(recipe) = self.crafting.selected_recipe {
                        ui.heading("Inputs:");
                        match recipe {
                            Recipe::Bronze => {
                                ui.horizontal(|ui| {
                                    ui.label("tin:");
                                    let (_, dropped_payload) =
                                        ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                                            if let Some(index) = self.crafting.input_items.get(&0) {
                                                ui.label(format!("{}", self.items[*index].0));
                                            } else {
                                                ui.label("...");
                                            }
                                        });
                                    if let Some(index) = dropped_payload {
                                        self.crafting.input_items.insert(0, *index);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("copper:");
                                    let (_, dropped_payload) =
                                        ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                                            if let Some(index) = self.crafting.input_items.get(&1) {
                                                ui.label(format!("{}", self.items[*index].0));
                                            } else {
                                                ui.label("...");
                                            }
                                        });
                                    if let Some(index) = dropped_payload {
                                        self.crafting.input_items.insert(1, *index);
                                    }
                                });
                            }
                            _ => {}
                        }
                        if ui.button("Craft").clicked() {
                            ui.label("todo");
                        }
                        if ui.button("Commit").clicked() {
                            ui.label("todo");
                        }
                    }
                });
            });

            if let Some(selected_recipe) = selected_recipe {
                self.crafting.select(selected_recipe);
            }
        });
    }
}
