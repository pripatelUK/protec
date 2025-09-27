use axum::{
    extract::{Path, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::TcpListener;
use tokio::{sync::broadcast, time::timeout};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let pairing_state = Arc::new(PairingState::new());

    let app = Router::new()
        .route("/health", get(health))
        // Phase 1 pairing APIs
        .route("/api/pairing/start", post(start_pairing))
        .route("/pairing/status/{pairing_code}", get(get_pairing_status))
        .route("/ws/pairing/{pairing_code}", get(ws_pairing))
        .with_state(pairing_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("listening on {}", addr);
    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "ok"
}

// ===== Pairing State & Types =====

#[derive(Clone)]
struct PairingState {
    pending: Arc<DashMap<String, PairingRequest>>, // pairing_code -> request
    // broadcast per pairing_code; we use a shared registry keyed by pairing_code
    channels: Arc<DashMap<String, broadcast::Sender<PairingEvent>>>,
}

impl PairingState {
    fn new() -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            channels: Arc::new(DashMap::new()),
        }
    }

    fn channel_for(&self, code: &str) -> broadcast::Sender<PairingEvent> {
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
enum PairingStatusKind {
    Pending,
    Paired,
    Expired,
}

#[derive(Debug, Clone)]
struct PairingRequest {
    pairing_code: String,
    session_id: String,
    created_at: Instant,
    expires_at: Instant,
    status: PairingStatusKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PairingEvent {
    Pending,
    Paired { session_id: String, rpc_endpoint: String },
    Expired,
}

#[derive(Debug, Serialize)]
struct StartPairingResponse {
    pairing_code: String,
    qr_data: String,
    session_id: String,
}

// ===== Handlers =====

async fn start_pairing(State(state): State<Arc<PairingState>>) -> impl IntoResponse {
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

    // Notify any listeners of pending
    let tx = state.channel_for(&pairing_code);
    let _ = tx.send(PairingEvent::Pending);

    // For QR we can embed just the pairing code; mobile app knows how to proceed
    let qr_data = format!("PAIR:{}", pairing_code);

    Json(StartPairingResponse {
        pairing_code,
        qr_data,
        session_id,
    })
}

#[derive(Debug, Serialize)]
struct PairingStatusResponse {
    status: PairingStatusKind,
    session_id: Option<String>,
    rpc_endpoint: Option<String>,
}

async fn get_pairing_status(
    State(state): State<Arc<PairingState>>,
    Path(pairing_code): Path<String>,
) -> impl IntoResponse {
    if let Some(req) = state.pending.get(&pairing_code) {
        let now = Instant::now();
        if now >= req.expires_at {
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
        Json(PairingStatusResponse {
            status: PairingStatusKind::Expired,
            session_id: None,
            rpc_endpoint: None,
        })
    }
}

async fn ws_pairing(
    State(state): State<Arc<PairingState>>,
    Path(pairing_code): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        use axum::extract::ws::Message;
        let mut ws = socket;

        let tx = state.channel_for(&pairing_code);
        let mut rx = tx.subscribe();

        // Immediately send current state if exists
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

        // Forward broadcast events to this websocket until it closes or timeout
        loop {
            let fut = rx.recv();
            match timeout(Duration::from_secs(180), fut).await {
                Ok(Ok(ev)) => {
                    if ws
                        .send(Message::Text(
                            serde_json::to_string(&ev).unwrap().into(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                _ => break,
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
    // 6-char base36 code
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
    // simple, non-crypto rng seeded by uuid
    let u = Uuid::new_v4();
    let b = u.as_bytes();
    let mut v: u64 = 0;
    for &x in b.iter().take(8) {
        v = (v << 8) ^ x as u64;
    }
    v
}
