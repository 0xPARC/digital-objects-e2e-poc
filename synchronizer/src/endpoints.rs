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
    let created_items = node.created_items.read().unwrap();
    let mtp = created_items
        .prove(&Value::from(item))
        .map_err(|e| CustomError(e.to_string()))?;
    drop(created_items);
    Ok(warp::reply::json(&mtp))
}

// GET /created_items
pub(crate) async fn handler_get_created_items(
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let created_items = node.created_items.read().unwrap().clone();
    Ok(warp::reply::json(&created_items))
}

// GET /nullifier/{nullifier}
pub(crate) async fn handler_get_nullifier(
    nullifier_str: String,
    node: Arc<Node>,
) -> Result<impl warp::Reply, warp::Rejection> {
    let nullifier = RawValue::from_hex(&nullifier_str).map_err(|e| CustomError(e.to_string()))?;
    let nullifiers = node.nullifiers.read().unwrap();
    let exists = nullifiers.contains(&nullifier);
    drop(nullifiers);
    Ok(warp::reply::json(&exists))
}

// ROUTES:

// build the routes
pub(crate) fn routes(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    get_created_item(node.clone())
        .or(get_created_items(node.clone()))
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

fn get_nullifier(
    node: Arc<Node>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let node_filter = warp::any().map(move || node.clone());

    warp::path!("nullifier" / String)
        .and(warp::get())
        .and(node_filter)
        .and_then(handler_get_nullifier)
}
