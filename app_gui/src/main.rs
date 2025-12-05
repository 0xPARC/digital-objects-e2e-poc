use std::{collections::HashMap, thread, time};

use anyhow::Result;
use app_cli::Config;
use common::{load_dotenv, log_init};
use egui::Ui;
use pod2::middleware::Params;
use tracing::info;

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

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process task response messages
        if let Ok(res) = self.task_res_rx.try_recv() {
            match res {
                Response::Craft(r) => {
                    if let Ok(entry) = &r {
                        self.load_item(entry, false).unwrap();
                    } else {
                        log::error!("{r:?}");
                    }
                    self.refresh_items().unwrap();
                    self.crafting.input_items = HashMap::new();
                    self.crafting.craft_result = Some(r);
                    self.crafting.commit_result = None;
                }
                Response::Commit(r) => {
                    if let Err(e) = &r {
                        log::error!("{e:?}");
                    }
                    // Reset filename
                    self.crafting.output_filename = "".to_string();
                    self.crafting.commit_result = Some(r);
                }
                Response::CraftAndCommit(r) => {
                    if let Ok(entry) = &r {
                        self.load_item(entry, false).unwrap();
                    } else {
                        log::error!("{r:?}");
                    }
                    self.refresh_items().unwrap();
                    self.crafting.input_items = HashMap::new();
                    // Reset filename
                    self.crafting.output_filename = "".to_string();
                    self.crafting.craft_result = None;
                    self.crafting.commit_result = Some(r);
                }
                Response::Null => {}
            }
        }

        // Left side panel "Item list"
        egui::SidePanel::left("item list").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("My Objects");
                if ui.button("Refresh").clicked() {
                    self.refresh_items().unwrap();
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, item) in self.items.clone().iter().enumerate() {
                    ui.dnd_drag_source(egui::Id::new(item.name.clone()), i, |ui| {
                        self.name_with_img(ui, &item.name);
                    });
                }
            });
            if !self.used_items.is_empty() {
                ui.separator();
                egui::Grid::new("used items title").show(ui, |ui| {
                    ui.collapsing("Consumed objects", |ui| {
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
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns_const(|[item_view_ui, crafting_ui]| {
                item_view_ui.vertical(|ui| {
                    self.update_item_view_ui(ctx, ui);
                });
                crafting_ui.vertical(|ui| {
                    let task_status = self.task_status.read().unwrap().clone();
                    // If the task is busy, display a spinner and the task name,
                    // else display the action UI.
                    if let Some(task) = task_status.busy {
                        ui.horizontal_centered(|ui| {
                            ui.spinner();
                            ui.heading(task);
                        });
                    } else {
                        self.update_action_ui(ctx, ui);
                    }
                });
            });
            // Display window(s).
            if self.modal_new_predicates {
                self.ui_new_predicate(ctx);
            }

            [
                (self.danger, egui::include_image!("../assets/water.png")),
                (self.cute, egui::include_image!("../assets/eucalyptus.png")),
            ]
            .into_iter()
            .for_each(|(flag, asset)| self.ui_cursor(ctx, ui, flag, asset));
        });

        // Shortcuts:
        // Alt + D: toggle 'dev_mode'
        if ctx.input(|i| i.key_released(egui::Key::D) && i.modifiers.alt) {
            self.dev_mode = !self.dev_mode;
            log::info!("dev_mode={:?}", self.dev_mode);
        }

        // Alt + T: toggle theme
        if ctx.input(|i| i.key_released(egui::Key::T) && i.modifiers.alt) {
            let theme = ctx.theme();
            log::info!("Switching from {theme:?} theme");
            ctx.set_theme(match theme {
                egui::Theme::Dark => egui::Theme::Light,
                egui::Theme::Light => egui::Theme::Dark,
            });
        }

        // Alt + M: toggle mock mode
        if ctx.input(|i| i.key_released(egui::Key::M) && i.modifiers.alt) {
            self.mock_mode = !self.mock_mode;
            log::info!("mock_mode={:?}", self.mock_mode);
        }

        // Ctrl + Q: quit
        if ctx.input(|i| i.key_released(egui::Key::Q) && i.modifiers.ctrl) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Alt + S: shark
        if ctx.input(|i| i.key_released(egui::Key::S) && i.modifiers.alt) {
            self.cute = false;
            self.danger = !self.danger;
        }
        // Alt + K: cuteness
        if ctx.input(|i| i.key_released(egui::Key::K) && i.modifiers.alt) {
            self.danger = false;
            self.cute = !self.cute;
        }
        // ALT + ?
        if ctx.input(|i| i.key_released(egui::Key::Questionmark) && i.modifiers.alt) {
            self.show_cheats = !self.show_cheats;
            log::info!("show_cheats={:?}", self.show_cheats);
        }

        egui::Window::new("Cheat codes")
            .collapsible(true)
            .movable(true)
            .resizable([true, true])
            .title_bar(true)
            .open(&mut self.show_cheats)
            .show(ctx, |ui| {
                ui.label("ALT + M: toggle mock mode".to_string());
                ui.label("ALT + S: danger mode".to_string());
                ui.label("ALT + K: cute mode".to_string());
                ui.label("ALT + ?: show cheat codes".to_string());
                ui.label("CTRL + Q: quit".to_string());
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
                ui.set_min_height(32.0);
                // Mock toggle taken into account.
                for verb in Verb::list()
                    .into_iter()
                    .filter(|v| self.mock_mode || v == &Verb::Gather || v == &Verb::Craft)
                {
                    if ui
                        .selectable_label(Some(verb) == self.crafting.selected_verb, verb.as_str())
                        .clicked()
                    {
                        self.crafting.selected_verb = Some(verb);
                        self.crafting.selected_process = verb.default_process();
                    }
                }
                // Mock toggle for 'new predicate'.
                if self.mock_mode
                    && ui
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

    pub fn name_with_img(&self, ui: &mut Ui, name: &String) {
        ui.horizontal(|ui| {
            ui.add(
                egui::Image::new(if name.starts_with("Axe") {
                    egui::include_image!("../assets/axe.png")
                } else if name.starts_with("Stone") {
                    egui::include_image!("../assets/stone.png")
                } else if name.starts_with("WoodenAxe") {
                    egui::include_image!("../assets/wooden-axe.png")
                } else if name.starts_with("Bronze") {
                    egui::include_image!("../assets/bronze.png")
                } else if name.starts_with("Wood") {
                    egui::include_image!("../assets/wood.png")
                } else if name.starts_with("Copper") {
                    egui::include_image!("../assets/copper.png")
                } else if name.starts_with("Coal") {
                    egui::include_image!("../assets/coal.png")
                } else if name.starts_with("Tin") {
                    egui::include_image!("../assets/tin.png")
                } else if name.starts_with("Tree House") {
                    egui::include_image!("../assets/tree-house.png")
                } else if name.starts_with("Refined Uranium") {
                    egui::include_image!("../assets/uranium.png")
                } else if name.starts_with("Tomato") {
                    egui::include_image!("../assets/tomato.png")
                } else if name.starts_with("Steel Sword") {
                    egui::include_image!("../assets/steel-sword.png")
                } else if name.starts_with("Dust") {
                    egui::include_image!("../assets/dust.png")
                } else if name.starts_with("Gem") {
                    egui::include_image!("../assets/gem.png")
                } else {
                    egui::include_image!("../assets/empty.png")
                })
                .max_width(18.0),
            );
            if self.dev_mode {
                ui.label(name);
            } else {
                ui.label(strip_suffix(name));
            }
        });
    }
}

fn strip_suffix(s: &str) -> &str {
    if let Some(pos) = s.rfind('_') {
        &s[..pos]
    } else {
        s
    }
}
