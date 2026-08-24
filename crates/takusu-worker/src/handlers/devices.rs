use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_ok, parse_json};
use takusu_contracts::{
    CreateDevice, RefreshEvaluatorHeartbeat, RefreshEvaluatorLease, Storage, UpdateDevice, Validate,
};

pub async fn create(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateDevice = parse_json(&mut req).await?;
    body.validate()?;
    let store = storage(&env)?;
    let row = store.register_device(&body).await?;
    json_ok(&row)
}

pub async fn list(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_devices().await?;
    json_ok(&rows)
}

pub async fn get(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let row = store.get_device(id).await?;
    json_ok(&row)
}

pub async fn update(mut req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateDevice = parse_json(&mut req).await?;
    body.validate()?;
    let store = storage(&env)?;
    let row = store.update_device(id, &body).await?;
    json_ok(&row)
}

pub async fn delete(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    store.delete_device(id).await?;
    Response::empty().map_err(WorkerError::from)
}

pub async fn heartbeat(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: RefreshEvaluatorHeartbeat = parse_json(&mut req).await?;
    if id != body.device_id {
        return Err(WorkerError::BadRequest(format!(
            "path device id {id} does not match body device_id {}",
            body.device_id
        )));
    }
    let store = storage(&env)?;
    let row = store
        .refresh_evaluator_heartbeat(&body.device_id, body.until)
        .await?;
    json_ok(&row)
}

pub async fn lease(mut req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: RefreshEvaluatorLease = parse_json(&mut req).await?;
    if id != body.device_id {
        return Err(WorkerError::BadRequest(format!(
            "path device id {id} does not match body device_id {}",
            body.device_id
        )));
    }
    let store = storage(&env)?;
    let row = store
        .refresh_evaluator_lease(&body.device_id, body.lease_until, body.next_eval_at)
        .await?;
    json_ok(&row)
}

pub async fn resident(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let authority = store.resolve_resident_authority(id).await?;
    json_ok(&authority)
}

pub async fn speech(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let row = store.get_device(id).await?;
    // Desktop is treated as always physically able to speak proactively.
    // On Android the physical ability is whether the audio service is running;
    // the privacy/private-output gate is a separate layer applied by the client.
    let can_speak_proactively = matches!(row.platform, takusu_contracts::DevicePlatform::Desktop)
        || row.audio_service_running;
    let capability = takusu_contracts::SpeechCapability {
        can_speak_proactively,
    };
    json_ok(&capability)
}
