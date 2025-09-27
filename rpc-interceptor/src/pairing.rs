use axum::{extract::{Path, State, WebSocketUpgrade}, response::IntoResponse, Json};
use axum::http::Method;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::{Duration, Instant}};
use tokio::{sync::broadcast, time::timeout};
use uuid::Uuid;

#[derive(Clone)]
pub struct PairingState {
    pub pending: Arc<DashMap<String, PairingRequest>>, // pairing_code -> request
    pub channels: Arc<DashMap<String, broadcast::Sender<PairingEvent>>>,
}

impl PairingState {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            channels: Arc::new(DashMap::new()),
        }
    }

    pub fn channel_for(&self, code: &str) -> broadcast::Sender<PairingEvent> {
        if let Some(ch) = self.channels.get(code) {
            return ch.clone();
        }
        let (tx, _rx) = broadcast::channel(16);
        self.channels.insert(code.to_string(), tx.clone());
        tx
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatusKind {
    Pending,
    Paired,
    Expired,
}

#[derive(Debug, Clone)]
pub struct PairingRequest {
    pub pairing_code: String,
    pub session_id: String,
    pub created_at: Instant,
    pub expires_at: Instant,
    pub status: PairingStatusKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairingEvent {
    Pending,
    Paired { session_id: String, rpc_endpoint: String },
    Expired,
}

#[derive(Debug, Serialize)]
pub struct StartPairingResponse {
    pub pairing_code: String,
    pub qr_data: String,
    pub session_id: String,
}

pub async fn start_pairing(State(state): State<Arc<PairingState>>) -> impl IntoResponse {
    let pairing_code = short_code();
    let session_id = Uuid::new_v4().to_string();

    let req = PairingRequest {
        pairing_code: pairing_code.clone(),
        session_id: session_id.clone(),
        created_at: Instant::now(),
        expires_at: Instant::now() + Duration::from_secs(120),
        status: PairingStatusKind::Pending,
    };
    state.pending.insert(pairing_code.clone(), req);

    let tx = state.channel_for(&pairing_code);
    let _ = tx.send(PairingEvent::Pending);

    println!(
        "[api] POST /api/pairing/start -> code={} session_id={}",
        pairing_code, session_id
    );

    let qr_data = format!("PAIR:{}", pairing_code);

    Json(StartPairingResponse {
        pairing_code,
        qr_data,
        session_id,
    })
}

#[derive(Debug, Serialize)]
pub struct PairingStatusResponse {
    pub status: PairingStatusKind,
    pub session_id: Option<String>,
    pub rpc_endpoint: Option<String>,
}

pub async fn get_pairing_status(
    State(state): State<Arc<PairingState>>,
    Path(pairing_code): Path<String>,
) -> impl IntoResponse {
    if let Some(req) = state.pending.get(&pairing_code) {
        let now = Instant::now();
        if now >= req.expires_at {
            println!(
                "[api] GET /pairing/status/{{{code}}} -> expired",
                code = pairing_code
            );
            let tx = state.channel_for(&pairing_code);
            let _ = tx.send(PairingEvent::Expired);
            drop(req);
            state.pending.remove(&pairing_code);
            return Json(PairingStatusResponse {
                status: PairingStatusKind::Expired,
                session_id: None,
                rpc_endpoint: None,
            });
        }
        match req.status {
            PairingStatusKind::Pending => Json(PairingStatusResponse {
                status: PairingStatusKind::Pending,
                session_id: None,
                rpc_endpoint: None,
            }),
            PairingStatusKind::Paired => Json(PairingStatusResponse {
                status: PairingStatusKind::Paired,
                session_id: Some(req.session_id.clone()),
                rpc_endpoint: Some(format!("http://localhost:3000/rpc/{}", req.session_id)),
            }),
            PairingStatusKind::Expired => Json(PairingStatusResponse {
                status: PairingStatusKind::Expired,
                session_id: None,
                rpc_endpoint: None,
            }),
        }
    } else {
        println!(
            "[api] GET /pairing/status/{{{code}}} -> not_found",
            code = pairing_code
        );
        Json(PairingStatusResponse {
            status: PairingStatusKind::Expired,
            session_id: None,
            rpc_endpoint: None,
        })
    }
}

pub async fn ws_pairing(
    State(state): State<Arc<PairingState>>,
    Path(pairing_code): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        use axum::extract::ws::Message;
        let mut ws = socket;

        let tx = state.channel_for(&pairing_code);
        let mut rx = tx.subscribe();

        println!(
            "[ws] /ws/pairing/{{{code}}} connected",
            code = pairing_code
        );

        if let Some(req) = state.pending.get(&pairing_code) {
            let _ = ws
                .send(Message::Text(
                    serde_json::to_string(&to_event(&req)).unwrap().into(),
                ))
                .await;
        } else {
            let _ = ws
                .send(Message::Text(
                    serde_json::to_string(&PairingEvent::Expired)
                        .unwrap()
                        .into(),
                ))
                .await;
        }

        loop {
            let fut = rx.recv();
            match timeout(Duration::from_secs(180), fut).await {
                Ok(Ok(ev)) => {
                    match &ev {
                        PairingEvent::Pending => println!(
                            "[ws] {{{code}}} -> pending",
                            code = pairing_code
                        ),
                        PairingEvent::Paired { .. } => println!(
                            "[ws] {{{code}}} -> paired",
                            code = pairing_code
                        ),
                        PairingEvent::Expired => println!(
                            "[ws] {{{code}}} -> expired",
                            code = pairing_code
                        ),
                    }
                    if ws
                        .send(Message::Text(
                            serde_json::to_string(&ev).unwrap().into(),
                        ))
                        .await
                        .is_err()
                    {
                        println!(
                            "[ws] /ws/pairing/{{{code}}} closed",
                            code = pairing_code
                        );
                        break;
                    }
                }
                _ => {
                    println!(
                        "[ws] /ws/pairing/{{{code}}} timeout",
                        code = pairing_code
                    );
                    break;
                }
            }
        }
    })
}

fn to_event(req: &PairingRequest) -> PairingEvent {
    match req.status {
        PairingStatusKind::Pending => PairingEvent::Pending,
        PairingStatusKind::Paired => PairingEvent::Paired {
            session_id: req.session_id.clone(),
            rpc_endpoint: format!("http://localhost:3000/rpc/{}", req.session_id),
        },
        PairingStatusKind::Expired => PairingEvent::Expired,
    }
}

fn short_code() -> String {
    let mut code = String::new();
    let mut n = rand_u64();
    for _ in 0..6 {
        let d = (n % 36) as u8;
        n /= 36;
        let ch = if d < 10 { (b'0' + d) as char } else { (b'a' + (d - 10)) as char };
        code.push(ch);
    }
    code
}

fn rand_u64() -> u64 {
    let u = Uuid::new_v4();
    let b = u.as_bytes();
    let mut v: u64 = 0;
    for &x in b.iter().take(8) {
        v = (v << 8) ^ x as u64;
    }
    v
}

#[derive(Debug, Deserialize)]
pub struct CompletePairRequest {
    pub pairing_code: String,
    pub device_id: String,
}

#[derive(Debug, Serialize)]
pub struct CompletePairResponse {
    pub ok: bool,
    pub session_id: String,
    pub rpc_endpoint: String,
}

pub async fn complete_pair(
    State(state): State<Arc<PairingState>>,
    Json(body): Json<CompletePairRequest>,
) -> impl IntoResponse {
    let code = body.pairing_code.clone();
    if let Some(req) = state.pending.get(&code) {
        if Instant::now() >= req.expires_at {
            let tx = state.channel_for(&code);
            let _ = tx.send(PairingEvent::Expired);
            drop(req);
            state.pending.remove(&code);
            println!(
                "[api] POST /api/pair code={code} -> expired",
                code = code
            );
            return (axum::http::StatusCode::GONE, Json(serde_json::json!({
                "ok": false,
                "error": "pairing_expired"
            }))).into_response();
        }

        let session_id = req.session_id.clone();
        drop(req);

        if let Some(mut entry) = state.pending.get_mut(&code) {
            entry.status = PairingStatusKind::Paired;
        } else {
            println!(
                "[api] POST /api/pair code={code} -> not_found_after_check",
                code = code
            );
            return (axum::http::StatusCode::NOT_FOUND, Json(serde_json::json!({
                "ok": false,
                "error": "pairing_not_found"
            }))).into_response();
        }

        let rpc_endpoint = format!("http://localhost:3000/rpc/{}", session_id);

        let tx = state.channel_for(&code);
        let _ = tx.send(PairingEvent::Paired { session_id: session_id.clone(), rpc_endpoint: rpc_endpoint.clone() });

        println!(
            "[api] POST /api/pair code={code} device_id={did} -> paired session_id={sid}",
            code = code,
            did = body.device_id,
            sid = session_id
        );

        return Json(CompletePairResponse { ok: true, session_id, rpc_endpoint }).into_response();
    }

    println!(
        "[api] POST /api/pair code={code} -> not_found",
        code = body.pairing_code
    );
    (axum::http::StatusCode::NOT_FOUND, Json(serde_json::json!({
        "ok": false,
        "error": "pairing_not_found"
    }))).into_response()
}

