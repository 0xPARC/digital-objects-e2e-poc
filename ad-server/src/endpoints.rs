use std::sync::Arc;

use common::CustomError;
use uuid::Uuid;
use warp::Filter;

use crate::Context;

// HANDLERS:

// GET /sample/{req_id}, eg: `curl http://127.0.0.1:8000/sample/02f09a3f-1624-3b1d-8409-44eff7708208`
pub async fn handler_get_sample(
    req_id: Uuid,
    _ctx: Arc<Context>,
) -> Result<impl warp::Reply, warp::Rejection> {
    if req_id == Uuid::nil() {
        return Err(CustomError("sample error msg".to_string()).into());
    }
    Ok(warp::reply::json(&req_id))
}

// ROUTES:

// build the routes
pub fn routes(
    ctx: Arc<Context>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    get_sample(ctx.clone())
}
fn get_sample(
    ctx: Arc<Context>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    warp::path!("sample" / Uuid)
        .and(warp::get())
        .and(with_ctx(ctx))
        .and_then(handler_get_sample)
}

fn with_ctx(
    ctx: Arc<Context>,
) -> impl Filter<Extract = (Arc<Context>,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || ctx.clone())
}
