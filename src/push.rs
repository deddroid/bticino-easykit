//! Call notifications over Firebase Cloud Messaging.
//!
//! The official app subscribes to the push WebSocket with
//! `filter: "silent"` and gets calls from FCM instead. On EASYKIT the
//! `BDIY-rtc` push (the SDP offer needed to answer a call) is only sent
//! there, so we register as an Android FCM client of the app's Firebase
//! project and hand the token to `/api/addpushcontext`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use fcm_receiver_rs::client::{AndroidApp, FcmClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{config_dir, Client, Error, Result};
use crate::protocol as p;

/// FCM identity persisted between runs, so the token stays stable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcmState {
    pub android_id: u64,
    pub security_token: u64,
    pub fcm_token: String,
    pub private_key: String,
    pub auth_secret: String,
}

/// One push as received: the FCM data keys (`push_type`, `extra_params`,
/// `alert`, ...) with `extra_params` already parsed when it is JSON.
#[derive(Debug, Clone)]
pub struct Push {
    pub push_type: String,
    pub extra_params: Value,
    pub fields: BTreeMap<String, String>,
}

impl Push {
    /// Pure conversion from FCM key/value pairs.
    pub fn from_fields(fields: BTreeMap<String, String>) -> Self {
        let push_type = fields.get("push_type").cloned().unwrap_or_default();
        let extra_params = fields
            .get("extra_params")
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        Self { push_type, extra_params, fields }
    }

    /// Message from the push WebSocket: `{"type":"Websocket","push_type":…,"extra_params":{…}}`.
    pub fn from_ws(v: &Value) -> Self {
        let mut fields = BTreeMap::new();
        if let Some(o) = v.as_object() {
            for (k, x) in o {
                if k != "extra_params" {
                    fields.insert(k.clone(), x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string()));
                }
            }
        }
        Self {
            push_type: v["push_type"].as_str().unwrap_or_default().to_owned(),
            extra_params: v["extra_params"].clone(),
            fields,
        }
    }

    /// Decrypted web push: `{"data": {"push_type": ..., "extra_params": ...}, ...}`.
    pub fn from_webpush(bytes: &[u8]) -> Self {
        let v: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
        let src = if v["data"].is_object() { &v["data"] } else { &v };
        let fields = src
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, x)| (k.clone(), x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())))
                    .collect()
            })
            .unwrap_or_else(|| {
                BTreeMap::from([("raw".to_owned(), String::from_utf8_lossy(bytes).into_owned())])
            });
        Self::from_fields(fields)
    }

    /// `"offer"`, `"terminate"`, `"rescind"` for `*-rtc` pushes.
    pub fn rtc_type(&self) -> Option<&str> {
        self.push_type.ends_with("-rtc").then(|| self.extra_params["data"]["type"].as_str()).flatten()
    }
}

fn state_path() -> PathBuf {
    config_dir().join("fcm.toml")
}

fn new_fcm_client() -> Result<FcmClient> {
    let mut c = FcmClient::new(p::FCM_API_KEY.into(), p::FCM_APP_ID.into(), p::FCM_PROJECT_ID.into())
        .map_err(|e| Error::Other(format!("FCM: {e}")))?;
    c.android_app = Some(AndroidApp {
        gcm_sender_id: p::FCM_SENDER_ID.into(),
        android_package: p::ANDROID_PACKAGE.into(),
        android_package_cert: p::ANDROID_CERT_SHA1.into(),
    });
    Ok(c)
}

/// Loads the saved FCM identity or registers a new one.
pub fn load_or_register() -> Result<(FcmClient, FcmState, bool)> {
    let mut c = new_fcm_client()?;
    if let Some(st) = std::fs::read_to_string(state_path()).ok().and_then(|s| toml::from_str::<FcmState>(&s).ok()) {
        c.load_keys(&st.private_key, &st.auth_secret).map_err(|e| Error::Other(format!("FCM: {e}")))?;
        c.android_id = st.android_id;
        c.security_token = st.security_token;
        c.fcm_token = Some(st.fcm_token.clone());
        return Ok((c, st, false));
    }
    let (private_key, auth_secret) = c.create_new_keys().map_err(|e| Error::Other(format!("FCM: {e}")))?;
    let (fcm_token, android_id, security_token) = register_web(&private_key, &auth_secret).map_err(|e| Error::Other(format!("FCM registration: {e}")))?;
    let st = FcmState { android_id, security_token, fcm_token, private_key, auth_secret };
    std::fs::create_dir_all(config_dir())?;
    let body = toml::to_string(&st).map_err(|e| Error::Other(e.to_string()))?;
    crate::client::write_private(&state_path(), body.as_bytes())?;
    Ok((c, st, true))
}

/// Web (Chrome) FCM registration on the app's Firebase project, as done by
/// `firebase-messaging` for Ring: GCM check-in, `register3` as
/// `org.chromium.linux` with a `wp:` subtype (retried, errors are often
/// transient), Firebase Installation, then a web push registration. The
/// API key only accepts the app's `X-Android-Package`/`X-Android-Cert`.
/// (A pure Android `register3` gets `FIS_AUTH_ERROR`.)
fn register_web(private_key_b64: &str, auth_secret_b64: &str) -> fcm_receiver_rs::Result<(String, u64, u64)> {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use base64::Engine;
    use fcm_receiver_rs::{crypto, http_client as h, Error as E};

    let http = h::build_default_http_client()?;
    let resp = h::send_gcm_check_in_request(&http, &fcm_receiver_rs::messages::create_check_in_request(0, None))?;
    let android_id = resp.android_id.ok_or(E::InvalidData("no android_id"))?;
    let security_token = resp.security_token.ok_or(E::InvalidData("no security_token"))?;

    let subtype = format!("wp:receiver.push.com#{}", fcm_receiver_rs::util::generate_firebase_fid()?);
    let mut gcm_token = None;
    let mut last = None;
    for _ in 0..5 {
        let req = h::GcmRegisterRequest {
            android_id,
            security_token,
            app_id: p::FCM_APP_ID,
            android_app: None,
            installation_auth: None,
            sender_override: None,
            subtype_override: Some(subtype.clone()),
            delete: false,
            extra_params: Vec::new(),
        };
        match h::send_gcm_register_request(&http, &req) {
            Ok(t) => {
                gcm_token = Some(t);
                break;
            }
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
    }
    let gcm_token = match gcm_token {
        Some(t) => t,
        None => return Err(last.unwrap_or(E::InvalidData("register3 failed"))),
    };

    let fid = fcm_receiver_rs::util::generate_firebase_fid()?;
    let install = h::send_fcm_install_request(
        &http,
        &h::FcmInstallRequest {
            api_key: p::FCM_API_KEY,
            project_id: p::FCM_PROJECT_ID,
            app_id: p::FCM_APP_ID,
            fid: &fid,
            // The API key is restricted to the Android app.
            android_app: Some(h::AndroidAppOptions {
                gcm_sender_id: p::FCM_SENDER_ID,
                android_package: p::ANDROID_PACKAGE,
                android_package_cert: p::ANDROID_CERT_SHA1,
            }),
            firebase_client_header: None,
            firebase_client_log_type: None,
            user_agent: None,
        },
    )?;

    let sk = crypto::decode_private_key(&STANDARD.decode(private_key_b64).map_err(|_| E::InvalidData("key"))?)?;
    let auth = STANDARD.decode(auth_secret_b64).map_err(|_| E::InvalidData("auth"))?;
    let body = serde_json::json!({ "web": {
        "applicationPubKey": null,
        "auth": URL_SAFE_NO_PAD.encode(auth),
        "endpoint": h::build_endpoint_url(&gcm_token),
        "p256dh": URL_SAFE_NO_PAD.encode(crypto::public_key_bytes(&sk.public_key())),
    }});
    let r = reqwest::blocking::Client::new()
        .post(format!("https://fcmregistrations.googleapis.com/v1/projects/{}/registrations", p::FCM_PROJECT_ID))
        .header("x-goog-api-key", p::FCM_API_KEY)
        .header("X-Android-Package", p::ANDROID_PACKAGE)
        .header("X-Android-Cert", p::ANDROID_CERT_SHA1)
        .header("x-goog-firebase-installations-auth", &install.auth_token.token)
        .json(&body)
        .send()
        .map_err(|e| E::Other(e.to_string()))?;
    let status = r.status();
    let text = r.text().map_err(|e| E::Other(e.to_string()))?;
    if !status.is_success() {
        return Err(E::Other(format!("fcmregistrations HTTP {status}: {text}")));
    }
    let v: Value = serde_json::from_str(&text)?;
    let token = v["token"].as_str().ok_or(E::InvalidData("no FCM token"))?.to_owned();
    Ok((token, android_id, security_token))
}

/// Tells the Netatmo cloud to deliver call pushes to this FCM token.
pub fn add_push_context(api: &mut Client, fcm_token: &str) -> Result<Value> {
    let device = format!("bticino-easykit+{}", std::env::consts::OS);
    let body = p::addpushcontext(fcm_token, &device, env!("CARGO_PKG_VERSION"));
    api.post(p::ADDPUSHCONTEXT_PATH, &body)
}

/// Stops call pushes to the saved FCM token and forgets the identity.
pub fn drop_push_context(api: &mut Client) -> Result<Option<Value>> {
    let Some(st) = std::fs::read_to_string(state_path()).ok().and_then(|s| toml::from_str::<FcmState>(&s).ok())
    else {
        return Ok(None);
    };
    // The cloud answers 406 `{"code":7,"message":"Nothing to drop"}` even
    // right after a successful add, with JSON or form bodies alike; treat it
    // as done and forget the local identity anyway.
    let r = match api.post(p::DROPPUSHCONTEXT_PATH, &p::droppushcontext(&st.fcm_token)) {
        Err(Error::Api { status: 406, body }) => serde_json::from_str(&body).unwrap_or(Value::Null),
        other => other?,
    };
    std::fs::remove_file(state_path())?;
    Ok(Some(r))
}

/// Blocks, calling `on_push` for every data message.
pub fn listen(mut c: FcmClient, on_push: impl Fn(Push) + Send + Sync + 'static) -> Result<()> {
    let on_push = Arc::new(on_push);
    let raw = on_push.clone();
    c.on_raw_message = Some(Arc::new(move |m| {
        let fields = m.app_data.into_iter().map(|d| (d.key, d.value)).collect();
        raw(Push::from_fields(fields));
    }));
    c.on_data_message = Some(Arc::new(move |bytes| on_push(Push::from_webpush(&bytes))));
    c.start_listening().map_err(|e| Error::Other(format!("FCM listen: {e}")))
}

/// Listens on the push WebSocket (`app_camera`, no filter) and forwards
/// every message. Re-subscribes with a fresh token every 50 minutes.
pub async fn listen_ws(
    mut token: impl FnMut() -> Result<String> + Send,
    tx: tokio::sync::mpsc::UnboundedSender<Push>,
) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let (mut ws, _) = tokio_tungstenite::connect_async(p::PUSH_WS_URL)
        .await
        .map_err(|e| Error::Other(format!("push ws: {e}")))?;
    let subscribe = |t: String| Message::Text(p::push_subscribe(&t).to_string().into());
    ws.send(subscribe(token()?)).await.map_err(|e| Error::Other(e.to_string()))?;
    let mut resub = tokio::time::interval(std::time::Duration::from_secs(50 * 60));
    resub.tick().await;
    loop {
        tokio::select! {
            _ = resub.tick() => {
                ws.send(subscribe(token()?)).await.map_err(|e| Error::Other(e.to_string()))?;
            }
            m = ws.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        let _ = tx.send(Push::from_ws(&v));
                    }
                }
                Some(Ok(Message::Close(_))) | None => return Err(Error::Other("push ws closed".into())),
                Some(Err(e)) => return Err(Error::Other(format!("push ws: {e}"))),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bdiy_rtc_offer() {
        let mut f = BTreeMap::new();
        f.insert("push_type".into(), "BDIY-rtc".into());
        f.insert(
            "extra_params".into(),
            r#"{"session_id":"s","tag_id":"t","device_id":"00:03:50:aa:bb:cc","home_id":"h","data":{"type":"offer","session_description":{"type":"call","sdp":"v=0"}}}"#.into(),
        );
        let p = Push::from_fields(f);
        assert_eq!(p.rtc_type(), Some("offer"));
        assert_eq!(p.extra_params["tag_id"], "t");
    }

    #[test]
    fn parses_webpush_payload() {
        let p = Push::from_webpush(
            br#"{"data":{"push_type":"BDIY-rtc","extra_params":"{\"data\":{\"type\":\"terminate\"}}"},"from":"514810499773"}"#,
        );
        assert_eq!(p.rtc_type(), Some("terminate"));
    }

    #[test]
    fn parses_ws_message() {
        let v = serde_json::json!({"type":"Websocket","push_type":"BDIY-incoming_call","extra_params":{"session_id":"s"}});
        let p = Push::from_ws(&v);
        assert_eq!(p.push_type, "BDIY-incoming_call");
        assert_eq!(p.extra_params["session_id"], "s");
    }

    #[test]
    fn non_rtc_push_has_no_rtc_type() {
        let mut f = BTreeMap::new();
        f.insert("push_type".into(), "BDIY-incoming_call".into());
        assert_eq!(Push::from_fields(f).rtc_type(), None);
    }
}
