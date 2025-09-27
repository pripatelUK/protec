use axum::{routing::{get, post}, Router};
use axum::http::Method;
use std::{
    env,
    net::SocketAddr,
    sync::Arc,
};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
mod pairing;
use pairing::{
    complete_pair,
    get_pairing_status,
    start_pairing,
    ws_pairing,
    PairingState,
};
mod rpc;
use rpc::{rpc_entry, approve, AppState as RpcAppState, RpcConfig};
mod passkeys;
use passkeys::{
    PasskeyState,
    register_start,
    register_finish,
    assert_start,
    assert_finish,
    resume_session,
    ResumeState,
};

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let pairing_state = Arc::new(PairingState::new());
    let rp_id = std::env::var("RP_ID").unwrap_or_else(|_| "localhost".to_string());
    let hmac_secret = std::env::var("PASSKEY_HMAC_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    let origins = std::env::var("RP_ORIGINS").unwrap_or_else(|_| "http://localhost:3000".to_string());
    let origin_list: Vec<String> = origins.split(',').map(|s| s.trim().to_string()).collect();
    let passkey_state = Arc::new(PasskeyState::new(rp_id, hmac_secret, origin_list));
    let upstream_url = std::env::var("RPC_PROVIDER_URL").unwrap_or_else(|_| "https://eth.llamarpc.com".to_string());
    println!("RPC upstream_url={}", upstream_url);
    let rpc_state = Arc::new(RpcAppState {
        rpc: RpcConfig { upstream_url },
        http: reqwest::Client::new(),
        pairing: pairing_state.clone(),
        approvals_by_intent: dashmap::DashMap::new(),
    });

    let pairing_router = Router::new()
        .route("/health", get(health))
        .route("/api/pairing/start", post(start_pairing))
        .route("/pairing/status/{pairing_code}", get(get_pairing_status))
        .route("/ws/pairing/{pairing_code}", get(ws_pairing))
        .route("/api/pair", post(complete_pair))
        .with_state(pairing_state.clone());

    let rpc_router = Router::new()
        .route("/rpc/{session_id}", post(rpc_entry))
        .route("/api/approve/{approval_id}", post(approve))
        .with_state(rpc_state.clone());

    let passkeys_router = Router::new()
        .route("/api/passkeys/register/start", post(register_start))
        .route("/api/passkeys/register/finish", post(register_finish))
        .route("/api/passkeys/assert/start", post(assert_start))
        .route("/api/passkeys/assert/finish", post(assert_finish))
        .with_state(passkey_state.clone());

    let resume_router = Router::new()
        .route("/api/sessions/resume", post(resume_session))
        .with_state(Arc::new(ResumeState { passkeys: passkey_state.clone(), pairing: pairing_state.clone() }));

    let app = pairing_router
        .merge(rpc_router)
        .merge(passkeys_router)
        .merge(resume_router)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers(Any),
        );

    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);
    let addr: SocketAddr = format!("{}:{}", host, port).parse().expect("invalid HOST/PORT");
    println!("listening on {}", addr);
    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "ok"
}
