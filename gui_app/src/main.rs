use std::{
    collections::HashMap,
    fmt::Write,
    fs::{self},
    io,
    path::PathBuf,
};

use anyhow::{Result, anyhow};
use app::{Config, CraftedItem, Recipe, load_item, log_init};
use common::load_dotenv;
use eframe::egui;
use itertools::Itertools;
use pod2::middleware::{Hash, Statement, StatementArg, TypedValue, Value};
use tracing::info;

fn main() -> Result<()> {
    log_init();
    load_dotenv()?;
    let cfg = Config::from_env()?;
    info!(?cfg, "Loaded config");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_maximized(true),
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

fn _indent(w: &mut impl Write, indent: usize) {
    for _ in 0..indent {
        write!(w, "  ").unwrap();
    }
}

fn _pretty_val(w: &mut impl Write, indent: usize, v: &Value) {
    match v.typed() {
        TypedValue::Raw(v) => write!(w, "{:#}", v).unwrap(),
        TypedValue::Array(a) => {
            if a.array().is_empty() {
                write!(w, "[]").unwrap();
                return;
            }
            write!(w, "[\n").unwrap();
            _indent(w, indent + 1);
            for (i, v) in a.array().iter().enumerate() {
                if i > 0 {
                    write!(w, ",\n").unwrap();
                    _indent(w, indent + 1);
                }
                _pretty_val(w, indent + 1, v);
            }
            write!(w, "\n").unwrap();
            _indent(w, indent);
            write!(w, "]").unwrap()
        }
        TypedValue::Set(s) => {
            let values: Vec<_> = s.set().iter().sorted_by_key(|k| k.raw()).collect();
            if values.is_empty() {
                write!(w, "#[]").unwrap();
                return;
            }
            write!(w, "#[\n").unwrap();
            _indent(w, indent + 1);
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    write!(w, ",\n").unwrap();
                    _indent(w, indent + 1);
                }
                _pretty_val(w, indent + 1, v);
            }
            write!(w, "\n").unwrap();
            _indent(w, indent);
            write!(w, "]").unwrap()
        }
        TypedValue::Dictionary(d) => {
            let kvs: Vec<_> = d.kvs().iter().sorted_by_key(|(k, _)| k.name()).collect();
            if kvs.is_empty() {
                write!(w, "{{}}").unwrap();
                return;
            }
            write!(w, "{{\n").unwrap();
            _indent(w, indent + 1);
            for (i, (k, v)) in kvs.iter().enumerate() {
                if i > 0 {
                    write!(w, ",\n").unwrap();
                    _indent(w, indent + 1);
                }
                write!(w, "{}: ", k).unwrap();
                _pretty_val(w, indent + 1, v);
            }
            write!(w, "\n").unwrap();
            _indent(w, indent);
            write!(w, "}}").unwrap()
        }
        _ => write!(w, "{}", v).unwrap(),
    }
}

fn _pretty_arg(w: &mut impl Write, indent: usize, arg: &StatementArg) {
    match arg {
        StatementArg::None => write!(w, "  none").unwrap(),
        StatementArg::Literal(v) => _pretty_val(w, indent, v),
        StatementArg::Key(ak) => write!(w, "  {}", ak).unwrap(),
    }
}

fn _pretty_st(w: &mut impl Write, st: &Statement) {
    write!(w, "{}(\n", st.predicate()).unwrap();
    _indent(w, 1);
    for (i, arg) in st.args().iter().enumerate() {
        if i != 0 {
            write!(w, ",\n").unwrap();
            _indent(w, 1);
        }
        _pretty_arg(w, 1, arg);
    }
    write!(w, "\n)").unwrap();
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

struct Item {
    name: String,
    id: Hash,
    crafted_item: CraftedItem,
    path: PathBuf,
}

struct App {
    cfg: Config,
    items: Vec<Item>,
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
            let crafted_item = load_item(&entry)?;
            let id = Hash::from(
                crafted_item.pod.public_statements[0].args()[0]
                    .literal()
                    .unwrap()
                    .raw(),
            );
            items.push(Item {
                name,
                id,
                crafted_item: crafted_item,
                path: entry,
            });
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
                for (i, item) in self.items.iter().enumerate() {
                    ui.dnd_drag_source(egui::Id::new(item.name.clone()), i, |ui| {
                        ui.label(&item.name);
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
                            if let Some(item) = item {
                                ui.heading(format!("{}", item.name));
                            } else {
                                ui.heading("...");
                            }
                        });
                        if let Some(selected_item) = dropped_payload {
                            self.item_view.select(*selected_item);
                        }
                    });
                    ui.separator();

                    if let Some(item) = item {
                        egui::ScrollArea::horizontal()
                            .id_salt("item properties scroll")
                            .show(ui, |ui| {
                                egui::Grid::new("item properties").show(ui, |ui| {
                                    ui.label("path:");
                                    ui.label(format!("{:?}", item.path));
                                    ui.end_row();
                                    ui.label("id:");
                                    ui.label(format!("{:#}", item.id));
                                    ui.end_row();
                                });
                            });
                        ui.horizontal(|ui| {
                            if ui.button("Verify").clicked() {
                                let result = item.crafted_item.pod.pod.verify();
                                // TODO: Verify commit on-chain via synchronizer
                                self.item_view.verify_result =
                                    Some(result.map_err(|e| anyhow!("{e}")));
                            }
                            ui.label(format!("{:?}", self.item_view.verify_result));
                        });
                        ui.heading("Statements:");
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let sts = &item.crafted_item.pod.public_statements;
                            ui.separator();
                            for st in sts {
                                let mut st_str = String::new();
                                _pretty_st(&mut st_str, st);
                                ui.add(
                                    egui::Label::new(egui::RichText::new(&st_str).monospace())
                                        .wrap(),
                                );
                                ui.add_space(4.0);
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
                        let inputs = match recipe {
                            Recipe::Bronze => vec!["tin", "copper"],
                            _ => vec![],
                        };
                        egui::Grid::new("crafting inputs").show(ui, |ui| {
                            for (input_index, input) in inputs.iter().enumerate() {
                                ui.label(format!("{input}:"));
                                let (_, dropped_payload) =
                                    ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                                        if let Some(index) =
                                            self.crafting.input_items.get(&input_index)
                                        {
                                            ui.label(format!("{}", self.items[*index].name));
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
