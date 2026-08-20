use worker::{Env, Request, Response, ResponseBuilder};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_ok, parse_json};
use takusu_contracts::{
    EvaluationInputs, EventDeliveryState, EventLedgerInsert, ScheduleRevisionResponse, Storage,
};

#[derive(Debug, serde::Deserialize)]
pub struct EvaluateEventsRequest {
    #[serde(default)]
    pub device_id: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ClaimEventRequest {
    pub device_id: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ClaimEventResponse {
    pub claimed: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct CommitEventsRequest {
    pub schedule_revision: i64,
    pub events: Vec<EventLedgerInsert>,
}

pub async fn revision(_req: Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    json_ok(&ScheduleRevisionResponse {
        revision: store.get_schedule_revision().await?,
    })
}

pub async fn insert(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let event: EventLedgerInsert = parse_json(&mut req).await?;
    let store = storage(&env)?;
    let row = store.insert_event_ledger(&event).await?;
    json_ok(&row)
}

pub async fn commit(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: CommitEventsRequest = parse_json(&mut req).await?;
    let store = storage(&env)?;
    store
        .commit_event_evaluation(body.schedule_revision, &body.events)
        .await?;
    Ok(Response::empty()?)
}

pub async fn list(req: Request, env: Env) -> Result<Response, WorkerError> {
    let device_id = req
        .url()?
        .query_pairs()
        .find(|(key, _)| key == "device_id")
        .map(|(_, value)| value.into_owned());
    let store = storage(&env)?;
    let events = store.list_event_ledger(device_id.as_deref()).await?;
    json_ok(&events)
}

pub async fn evaluate(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let _body: EvaluateEventsRequest = parse_json(&mut req).await?;
    let _ = storage(&env)?;
    // The worker does not run the planner evaluator. The local resident host is
    // the authority for planner policy; the worker's role is to return a
    // consistent snapshot via GET /events/snapshot for that host. Return a
    // clear 501 instead of the previous empty-success stub.
    let body = serde_json::json!({
        "message": "Worker-side event evaluation is not implemented; the local resident host is the authority for planner policy.",
    });
    let body = body.to_string();
    let mut resp = ResponseBuilder::new()
        .with_status(501)
        .ok(body)
        .map_err(WorkerError::Worker)?;
    resp.headers_mut()
        .set("content-type", "application/json")
        .map_err(WorkerError::Worker)?;
    Ok(resp)
}

pub async fn snapshot(_req: Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let inputs: EvaluationInputs = store.get_evaluation_inputs().await?;
    json_ok(&inputs)
}

pub async fn claim(mut req: Request, env: Env, event_id: &str) -> Result<Response, WorkerError> {
    let body: ClaimEventRequest = parse_json(&mut req).await?;
    let store = storage(&env)?;
    json_ok(&ClaimEventResponse {
        claimed: store
            .claim_event_delivery(&body.device_id, event_id)
            .await?,
    })
}

pub async fn acknowledge(_req: Request, env: Env, event_id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let event = store
        .update_event_delivery_state(event_id, EventDeliveryState::Acknowledged)
        .await?;
    json_ok(&event)
}

pub async fn update_state(
    mut req: Request,
    env: Env,
    event_id: &str,
) -> Result<Response, WorkerError> {
    let state: EventDeliveryState = parse_json(&mut req).await?;
    let store = storage(&env)?;
    let event = store.update_event_delivery_state(event_id, state).await?;
    json_ok(&event)
}
