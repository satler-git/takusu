//! # takusu-worker — Cloudflare Worker (Rust/WASM)
//!
//! Storage + auth layer for the decoupled takusu architecture. Exposes a REST
//! API that mirrors the data subset of the takusu-local API: tasks, habits,
//! schedules, tokens, settings, Google Calendar settings/mappings. The local
//! server (`takusu-local`) is the only intended client.
//!
//! What lives here: D1 CRUD, SHA-256 token hashing, UUID v7 issuance.
//! What does NOT live here: scheduling (takusu-core), Google Calendar I/O
//! (google-cal), iCal parsing (takusu-ical) — those run in the native local
//! server.

#[cfg(target_arch = "wasm32")]
mod auth;
#[cfg(target_arch = "wasm32")]
mod error;
#[cfg(target_arch = "wasm32")]
mod handlers;
pub mod memory;
pub mod models;
#[cfg(target_arch = "wasm32")]
mod router;
#[cfg(target_arch = "wasm32")]
mod storage_d1;
#[cfg(target_arch = "wasm32")]
mod storage_d1_impl;
pub mod util;
pub mod validate;

#[cfg(target_arch = "wasm32")]
use std::sync::Once;
#[cfg(target_arch = "wasm32")]
use worker::{Context, Env, Request, Response};

#[cfg(target_arch = "wasm32")]
static INIT: Once = Once::new();

#[cfg(target_arch = "wasm32")]
fn init_logging(env: &Env) {
    INIT.call_once(|| {
        console_error_panic_hook::set_once();
        let level = env
            .var("TAKUSU_LOG")
            .ok()
            .and_then(|v| v.to_string().parse::<log::LevelFilter>().ok())
            .and_then(|f| f.to_level())
            .unwrap_or(log::Level::Info);
        wasm_logger::init(wasm_logger::Config::new(level));
    });
}

#[cfg(target_arch = "wasm32")]
#[worker::event(fetch)]
pub async fn fetch(req: Request, env: Env, _ctx: Context) -> worker::Result<Response> {
    init_logging(&env);
    router::handle(req, env).await
}
