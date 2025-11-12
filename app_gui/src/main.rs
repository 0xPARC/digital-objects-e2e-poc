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
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns_const(|[item_view_ui, crafting_ui]| {
                item_view_ui.vertical(|ui| {
                    self.update_item_view_ui(ui);
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

            self.ui_danger(ctx, ui);
        });

        // Shortcuts:
        // Alt + D: toggle 'dev_mode'
        if ctx.input(|i| i.key_released(egui::Key::D) && i.modifiers.alt) {
            self.dev_mode = !self.dev_mode;
            log::info!("dev_mode={:?}", self.dev_mode);
        }
        // Ctrl + Q: quit
        if ctx.input(|i| i.key_released(egui::Key::Q) && i.modifiers.ctrl) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Alt + S: danger
        if ctx.input(|i| i.key_released(egui::Key::S) && i.modifiers.alt) {
            self.danger = !self.danger;
        }
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

    pub fn name_with_img(&self, ui: &mut Ui, name: &String) {
        ui.horizontal(|ui| {
            ui.add(
                egui::Image::new(if name.starts_with("Axe") {
                    egui::include_image!("../assets/axe.png")
                } else if name.starts_with("Stick") {
                    egui::include_image!("../assets/stick.png")
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
                } else if name.starts_with("Tin") {
                    egui::include_image!("../assets/tin.png")
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
