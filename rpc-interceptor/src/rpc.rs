use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct RpcConfig {
    pub upstream_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub rpc: RpcConfig,
    pub http: Client,
}

// MVP: pass-through only, validates session presence via pairing state later
pub async fn rpc_entry(
    State(app): State<Arc<AppState>>,
    Path(_session_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
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


