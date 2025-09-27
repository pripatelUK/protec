use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use crate::pairing::PairingState;
use dashmap::DashMap;
use uuid::Uuid;
use std::time::{Duration, Instant};
use alloy_primitives::{keccak256, hex, TxKind};
use alloy_rlp::Decodable;
use alloy_consensus::TxEnvelope;

#[derive(Clone)]
pub struct RpcConfig {
    pub upstream_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub rpc: RpcConfig,
    pub http: Client,
    pub pairing: Arc<PairingState>,
    pub approvals_by_intent: DashMap<String, ApprovalRequest>, // (session_id|intent_key) -> approval
}

#[derive(Clone, Debug)]
pub enum ApprovalStatus { Pending, Approved, Denied, Expired }

#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    pub id: String,
    pub session_id: String,
    pub method: String,
    pub original_params: Value,
    pub created_at: Instant,
    pub expires_at: Instant,
    pub status: ApprovalStatus,
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

    // Method bucketing with intent key scaffolding
    let method = body.get("method").and_then(|v| v.as_str()).unwrap_or("");
    match method {
        // Preview trigger: create/refresh a transient approval, always forward upstream immediately
        "eth_estimateGas" => {
            let chain_id = fetch_chain_id(&app).await;
            let intent_key = chain_id
                .as_deref()
                .map(|cid| canonicalize_estimate_params(body.get("params"), cid))
                .unwrap_or_else(|| "unknown".to_string());

            let appr = ApprovalRequest {
                id: Uuid::new_v4().to_string(),
                session_id: session_id.clone(),
                method: method.to_string(),
                original_params: body.get("params").cloned().unwrap_or(Value::Null),
                created_at: Instant::now(),
                expires_at: Instant::now() + Duration::from_secs(120),
                status: ApprovalStatus::Pending,
            };
            let key = format!("{}|{}", session_id, intent_key);
            app.approvals_by_intent.insert(key, appr);
            // pass-through
        }
        // Final gate: require approved
        "eth_sendRawTransaction" => {
            let chain_id = fetch_chain_id(&app).await;
    let maybe_intent = chain_id
                .as_deref()
                .and_then(|cid| canonicalize_send_raw_params_alloy(body.get("params"), cid));

            // Try intent match first; fallback to session-level pending (coarse)
            let entry_opt = match maybe_intent {
                Some(intent_key) => {
                    let key = format!("{}|{}", session_id, intent_key);
                    app.approvals_by_intent.get(&key)
                }
                None => None,
            };

            // Normalize to owned ApprovalRequest to avoid Ref/RefMulti type mismatch
            let entry_owned: Option<ApprovalRequest> = if let Some(r) = entry_opt {
                Some(r.clone())
            } else {
                let mut found: Option<ApprovalRequest> = None;
                for kv in app.approvals_by_intent.iter() {
                    if kv.key().starts_with(&(session_id.clone() + "|")) {
                        found = Some(kv.value().clone());
                        break;
                    }
                }
                found
            };

            if let Some(entry) = entry_owned.as_ref() {
                let st = &entry.status;
                let now = Instant::now();
                let not_expired = now < entry.expires_at;
                match st {
                    ApprovalStatus::Pending if not_expired => {
                        // Return JSON-RPC error explaining approval required
                        let id_val = body.get("id").cloned().unwrap_or(Value::Null);
                        let resp = serde_json::json!({
                            "jsonrpc":"2.0",
                            "id": id_val,
                            "error": {"code": -32001, "message": "Mobile approval required", "data": {"approval_id": entry.id, "status": "Pending"}}
                        });
                        return (StatusCode::OK, Json(resp)).into_response();
                    }
                    ApprovalStatus::Denied if not_expired => {
                        let id_val = body.get("id").cloned().unwrap_or(Value::Null);
                        let resp = serde_json::json!({
                            "jsonrpc":"2.0",
                            "id": id_val,
                            "error": {"code": -32002, "message": "Approval denied", "data": {"approval_id": entry.id}}
                        });
                        return (StatusCode::OK, Json(resp)).into_response();
                    }
                    ApprovalStatus::Expired | ApprovalStatus::Pending | ApprovalStatus::Approved | ApprovalStatus::Denied => {
                        // If expired or approved/denied but expired, fall through; approved will pass-through
                    }
                }
            } else {
                // No approval exists; require it
                let id_val = body.get("id").cloned().unwrap_or(Value::Null);
                let resp = serde_json::json!({
                    "jsonrpc":"2.0",
                    "id": id_val,
                    "error": {"code": -32004, "message": "No approval found for session", "data": {"session_id": session_id}}
                });
                return (StatusCode::OK, Json(resp)).into_response();
            }
        }
        _ => {}
    }

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

async fn fetch_chain_id(app: &Arc<AppState>) -> Option<String> {
    let payload = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]
    });
    if let Ok(r) = app.http.post(&app.rpc.upstream_url).json(&payload).send().await {
        if let Ok(v) = r.json::<serde_json::Value>().await {
            return v.get("result").and_then(|x| x.as_str()).map(|s| s.to_string());
        }
    }
    None
}

fn canonicalize_estimate_params(params: Option<&Value>, chain_id_hex: &str) -> String {
    let cid = chain_id_hex.to_lowercase();
    let mut from="".to_string();
    let mut to="".to_string();
    let mut value="".to_string();
    let mut data="".to_string();
    if let Some(Value::Array(arr)) = params {
        if let Some(Value::Object(obj)) = arr.get(0) {
            from = obj.get("from").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            to = obj.get("to").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            value = obj.get("value").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            data = obj.get("data").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        }
    }
    format!("{}|{}|{}|{}|{}", cid, from, to, value, data)
}

fn canonicalize_send_raw_params_alloy(params: Option<&Value>, chain_id_hex: &str) -> Option<String> {
    let cid = chain_id_hex.to_lowercase();
    let raw = match params.and_then(|v| v.get(0)) {
        Some(Value::String(s)) => s.trim_start_matches("0x").to_string(),
        _ => return None,
    };
    let bytes = hex::decode(raw).ok()?;
    let mut slice: &[u8] = &bytes;
    if let Ok(env) = TxEnvelope::decode(&mut slice) {
        let (to, value, data) = match env {
            TxEnvelope::Legacy(signed) => {
                let t = signed.tx();
                let to_str = match t.to {
                    TxKind::Call(addr) => format!("{:?}", addr),
                    TxKind::Create => String::new(),
                };
                (to_str, format!("0x{:x}", t.value), format!("0x{}", hex::encode(&t.input)))
            }
            TxEnvelope::Eip2930(signed) => {
                let t = signed.tx();
                let to_str = match t.to {
                    TxKind::Call(addr) => format!("{:?}", addr),
                    TxKind::Create => String::new(),
                };
                (to_str, format!("0x{:x}", t.value), format!("0x{}", hex::encode(&t.input)))
            }
            TxEnvelope::Eip1559(signed) => {
                let t = signed.tx();
                let to_str = match t.to {
                    TxKind::Call(addr) => format!("{:?}", addr),
                    TxKind::Create => String::new(),
                };
                (to_str, format!("0x{:x}", t.value), format!("0x{}", hex::encode(&t.input)))
            }
            // For MVP, skip 4844 intent extraction
            _ => (String::new(), String::new(), String::new()),
        };
        let canonical = format!("{}|{}|{}|{}", cid, to.to_lowercase(), value.to_lowercase(), data.to_lowercase());
        let hash = keccak256(canonical.as_bytes());
        return Some(format!("0x{}", hex::encode(hash)));
    }
    None
}


