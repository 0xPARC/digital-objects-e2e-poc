use std::{
    fs::{self},
    io,
    path::PathBuf,
};

use anyhow::{Result, anyhow};
use app::{Config, CraftedItem, load_item, log_init};
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

struct App {
    cfg: Config,
    items: Vec<(String, CraftedItem)>,
    item_view: ItemView,
    name: String,
    age: u32,
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
            name: "Foo".to_string(),
            age: 42,
        };
        app.refresh_items()?;
        Ok(app)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut selected_item = self.item_view.selected_item;
        egui::SidePanel::left("item list").show(ctx, |ui| {
            ui.heading("Item list");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (i, (name, _)) in self.items.iter().enumerate() {
                    ui.selectable_value(&mut selected_item, Some(i), name);
                }
            });
        });

        if let Some(selected_item) = selected_item {
            self.item_view.select(selected_item);
        }

        let item = self.item_view.selected_item.map(|i| &self.items[i]);
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some((name, item)) = item {
                ui.heading(format!("{name}"));
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let sts = &item.pod.public_statements;
                    ui.separator();
                    for st in sts {
                        ui.label(format!("{st}"));
                        ui.separator();
                    }
                });
                egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
                    if ui.button("Verify").clicked() {
                        let result = item.pod.pod.verify();
                        // TODO: Verify commit on-chain via synchronizer
                        self.item_view.verify_result = Some(result.map_err(|e| anyhow!("{e}")));
                    }
                    ui.label(format!("{:?}", self.item_view.verify_result));
                });
            }
            // ui.horizontal(|ui| {
            //     let name_label = ui.label("Your name: ");
            //     ui.text_edit_singleline(&mut self.name)
            //         .labelled_by(name_label.id);
            // });
            // ui.add(egui::Slider::new(&mut self.age, 0..=120).text("age"));
            // if ui.button("Increment").clicked() {
            //     self.age += 1;
            // }
            // ui.label(format!("Hello '{}', age {}", self.name, self.age));

            // ui.image(egui::include_image!("../assets/ferris.png"));
        });
    }
}
