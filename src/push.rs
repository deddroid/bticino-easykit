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
    let (fcm_token, _, android_id, security_token) =
        c.register().map_err(|e| Error::Other(format!("FCM registration: {e}")))?;
    let st = FcmState { android_id, security_token, fcm_token, private_key, auth_secret };
    std::fs::create_dir_all(config_dir())?;
    let body = toml::to_string(&st).map_err(|e| Error::Other(e.to_string()))?;
    crate::client::write_private(&state_path(), body.as_bytes())?;
    Ok((c, st, true))
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
    let r = api.post(p::DROPPUSHCONTEXT_PATH, &p::droppushcontext(&st.fcm_token))?;
    std::fs::remove_file(state_path())?;
    Ok(Some(r))
}

/// Blocks, calling `on_push` for every data message.
pub fn listen(mut c: FcmClient, on_push: impl Fn(Push) + Send + Sync + 'static) -> Result<()> {
    c.on_raw_message = Some(Arc::new(move |m| {
        let fields = m.app_data.into_iter().map(|d| (d.key, d.value)).collect();
        on_push(Push::from_fields(fields));
    }));
    c.start_listening().map_err(|e| Error::Other(format!("FCM listen: {e}")))
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
    fn non_rtc_push_has_no_rtc_type() {
        let mut f = BTreeMap::new();
        f.insert("push_type".into(), "BDIY-incoming_call".into());
        assert_eq!(Push::from_fields(f).rtc_type(), None);
    }
}
