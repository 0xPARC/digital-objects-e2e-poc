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

use crate::{
    App, Item,
    utils::{pretty_st, result2text},
};

#[derive(Default)]
pub struct ItemView {
    pub selected_item: Option<usize>,
    pub verify_result: Option<Result<()>>,
}

impl ItemView {
    pub fn select(&mut self, index: usize) {
        if Some(index) != self.selected_item {
            self.selected_item = Some(index);
            self.verify_result = None;
        }
    }
}

impl App {
    // Item view panel
    pub fn update_item_view_ui(&mut self, ui: &mut Ui) {
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
                    self.name_with_img(ui, &item.name);
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

    pub fn verify_item(&self, item: &Item) -> Result<()> {
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
}
