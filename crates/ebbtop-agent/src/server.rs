//! ebbtop P3a: the agent's HTTP surface, in the library so tests can serve it.
//!
//! Routes:
//! * `GET  /ws`       — the existing token-gated WebSocket (unchanged).
//! * `GET  /healthz`  — the existing unauthenticated liveness probe.
//! * `POST /pair`     — NEW: exchange a pairing code for one client token.

use axum::{
    Json, Router,
    extract::State,
    extract::rejection::JsonRejection,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};

use crate::pairing::{PAIR_PATH, PairError, PairRequest, PairResponse, now_ms};
use crate::state::AppState;
use crate::ws;

/// Build the router. `main` serves it with `axum_server`; tests serve it on
/// an ephemeral loopback port, so what the tests exercise is the production
/// route table.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws::ws_handler))
        .route("/healthz", get(healthz))
        .route(PAIR_PATH, post(pair_handler))
        .with_state(state)
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

/// HTTP status for a refused exchange. The messages differ so the operator
/// can tell "you never ran --pair" from "wrong digits", and none of them
/// contains the code or the expected value.
fn status_for(e: PairError) -> StatusCode {
    match e {
        PairError::NoPendingCode => StatusCode::NOT_FOUND,
        PairError::WrongCode | PairError::Expired => StatusCode::FORBIDDEN,
        PairError::TooManyAttempts => StatusCode::TOO_MANY_REQUESTS,
        PairError::StateIo => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `POST /pair` — one exchange, one issued token.
///
/// The code is read from the JSON body only. It is not logged, not echoed in
/// an error, and not accepted from the query string (a `?code=` request has
/// no body field and is refused as a bad request).
async fn pair_handler(
    State(state): State<AppState>,
    body: Result<Json<PairRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(req)) = body else {
        return no_store(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "bad pair request" }),
        );
    };
    // One exchange at a time: two concurrent requests must not both consume
    // the same pending code.
    let _guard = state.pair_lock.lock().await;
    match state
        .pairing
        .exchange(&req.code, req.label.as_deref(), now_ms())
    {
        Ok(rec) => no_store(
            StatusCode::OK,
            serde_json::json!(PairResponse {
                token: rec.token,
                host: state.hostname.clone(),
                agent_version: state.agent_version.clone(),
                nonce: Some(state.boot_nonce.clone()),
                scope: rec.scope,
            }),
        ),
        Err(e) => no_store(status_for(e), serde_json::json!({ "error": e.reason() })),
    }
}

/// JSON response that a proxy must never cache: the body can carry a token.
fn no_store(status: StatusCode, body: serde_json::Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_statuses_are_distinct() {
        assert_eq!(status_for(PairError::NoPendingCode), StatusCode::NOT_FOUND);
        assert_eq!(status_for(PairError::WrongCode), StatusCode::FORBIDDEN);
        assert_eq!(status_for(PairError::Expired), StatusCode::FORBIDDEN);
        assert_eq!(
            status_for(PairError::TooManyAttempts),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status_for(PairError::StateIo),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
