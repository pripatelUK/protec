use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use crate::pairing::PairingState;

#[derive(Clone)]
pub struct RpcConfig {
    pub upstream_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub rpc: RpcConfig,
    pub http: Client,
    pub pairing: Arc<PairingState>,
}

// MVP: pass-through only, validates session presence via pairing state later
pub async fn rpc_entry(
    State(app): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // Session validation
    if !app.pairing.has_session(&session_id) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"invalid_session"})),
        )
            .into_response();
    }

    // Method bucketing MVP: pass-through for now; recognition only
    let method = body.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let _is_preview = matches!(method, "eth_estimateGas");
    let _is_final_gate = matches!(method, "eth_sendRawTransaction");

    let url = &app.rpc.upstream_url;
    let resp = app.http.post(url).json(&body).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            match r.text().await {
                Ok(text) => (status, text).into_response(),
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error":"upstream_read_failed","details":e.to_string()})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error":"upstream_unreachable","details":e.to_string()})),
        )
            .into_response(),
    }
}


