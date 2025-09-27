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

#[tokio::main]
async fn main() {
    let pairing_state = Arc::new(PairingState::new());

    let app = Router::new()
        .route("/health", get(health))
        // Phase 1 pairing APIs
        .route("/api/pairing/start", post(start_pairing))
        .route("/pairing/status/{pairing_code}", get(get_pairing_status))
        .route("/ws/pairing/{pairing_code}", get(ws_pairing))
        .route("/api/pair", post(complete_pair))
        .with_state(pairing_state)
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
