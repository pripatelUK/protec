use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use dashmap::DashMap;
use uuid::Uuid;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use webauthn_rs::prelude::*;
use url::Url;

#[derive(Clone)]
pub struct PasskeyState {
    pub passkeys_by_device: Arc<DashMap<String, Vec<Passkey>>>, // device_id -> passkeys
    pub challenges: Arc<DashMap<String, PendingChallenge>>,     // challenge -> pending state
    pub hmac_secret: Arc<String>,
    pub rp_id: Arc<String>,
    pub webauthn: Arc<Webauthn>,
}

impl PasskeyState {
    pub fn new(rp_id: String, hmac_secret: String, origins: Vec<String>) -> Self {
        // Primary origin is first element or default http://localhost:3000
        let mut origin_iter = origins
            .into_iter()
            .filter_map(|s| Url::parse(&s).ok());
        let primary = origin_iter.next().unwrap_or_else(|| Url::parse("http://localhost:3000").unwrap());

        let mut builder = WebauthnBuilder::new(&rp_id, &primary).expect("valid webauthn config");
        builder = builder.rp_name("Protec").allow_any_port(true);
        for extra in origin_iter {
            builder = builder.append_allowed_origin(&extra);
        }
        let webauthn = builder.build().expect("build webauthn");
        Self {
            passkeys_by_device: Arc::new(DashMap::new()),
            challenges: Arc::new(DashMap::new()),
            hmac_secret: Arc::new(hmac_secret),
            rp_id: Arc::new(rp_id),
            webauthn: Arc::new(webauthn),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub device_id: String,
    pub created_at: Instant,
    pub kind: ChallengeKind,
    pub expires_at: Instant,
    pub challenge: String,
    pub reg_state: Option<PasskeyRegistration>,
    pub auth_state: Option<PasskeyAuthentication>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind { Register, Assert }

// -------- Register Start --------

#[derive(Debug, Deserialize)]
pub struct RegisterStartReq { pub device_id: String, pub display_name: String }

#[derive(Debug, Serialize)]
pub struct PublicKeyCredentialCreationOptions {
    pub challenge: String,
    pub rp: HashMap<&'static str, String>,
    pub user: HashMap<&'static str, String>,
    pub pubKeyCredParams: Vec<HashMap<&'static str, serde_json::Value>>, // minimal for MVP
    pub timeout: u64,
    pub attestation: &'static str,
}

pub async fn register_start(State(state): State<Arc<PasskeyState>>, Json(body): Json<RegisterStartReq>) -> impl IntoResponse {
    let user_unique_id = Uuid::new_v4();
    let (ccr, reg_state) = state.webauthn
        .start_passkey_registration(
            user_unique_id,
            &body.device_id,
            &body.display_name,
            None,
        )
        .expect("start reg");

    let chal_str = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(ccr.public_key.challenge.as_ref());
    state.challenges.insert(chal_str.clone(), PendingChallenge {
        device_id: body.device_id.clone(),
        created_at: Instant::now(),
        kind: ChallengeKind::Register,
        expires_at: Instant::now() + Duration::from_secs(120),
        challenge: chal_str,
        reg_state: Some(reg_state),
        auth_state: None,
    });

    Json(ccr).into_response()
}

// -------- Register Finish --------

#[derive(Debug, Deserialize)]
pub struct RegisterFinishReq {
    pub device_id: String,
    pub id: String,
    pub rawId: String,
    #[serde(rename = "type")] pub typ: String,
    pub response: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SimpleOk { pub ok: bool }

pub async fn register_finish(State(state): State<Arc<PasskeyState>>, Json(body): Json<RegisterFinishReq>) -> impl IntoResponse {
    let reg_val = serde_json::json!({
        "id": body.id,
        "rawId": body.rawId,
        "type": body.typ,
        "response": body.response,
    });
    let response: RegisterPublicKeyCredential = serde_json::from_value(reg_val)
        .map_err(|_| (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false}))))
        .unwrap();

    let chal = extract_challenge_from_client_data_bytes(response.response.client_data_json.as_ref())
        .unwrap_or_default();
    let Some((_, pend)) = state.challenges.remove(&chal) else {
        return (
            axum::http::StatusCode::GONE,
            Json(serde_json::json!({"ok": false, "error": "challenge_not_found"}))
        ).into_response();
    };
    if pend.kind != ChallengeKind::Register || pend.device_id != body.device_id || Instant::now() >= pend.expires_at {
        return (
            axum::http::StatusCode::GONE,
            Json(serde_json::json!({"ok": false, "error": "challenge_invalid_or_expired"}))
        ).into_response();
    }

    let reg_state = pend.reg_state.expect("reg state exists");
    let res = state.webauthn.finish_passkey_registration(&response, &reg_state)
        .map_err(|_e| axum::http::StatusCode::BAD_REQUEST);
    if res.is_err() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false}))
        ).into_response();
    }
    let passkey = res.unwrap();
    let mut vec = state.passkeys_by_device.get(&body.device_id).map(|v| v.clone()).unwrap_or_default();
    vec.retain(|pk| pk.cred_id() != passkey.cred_id());
    vec.push(passkey);
    state.passkeys_by_device.insert(body.device_id.clone(), vec);
    Json(SimpleOk { ok: true }).into_response()
}

// -------- Assert Start --------

#[derive(Debug, Deserialize)]
pub struct AssertStartReq { pub device_id: String }

#[derive(Debug, Serialize)]
pub struct PublicKeyCredentialRequestOptions {
    pub challenge: String,
    #[serde(rename = "rpId")] pub rp_id: String,
    pub allowCredentials: Vec<HashMap<&'static str, String>>,
    pub userVerification: &'static str,
    pub timeout: u64,
}

pub async fn assert_start(State(state): State<Arc<PasskeyState>>, Json(body): Json<AssertStartReq>) -> impl IntoResponse {
    let Some(pks) = state.passkeys_by_device.get(&body.device_id) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"no_credential"}))
        ).into_response();
    };
    let passkeys: Vec<Passkey> = pks.clone();
    let (car, auth_state) = state.webauthn
        .start_passkey_authentication(&passkeys)
        .expect("start auth");

    let chal_str = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(car.public_key.challenge.as_ref());
    state.challenges.insert(chal_str.clone(), PendingChallenge {
        device_id: body.device_id.clone(),
        created_at: Instant::now(),
        kind: ChallengeKind::Assert,
        expires_at: Instant::now() + Duration::from_secs(120),
        challenge: chal_str,
        reg_state: None,
        auth_state: Some(auth_state),
    });

    Json(car).into_response()
}

// -------- Assert Finish --------

#[derive(Debug, Deserialize)]
pub struct AssertFinishReq {
    pub device_id: String,
    pub id: String,
    pub rawId: String,
    #[serde(rename = "type")] pub typ: String,
    pub response: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct AssertFinishRes { pub ok: bool, pub assertion_token: String }

pub async fn assert_finish(State(state): State<Arc<PasskeyState>>, Json(body): Json<AssertFinishReq>) -> impl IntoResponse {
    let auth_val = serde_json::json!({
        "id": body.id,
        "rawId": body.rawId,
        "type": body.typ,
        "response": body.response,
    });
    let response: PublicKeyCredential = serde_json::from_value(auth_val)
        .map_err(|_| (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({"ok": false}))))
        .unwrap();

    let chal = extract_challenge_from_client_data_bytes(response.response.client_data_json.as_ref())
        .unwrap_or_default();
    let Some((_, pend)) = state.challenges.remove(&chal) else {
        return (
            axum::http::StatusCode::GONE,
            Json(serde_json::json!({"ok": false, "error": "challenge_not_found"}))
        ).into_response();
    };
    if pend.kind != ChallengeKind::Assert || pend.device_id != body.device_id || Instant::now() >= pend.expires_at {
        return (
            axum::http::StatusCode::GONE,
            Json(serde_json::json!({"ok": false, "error": "challenge_invalid_or_expired"}))
        ).into_response();
    }

    let auth_state = pend.auth_state.expect("auth state exists");
    let auth_res = state.webauthn
        .finish_passkey_authentication(&response, &auth_state)
        .map_err(|_e| axum::http::StatusCode::BAD_REQUEST);
    if auth_res.is_err() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok": false}))
        ).into_response();
    }

    // Update credential counters if needed
    if let Ok(resu) = auth_res {
        if let Some(mut vec) = state.passkeys_by_device.get_mut(&body.device_id) {
            for pk in vec.iter_mut() {
                let _ = pk.update_credential(&resu);
            }
        }
    }

    let iat = now_unix();
    let exp = iat + 120;
    let payload = serde_json::json!({
        "device_id": body.device_id,
        "challenge": chal,
        "iat": iat,
        "exp": exp,
    }).to_string();
    let sig = hmac_sha256(&state.hmac_secret, payload.as_bytes());
    let token = format!("{}.{}", URL_SAFE_NO_PAD.encode(payload), URL_SAFE_NO_PAD.encode(sig));
    Json(AssertFinishRes { ok: true, assertion_token: token }).into_response()
}

fn hmac_sha256(secret: &str, msg: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn extract_challenge_from_client_data_bytes(client_data_bytes: &[u8]) -> Option<String> {
    // clientDataJSON is UTF-8 JSON bytes (already decoded). Extract challenge field (base64url) and normalize.
    let txt = String::from_utf8(client_data_bytes.to_vec()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let chal_b64_any = v.get("challenge")?.as_str()?;
    let chal_bytes = base64::engine::general_purpose::URL_SAFE.decode(chal_b64_any).or_else(|_| URL_SAFE_NO_PAD.decode(chal_b64_any)).ok()?;
    let chal_nopad = URL_SAFE_NO_PAD.encode(chal_bytes);
    Some(chal_nopad)
}


