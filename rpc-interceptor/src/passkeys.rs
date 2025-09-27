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
#[derive(Clone)]
pub struct ResumeState {
    pub passkeys: Arc<PasskeyState>,
    pub pairing: Arc<crate::pairing::PairingState>,
}

#[derive(Debug, Deserialize)]
pub struct ResumeReq { pub assertion_token: String }

#[derive(Debug, Serialize)]
pub struct ResumeRes { pub ok: bool, pub session_id: String, pub rpc_endpoint: String }

pub async fn resume_session(State(state): State<Arc<ResumeState>>, Json(body): Json<ResumeReq>) -> impl IntoResponse {
    let Some(dev_id) = validate_assertion_token(&state.passkeys, &body.assertion_token) else {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"ok": false, "error": "invalid_token"}))
        ).into_response();
    };

    // Create a fresh session_id and index it so RPC accepts it.
    let session_id = uuid::Uuid::new_v4().to_string();
    state.pairing.sessions.insert(session_id.clone(), "resume".to_string());
    let rpc_endpoint = format!("http://localhost:3000/rpc/{}", session_id);
    Json(ResumeRes { ok: true, session_id, rpc_endpoint }).into_response()
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
pub struct RegisterStartReq {
    pub device_id: Option<String>,
    pub email: Option<String>,
    pub display_name: String,
}

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
    let principal = if let Some(e) = body.email.clone() { e }
        else if let Some(d) = body.device_id.clone() { d } else {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok": false, "error": "missing_identifier"}))
            ).into_response();
        };
    let user_unique_id = Uuid::new_v4();
    let (ccr, reg_state) = state.webauthn
        .start_passkey_registration(
            user_unique_id,
            &principal,
            &body.display_name,
            None,
        )
        .expect("start reg");

    let chal_str = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(ccr.public_key.challenge.as_ref());
    state.challenges.insert(chal_str.clone(), PendingChallenge {
        device_id: principal.clone(),
        created_at: Instant::now(),
        kind: ChallengeKind::Register,
        expires_at: Instant::now() + Duration::from_secs(120),
        challenge: chal_str,
        reg_state: Some(reg_state),
        auth_state: None,
    });

    // Force ES256 only to avoid Android TYPE_NOT_SUPPORTED_ERROR
    let mut v = serde_json::to_value(&ccr).unwrap_or_else(|_| serde_json::json!({}));
    {
        // helper to set pubKeyCredParams
        fn set_es256(target: &mut serde_json::Value) {
            *target
                .as_object_mut()
                .unwrap()
                .entry("pubKeyCredParams")
                .or_insert(serde_json::json!([])) = serde_json::json!([
                {"type":"public-key","alg":-7}
            ]);
        }
        if let Some(pk) = v.get_mut("publicKey") {
            set_es256(pk);
        } else if v.is_object() {
            set_es256(&mut v);
        }
    }
    if let Some(pk) = v.get("publicKey") {
        let rp = pk.get("rp").cloned().unwrap_or(serde_json::json!({}));
        let rp_id_dbg = rp.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let rp_name_dbg = rp.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let user = pk.get("user").cloned().unwrap_or(serde_json::json!({}));
        let user_name_dbg = user.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let user_id_len = user.get("id").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0);
        let chal_len = pk.get("challenge").and_then(|c| c.as_str()).map(|s| s.len()).unwrap_or(0);
        let params = pk.get("pubKeyCredParams").cloned().unwrap_or(serde_json::json!([]));
        println!(
            "[register_start] rp_id={} rp_name={} user_name={} user_id_len={} challenge_len={} algs={}",
            rp_id_dbg, rp_name_dbg, user_name_dbg, user_id_len, chal_len, params
        );
    }
    Json(v).into_response()
}

// -------- Register Finish --------

#[derive(Debug, Deserialize)]
pub struct RegisterFinishReq {
    pub device_id: Option<String>,
    pub email: Option<String>,
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
    let principal = body.email.clone().or(body.device_id.clone());
    if pend.kind != ChallengeKind::Register || Instant::now() >= pend.expires_at || principal.as_deref() != Some(pend.device_id.as_str()) {
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
    let principal = principal.expect("validated above");
    let mut vec = state.passkeys_by_device.get(&principal).map(|v| v.clone()).unwrap_or_default();
    vec.retain(|pk| pk.cred_id() != passkey.cred_id());
    vec.push(passkey);
    state.passkeys_by_device.insert(principal, vec);
    Json(SimpleOk { ok: true }).into_response()
}

// -------- Assert Start --------

#[derive(Debug, Deserialize)]
pub struct AssertStartReq { pub device_id: Option<String>, pub email: Option<String> }

#[derive(Debug, Serialize)]
pub struct PublicKeyCredentialRequestOptions {
    pub challenge: String,
    #[serde(rename = "rpId")] pub rp_id: String,
    pub allowCredentials: Vec<HashMap<&'static str, String>>,
    pub userVerification: &'static str,
    pub timeout: u64,
}

pub async fn assert_start(State(state): State<Arc<PasskeyState>>, Json(body): Json<AssertStartReq>) -> impl IntoResponse {
    let principal = body.email.clone().or(body.device_id.clone());
    let (car, auth_state, pend_device_id) = if let Some(id) = principal.clone() {
        let Some(pks) = state.passkeys_by_device.get(&id) else {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error":"no_credential"}))
            ).into_response();
        };
        let passkeys: Vec<Passkey> = pks.clone();
        let (car, auth_state) = state.webauthn
            .start_passkey_authentication(&passkeys)
            .expect("start auth");
        (car, auth_state, id)
    } else {
        // Discoverable/usernameless: aggregate all known passkeys as allow list
        let mut all: Vec<Passkey> = Vec::new();
        for entry in state.passkeys_by_device.iter() {
            all.extend(entry.value().iter().cloned());
        }
        let (car, auth_state) = state.webauthn
            .start_passkey_authentication(&all)
            .expect("start discoverable auth");
        (car, auth_state, String::new())
    };

    let chal_str = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(car.public_key.challenge.as_ref());
    state.challenges.insert(chal_str.clone(), PendingChallenge {
        device_id: pend_device_id,
        created_at: Instant::now(),
        kind: ChallengeKind::Assert,
        expires_at: Instant::now() + Duration::from_secs(120),
        challenge: chal_str,
        reg_state: None,
        auth_state: Some(auth_state),
    });

    // Ensure request challenges prefer ES256
    let mut v = serde_json::to_value(&car).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(pk) = v.get_mut("publicKey") {
        pk.as_object_mut().unwrap().insert(
            "pubKeyCredParams".to_string(),
            serde_json::json!([{ "type": "public-key", "alg": -7 }]),
        );
    }
    if let Some(pk) = v.get("publicKey") {
        let rp_id_dbg = pk.get("rpId").cloned().unwrap_or(serde_json::json!("?"));
        let chal_len = pk.get("challenge").and_then(|c| c.as_str()).map(|s| s.len()).unwrap_or(0);
        let allow_len = pk.get("allowCredentials").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0);
        let params = pk.get("pubKeyCredParams").cloned().unwrap_or(serde_json::json!([]));
        println!("[assert_start] rp_id={} challenge_len={} allow_len={} algs={}", rp_id_dbg, chal_len, allow_len, params);
    }
    Json(v).into_response()
}

// -------- Assert Finish --------

#[derive(Debug, Deserialize)]
pub struct AssertFinishReq {
    pub device_id: Option<String>,
    pub email: Option<String>,
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
    let principal = body.email.clone().or(body.device_id.clone());
    if pend.kind != ChallengeKind::Assert || Instant::now() >= pend.expires_at || (!pend.device_id.is_empty() && principal.as_deref() != Some(pend.device_id.as_str())) {
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
    let mut owner_id: Option<String> = None;
    if let Ok(resu) = auth_res {
        // Update match owner if known
        if let Some(id) = principal.clone() {
            if let Some(mut vec) = state.passkeys_by_device.get_mut(&id) {
                for pk in vec.iter_mut() {
                    let _ = pk.update_credential(&resu);
                }
                owner_id = Some(id);
            }
        } else {
            // Discover owner by credential id
            let cred = resu.cred_id();
            for entry in state.passkeys_by_device.iter() {
                let key = entry.key().clone();
                let mut vec = entry.value().clone();
                let mut updated = false;
                for pk in vec.iter_mut() {
                    if pk.cred_id() == cred {
                        let _ = pk.update_credential(&resu);
                        updated = true;
                    }
                }
                if updated {
                    state.passkeys_by_device.insert(key.clone(), vec);
                    owner_id = Some(key);
                    break;
                }
            }
        }
    }

    let iat = now_unix();
    let exp = iat + 120;
    let payload = serde_json::json!({
        "device_id": owner_id.unwrap_or_else(|| principal.unwrap_or_default()),
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

fn validate_assertion_token(state: &PasskeyState, token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 2 { return None; }
    let payload_b64 = parts[0];
    let sig_b64 = parts[1];
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig_b64).ok()?;
    let mac = hmac_sha256(&state.hmac_secret, &payload_bytes);
    if mac != sig { return None; }
    let v: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    let dev = v.get("device_id")?.as_str()?.to_string();
    let exp = v.get("exp")?.as_u64()?;
    if now_unix() > exp { return None; }
    Some(dev)
}


