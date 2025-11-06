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

pub(crate) fn pretty_st(w: &mut impl Write, st: &Statement) {
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

pub(crate) fn result2text<T: fmt::Debug, E: fmt::Debug>(r: &Option<Result<T, E>>) -> RichText {
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
