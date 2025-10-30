use std::sync::Arc;

use hex::FromHex;
use pod2::middleware::{RawValue, Value};
use warp::Filter;

use crate::Node;

/// struct used to convert sqlx errors to warp errors
#[allow(dead_code)]
#[derive(Debug)]
pub struct CustomError(pub String);
impl warp::reject::Reject for CustomError {}

// HANDLERS:

// GET /created_item/{item}
pub(crate) async fn handler_get_created_item(
    item_str: String,
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let item = RawValue::from_hex(&item_str).map_err(|e| CustomError(e.to_string()))?;
    let state = node.state.read().unwrap();
    let mtp = state
        .created_items
        .prove(&Value::from(item))
        .map_err(|e| CustomError(e.to_string()))?;
    Ok(warp::reply::json(&(state.epoch, mtp)))
}

// GET /created_items
pub(crate) async fn handler_get_created_items(
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let state = node.state.read().unwrap();
    Ok(warp::reply::json(&state.created_items))
}

// GET /created_items_root
pub(crate) async fn handler_get_latest_created_items_root(
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let state = node.state.read().unwrap();
    Ok(warp::reply::json(&(
        state.epoch,
        state.created_items_roots.last().unwrap(),
    )))
}

// GET /created_items_root/{epoch}
pub(crate) async fn handler_get_created_items_root(
    epoch: u64,
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let state = node.state.read().unwrap();
    (state.epoch >= epoch)
        .then(|| warp::reply::json(&state.created_items_roots[epoch as usize]))
        .ok_or(CustomError(format!("Invalid epoch: {}", epoch)).into())
}

// GET /nullifier/{nullifier}
pub(crate) async fn handler_get_nullifier(
    nullifier_str: String,
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let nullifier = RawValue::from_hex(&nullifier_str).map_err(|e| CustomError(e.to_string()))?;
    let state = node.state.read().unwrap();
    let exists = state.nullifiers.contains(&nullifier);
    Ok(warp::reply::json(&exists))
}

// ROUTES:

// build the routes
pub(crate) fn routes(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    get_created_item(node.clone())
        .or(get_created_items(node.clone()))
        .or(get_latest_created_items_root(node.clone()))
        .or(get_created_items_root(node.clone()))
        .or(get_nullifier(node))
}

fn get_created_item(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let node_filter = warp::any().map(move || node.clone());

    warp::path!("created_item" / String)
        .and(warp::get())
        .and(node_filter)
        .and_then(handler_get_created_item)
}

fn get_created_items(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let node_filter = warp::any().map(move || node.clone());

    warp::path!("created_items")
        .and(warp::get())
        .and(node_filter)
        .and_then(handler_get_created_items)
}

fn get_latest_created_items_root(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let node_filter = warp::any().map(move || node.clone());

    warp::path!("created_items_root")
        .and(warp::get())
        .and(node_filter)
        .and_then(handler_get_latest_created_items_root)
}

fn get_created_items_root(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let node_filter = warp::any().map(move || node.clone());

    warp::path!("created_items_root" / u64)
        .and(warp::get())
        .and(node_filter)
        .and_then(handler_get_created_items_root)
}

fn get_nullifier(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let node_filter = warp::any().map(move || node.clone());

    warp::path!("nullifier" / String)
        .and(warp::get())
        .and(node_filter)
        .and_then(handler_get_nullifier)
}
