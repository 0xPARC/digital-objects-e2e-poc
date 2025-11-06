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
use app_cli::{Config, CraftedItem, Recipe, commit_item, craft_item, load_item, log_init};
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
    let app = Box::new(App::new(cfg, params)?);
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
    // app.task_handler
    //     .join()
    //     .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(())
}

fn _indent(w: &mut impl Write, indent: usize) {
    for _ in 0..indent {
        write!(w, "  ").unwrap();
    }
}

fn pretty_val(w: &mut impl Write, indent: usize, v: &Value) {
    match v.typed() {
        TypedValue::Raw(v) => write!(w, "{v:#}").unwrap(),
        TypedValue::Array(a) => {
            if a.array().is_empty() {
                write!(w, "[]").unwrap();
                return;
            }
            writeln!(w, "[").unwrap();
            _indent(w, indent + 1);
            for (i, v) in a.array().iter().enumerate() {
                if i > 0 {
                    writeln!(w, ",").unwrap();
                    _indent(w, indent + 1);
                }
                pretty_val(w, indent + 1, v);
            }
            writeln!(w).unwrap();
            _indent(w, indent);
            write!(w, "]").unwrap()
        }
        TypedValue::Set(s) => {
            let values: Vec<_> = s.set().iter().sorted_by_key(|k| k.raw()).collect();
            if values.is_empty() {
                write!(w, "#[]").unwrap();
                return;
            }
            writeln!(w, "#[").unwrap();
            _indent(w, indent + 1);
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    writeln!(w, ",").unwrap();
                    _indent(w, indent + 1);
                }
                pretty_val(w, indent + 1, v);
            }
            writeln!(w).unwrap();
            _indent(w, indent);
            write!(w, "]").unwrap()
        }
        TypedValue::Dictionary(d) => {
            let kvs: Vec<_> = d.kvs().iter().sorted_by_key(|(k, _)| k.name()).collect();
            if kvs.is_empty() {
                write!(w, "{{}}").unwrap();
                return;
            }
            writeln!(w, "{{").unwrap();
            _indent(w, indent + 1);
            for (i, (k, v)) in kvs.iter().enumerate() {
                if i > 0 {
                    writeln!(w, ",").unwrap();
                    _indent(w, indent + 1);
                }
                write!(w, "{k}: ").unwrap();
                pretty_val(w, indent + 1, v);
            }
            writeln!(w).unwrap();
            _indent(w, indent);
            write!(w, "}}").unwrap()
        }
        _ => write!(w, "{v}").unwrap(),
    }
}

fn pretty_arg(w: &mut impl Write, indent: usize, arg: &StatementArg) {
    match arg {
        StatementArg::None => write!(w, "  none").unwrap(),
        StatementArg::Literal(v) => pretty_val(w, indent, v),
        StatementArg::Key(ak) => write!(w, "  {ak}").unwrap(),
    }
}

fn pretty_st(w: &mut impl Write, st: &Statement) {
    writeln!(w, "{}(", st.predicate()).unwrap();
    _indent(w, 1);
    for (i, arg) in st.args().iter().enumerate() {
        if i != 0 {
            writeln!(w, ",").unwrap();
            _indent(w, 1);
        }
        pretty_arg(w, 1, arg);
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
    output_filename: String,
    craft_result: Option<Result<PathBuf>>,
    commit_result: Option<Result<PathBuf>>,
}

impl Crafting {
    fn select(&mut self, recipe: Recipe) {
        if Some(recipe) != self.selected_recipe {
            *self = Self::default();
            self.selected_recipe = Some(recipe);
        }
    }
}

#[derive(Default)]
struct Committing {
    result: Option<Result<()>>,
}

#[derive(Clone)]
struct Item {
    name: String,
    id: Hash,
    crafted_item: CraftedItem,
    path: PathBuf,
}

fn recipe_inputs(r: &Recipe) -> Vec<Recipe> {
    match r {
        Recipe::Bronze => vec![Recipe::Tin, Recipe::Copper],
        _ => vec![],
    }
}

fn recipe_statement(r: &Recipe) -> &'static str {
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

struct App {
    cfg: Config,
    params: Params,
    recipes: Vec<Recipe>,
    items: Vec<Item>,
    used_items: Vec<Item>,
    item_view: ItemView,
    crafting: Crafting,
    committing: Committing,
    task_req_tx: mpsc::Sender<Request>,
    task_res_rx: mpsc::Receiver<Response>,
    _task_handler: JoinHandle<()>,
    task_status: Arc<RwLock<TaskStatus>>,
}

#[derive(Default, Clone)]
struct TaskStatus {
    busy: Option<String>,
}

enum Request {
    Craft {
        params: Params,
        pods_path: String,
        recipe: Recipe,
        output: PathBuf,
        input_paths: Vec<PathBuf>,
    },
    Commit {
        params: Params,
        cfg: Config,
        input: PathBuf,
    },
    Exit,
}

enum Response {
    Craft(Result<PathBuf>),
    Commit(Result<PathBuf>),
    Null,
}

fn handle_req(task_status: &RwLock<TaskStatus>, req: Request) -> Response {
    fn set_busy_task(task_status: &RwLock<TaskStatus>, task: &str) {
        let mut task_status = task_status.write().unwrap();
        task_status.busy = Some(task.to_string());
    }
    match req {
        Request::Craft {
            params,
            pods_path,
            recipe,
            output,
            input_paths,
        } => {
            set_busy_task(task_status, "Crafting");

            let r = craft_item(&params, recipe, &output, &input_paths);

            // move the files of the used inputs into the `used` subdir
            let used_path = Path::new(&pods_path).join("used");
            for input in input_paths {
                let parent_path = input.parent().unwrap();
                // if original file is not in 'used' subdir, move it there, ignore if it already is
                // in that subdir
                if parent_path != used_path {
                    fs::rename(
                        input.clone(),
                        format!(
                            "{}/used/{}",
                            parent_path.display(),
                            input.file_name().unwrap().display()
                        ),
                    )
                    .unwrap();
                }
            }

            task_status.write().unwrap().busy = None;
            Response::Craft(r.map(|_| output))
        }
        Request::Commit { params, cfg, input } => {
            set_busy_task(task_status, "Committing");

            Runtime::new().unwrap();
            let rt = Runtime::new().unwrap();
            let r = rt.block_on(async { commit_item(&params, &cfg, &input).await });
            task_status.write().unwrap().busy = None;
            Response::Commit(r.map(|_| input))
        }
        Request::Exit => Response::Null,
    }
}

impl App {
    /// returns a vector with [self.items | self.used_items]
    fn all_items(&self) -> Vec<Item> {
        [self.items.clone(), self.used_items.clone()].concat()
    }
    fn load_item(&mut self, entry: &Path, used: bool) -> Result<()> {
        log::debug!("loading {entry:?}");
        let name = entry.file_name().unwrap().to_str().unwrap().to_string();
        let crafted_item = load_item(entry)?;
        let id = Hash::from(
            crafted_item.pod.public_statements[0].args()[0]
                .literal()
                .unwrap()
                .raw(),
        );
        let item = Item {
            name,
            id,
            crafted_item,
            path: entry.to_path_buf(),
        };
        if used {
            self.used_items.push(item);
        } else {
            self.items.push(item);
        }
        self.items.sort_by_key(|item| item.name.clone());
        self.used_items.sort_by_key(|item| item.name.clone());
        Ok(())
    }

    fn refresh_items(&mut self) -> Result<()> {
        // create 'pods_path' & 'pods_path/used' dir in case they do not exist
        fs::create_dir_all(format!("{}/used", &self.cfg.pods_path))?;

        self.items = Vec::new();
        self.used_items = Vec::new();
        log::info!("Loading items...");
        for entry in fs::read_dir(&self.cfg.pods_path)? {
            let entry = entry?;
            // skip dirs
            if !entry.file_type()?.is_dir() {
                self.load_item(&(entry.path()), false)?;
            }
        }

        log::info!("Loading used items...");
        for entry in fs::read_dir(format!("{}/used", &self.cfg.pods_path))? {
            let entry = entry?;
            // skip dirs
            if !entry.file_type()?.is_dir() {
                self.load_item(&(entry.path()), true)?;
            }
        }
        Ok(())
    }

    fn verify_item(&self, item: &Item) -> Result<()> {
        item.crafted_item.pod.pod.verify()?;

        // Verify that the item exists on-blob-space:
        // first get the merkle proof of item existence from the Synchronizer
        let item_id = RawValue::from(item.crafted_item.def.item_hash(&self.params)?);
        let item_hex: String = format!("{item_id:#}");
        let (epoch, _): (u64, RawValue) =
            reqwest::blocking::get(format!("{}/created_items_root", self.cfg.sync_url,))?.json()?;
        info!("Verifying commitment of item {item_id:#} via synchronizer at epoch {epoch}");
        let (epoch, mtp): (u64, MerkleProof) = reqwest::blocking::get(format!(
            "{}/created_item/{}",
            self.cfg.sync_url,
            &item_hex[2..]
        ))?
        .json()?;
        info!("mtp at epoch {epoch}: {mtp:?}");

        // fetch the associated Merkle root
        let merkle_root: RawValue = reqwest::blocking::get(format!(
            "{}/created_items_root/{}",
            self.cfg.sync_url, &epoch
        ))?
        .json()?;

        // verify the obtained merkle proof
        Set::verify(
            self.params.max_depth_mt_containers,
            merkle_root.into(),
            &mtp,
            &item_id.into(),
        )?;

        info!("Crafted item at {:?} successfully verified!", item.path);

        Ok(())
    }

    fn new(cfg: Config, params: Params) -> Result<Self> {
        let task_status = Arc::new(RwLock::new(TaskStatus::default()));
        let task_status_cloned = task_status.clone();
        let (req_tx, req_rx) = channel();
        let (res_tx, res_rx) = channel();
        let task_handler = thread::spawn(move || {
            let task_status = task_status_cloned;
            loop {
                match req_rx.recv() {
                    Ok(req) => {
                        if matches!(req, Request::Exit) {
                            return;
                        }
                        res_tx.send(handle_req(&task_status, req)).unwrap();
                    }
                    Err(e) => {
                        error!("channel error: {e}");
                        return;
                    }
                }
            }
        });
        let recipes = vec![Recipe::Copper, Recipe::Tin, Recipe::Bronze];
        let mut app = Self {
            cfg,
            params,
            recipes,
            items: vec![],
            used_items: vec![],
            item_view: Default::default(),
            crafting: Default::default(),
            committing: Default::default(),
            task_req_tx: req_tx,
            task_res_rx: res_rx,
            _task_handler: task_handler,
            task_status,
        };
        app.refresh_items()?;
        Ok(app)
    }
}

fn result2text<T: fmt::Debug, E: fmt::Debug>(r: &Option<Result<T, E>>) -> RichText {
    match r {
        None => RichText::new(""),
        Some(Err(e)) => RichText::new(format!("{e:?}"))
            .background_color(Color32::LIGHT_RED)
            .color(Color32::BLACK),
        Some(ok) => RichText::new(format!("{ok:?}"))
            .background_color(Color32::LIGHT_GREEN)
            .color(Color32::BLACK),
    }
}

impl App {
    // Item view panel
    fn update_item_view_ui(&mut self, ui: &mut Ui) {
        let item = self
            .item_view
            .selected_item
            .map(|i| self.all_items()[i].clone());
        egui::Grid::new("item title").show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.heading("Item: ");
            let frame = Frame::default().inner_margin(4.0);
            let (_, dropped_payload) = ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
                if let Some(item) = item.clone() {
                    ui.heading(item.name.to_string());
                } else {
                    ui.heading("...");
                }
            });
            ui.end_row();
            if let Some(selected_item) = dropped_payload {
                self.item_view.select(*selected_item);
            }
        });
        ui.separator();

        if let Some(item) = item {
            let mut verify_clicked = false;
            egui::ScrollArea::horizontal()
                .id_salt("item properties scroll")
                .show(ui, |ui| {
                    egui::Grid::new("item properties").show(ui, |ui| {
                        ui.label("path:");
                        ui.label(format!("{:?}", item.path));
                        ui.end_row();
                        ui.label("id:");
                        ui.label(RichText::new(format!("{:#}", item.id)).monospace());
                        ui.end_row();
                    });
                });
            egui::Grid::new("item buttons").show(ui, |ui| {
                if ui.button("Verify").clicked() {
                    verify_clicked = true;
                }
                ui.label(result2text(&self.item_view.verify_result));
            });
            ui.heading("Statements:");
            egui::ScrollArea::vertical().show(ui, |ui| {
                let sts = &item.crafted_item.pod.public_statements;
                ui.separator();
                for st in sts {
                    let mut st_str = String::new();
                    pretty_st(&mut st_str, st);
                    ui.add(Label::new(RichText::new(&st_str).monospace()).wrap());
                    ui.add_space(4.0);
                }
            });

            if verify_clicked {
                self.item_view.verify_result = Some(self.verify_item(&item));
            }
        }
    }

    // Crafting panel
    fn update_crafting_ui(&mut self, ui: &mut Ui) {
        let mut selected_recipe = self.crafting.selected_recipe;
        egui::Grid::new("crafting title").show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.heading("Crafting");
            ui.end_row();
        });
        ui.separator();
        egui::ComboBox::from_label("")
            .selected_text(selected_recipe.map(|r| r.to_string()).unwrap_or_default())
            .show_ui(ui, |ui| {
                for recipe in &self.recipes {
                    ui.selectable_value(&mut selected_recipe, Some(*recipe), recipe.to_string());
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
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.separator();
                let s = recipe_statement(&recipe);
                ui.add(Label::new(RichText::new(s).monospace()).wrap());
            });
        }
    }
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

        // If the task is busy, display a spinner and the task name
        let task_status = self.task_status.read().unwrap().clone();
        if let Some(task) = task_status.busy {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.heading(task);
                });
            });
            return;
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
            ui.heading("Used items");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, item) in self.used_items.iter().enumerate() {
                    ui.dnd_drag_source(
                        egui::Id::new(item.name.clone()),
                        self.items.len() + i,
                        |ui| {
                            ui.label(&item.name);
                        },
                    );
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns_const(|[item_view_ui, crafting_ui]| {
                item_view_ui.vertical(|ui| {
                    self.update_item_view_ui(ui);
                });

                crafting_ui.vertical(|ui| {
                    self.update_crafting_ui(ui);
                });
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&egui_glow::glow::Context>) {
        self.task_req_tx.send(Request::Exit).unwrap();
        // if the task is not busy it should terminate before 100 ms
        thread::sleep(time::Duration::from_millis(100));
    }
}
