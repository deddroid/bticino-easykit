//! Pure request builders for the Netatmo cloud API used by the
//! BTicino "Home + Security" app. Nothing here performs I/O, so every
//! payload can be checked byte-for-byte in unit tests.
//!
//! See `docs/PROTOCOL.md` for the consolidated protocol description.

use serde_json::{json, Value};

/// REST base URL.
pub const BASE_URL: &str = "https://app.netatmo.net";
/// OAuth2 token endpoint (form-encoded).
pub const TOKEN_PATH: &str = "/oauth2/token";
pub const HOMESDATA_PATH: &str = "/api/homesdata";
pub const HOMESTATUS_PATH: &str = "/syncapi/v1/homestatus";
pub const SETSTATE_PATH: &str = "/syncapi/v1/setstate";
pub const GETCONFIGS_PATH: &str = "/syncapi/v1/getconfigs";
pub const GETEVENTS_PATH: &str = "/api/getevents";
pub const ADDPUSHCONTEXT_PATH: &str = "/api/addpushcontext";
pub const DROPPUSHCONTEXT_PATH: &str = "/api/droppushcontext";
/// Push WebSocket (events: incoming call, missed call, connection...).
pub const PUSH_WS_URL: &str = "wss://app-ws.netatmo.net/ws/";
/// Signaling WebSocket (WebRTC SDP / ICE exchange).
pub const SIGNALING_WS_URL: &str = "wss://app-ws.netatmo.net/appws/";

/// OAuth client of the official Android app (public, embedded in the APK).
pub const CLIENT_ID: &str = "na_client_android_welcome";
pub const CLIENT_SECRET: &str = "8ab584d62ca2a77e37ccc6b2c7e4f29e";
pub const SCOPE: &str = "security_scopes";
pub const APP_VERSION: &str = "4.1.1.3";
pub const APP_TYPE: &str = "app_camera";
/// `versionCode` of Home + Security 26.10.1.1. `addpushcontext`
/// sends it as `app_version`; with an old value the call answers "ok" but
/// nothing is stored (a later `droppushcontext` says "Nothing to drop").
pub const APP_VERSION_CODE: &str = "500000030";

/// Honest User-Agent: the API accepts it, so we do not pose as the
/// official app. (The app sends `NetatmoApp(Security/v…) Android(…)`.)
pub fn user_agent() -> String {
    format!("bticino-easykit/{} (+https://github.com/deddroid/bticino-easykit)", env!("CARGO_PKG_VERSION"))
}

/// Form fields for the password grant.
pub fn password_grant<'a>(username: &'a str, password: &'a str) -> Vec<(&'static str, &'a str)> {
    vec![
        ("grant_type", "password"),
        ("username", username),
        ("password", password),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("scope", SCOPE),
        ("app_version", APP_VERSION),
    ]
}

/// Form fields for the refresh-token grant.
pub fn refresh_grant(refresh_token: &str) -> Vec<(&'static str, &str)> {
    vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
    ]
}

pub fn homesdata() -> Value {
    json!({ "app_type": APP_TYPE, "app_version": APP_VERSION, "sync_measurements": false })
}

pub fn homestatus(home_id: &str) -> Value {
    json!({ "app_type": APP_TYPE, "app_version": APP_VERSION, "home_id": home_id })
}

pub fn getevents(home_id: &str, size: u32) -> Value {
    json!({ "app_type": APP_TYPE, "app_version": APP_VERSION, "home_id": home_id, "size": size })
}

pub fn getconfigs(home_id: &str, module_ids: &[&str]) -> Value {
    json!({ "module_ids": module_ids, "home_id": home_id, "app_identifier": APP_TYPE })
}

/// `setstate` body for a single module. `bridge` is added only when the
/// target is not the bridge itself, as the official app does.
pub fn setstate(
    home_id: &str,
    timezone: Option<&str>,
    module_id: &str,
    bridge: Option<&str>,
    state: Value,
) -> Value {
    let mut module = json!({ "id": module_id });
    if let Some(b) = bridge.filter(|b| *b != module_id) {
        module["bridge"] = json!(b);
    }
    if let Value::Object(map) = state {
        for (k, v) in map {
            module[k] = v;
        }
    }
    let mut home = json!({ "id": home_id, "modules": [module] });
    if let Some(tz) = timezone {
        home["timezone"] = json!(tz);
    }
    json!({ "app_type": APP_TYPE, "app_version": APP_VERSION, "home": home })
}

/// Door release on a `BNDL` module (the device relocks by itself).
pub fn unlock_state() -> Value {
    json!({ "lock": false })
}

/// Staircase light on a `BNSL` module.
pub fn light_state(on: bool) -> Value {
    json!({ "on": on })
}

/// Firebase project of the official Android app (`res/values/strings.xml`).
pub const FCM_API_KEY: &str = "AIzaSyDpwbtvvtbLkvMGgWIB5mbKRFYzSX1Er18";
pub const FCM_APP_ID: &str = "1:514810499773:android:ed5e6854097137cb";
pub const FCM_PROJECT_ID: &str = "security-7228d";
pub const FCM_SENDER_ID: &str = "514810499773";
pub const ANDROID_PACKAGE: &str = "com.netatmo.camera";
/// SHA-1 of the APK signing certificate.
pub const ANDROID_CERT_SHA1: &str = "cdaf5d982a79b76743546599a87f7c45bac269d8";

/// Registers an FCM token so the cloud sends call pushes (including the
/// `BDIY-rtc` SDP offer, which never reaches the WebSocket) to this client.
pub fn addpushcontext(fcm_token: &str, device_version: &str, os_version: &str) -> Value {
    json!({
        "credentials": json!({ "device_token": fcm_token }).to_string(),
        "device_version": device_version,
        "os_version": os_version,
        "type": "Android",
        "app_version": APP_VERSION_CODE,
        "app_type": APP_TYPE,
    })
}

/// Undoes [`addpushcontext`].
pub fn droppushcontext(fcm_token: &str) -> Value {
    json!({
        "credentials": json!({ "device_token": fcm_token }).to_string(),
        "type": "Android",
    })
}

/// First message on the push WebSocket. No `filter` field on purpose:
/// with one, call notifications are suppressed.
pub fn push_subscribe(access_token: &str) -> Value {
    json!({
        "action": "Subscribe",
        "access_token": access_token,
        "app_type": APP_TYPE,
        "platform": "Android",
        "version": APP_VERSION,
    })
}

/// TURN/STUN credentials for WebRTC (POST, JSON body).
pub const TURN_URL: &str = "https://app-turn.netatmo.net/api/getcredentials";

pub fn turn_request() -> Value {
    json!({ "client_type": "user" })
}

/// First message on the signaling WebSocket.
pub fn signaling_subscribe(access_token: &str) -> Value {
    json!({
        "action": "subscribe",
        "access_token": access_token,
        "app_type": "app_security",
        "version": "1.0",
        "platform": "android",
    })
}

/// Identifies one WebRTC session once the server or a push assigned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcSession {
    pub session_id: String,
    pub tag_id: String,
    pub device_id: String,
    pub correlation_id: String,
}

/// Live view offer: `module_id` selects the external unit.
pub fn rtc_offer(device_id: &str, correlation_id: &str, sdp: &str, module_id: Option<&str>) -> Value {
    let mut sd = json!({ "type": "call", "sdp": sdp });
    if let Some(m) = module_id {
        sd["module_id"] = json!(m);
    }
    json!({
        "action": "rtc",
        "data": { "type": "offer", "session_description": sd },
        "device_id": device_id,
        "correlation_id": correlation_id,
    })
}

fn rtc_in_session(s: &RtcSession, data: Value) -> Value {
    json!({
        "action": "rtc",
        "data": data,
        "session_id": s.session_id,
        "tag_id": s.tag_id,
        "device_id": s.device_id,
        "correlation_id": s.correlation_id,
    })
}

/// Answer to a device offer received as a `*-rtc` push.
pub fn rtc_answer(s: &RtcSession, sdp: &str) -> Value {
    rtc_in_session(s, json!({ "type": "answer", "session_description": { "type": "call", "sdp": sdp } }))
}

/// Trickle ICE candidate.
pub fn rtc_candidate(s: &RtcSession, candidate: &str, sdp_m_line_index: u16) -> Value {
    rtc_in_session(
        s,
        json!({ "type": "candidate", "ice_candidate": { "sdp_m_line_index": sdp_m_line_index, "candidate": candidate } }),
    )
}

/// Hang up.
pub fn rtc_terminate(s: &RtcSession) -> Value {
    rtc_in_session(s, json!({ "type": "terminate" }))
}

/// Session carried by a `*-rtc` offer push (`extra_params`).
pub fn session_from_push(extra_params: &Value) -> Option<RtcSession> {
    let g = |k: &str| match &extra_params[k] {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    Some(RtcSession {
        session_id: g("session_id")?,
        tag_id: g("tag_id")?,
        device_id: g("device_id")?,
        correlation_id: g("correlation_id").unwrap_or_else(|| "0".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_this_project() {
        assert!(user_agent().starts_with("bticino-easykit/"));
    }

    #[test]
    fn password_grant_fields() {
        let f = password_grant("a@b.c", "pw");
        assert_eq!(f[0], ("grant_type", "password"));
        assert!(f.contains(&("scope", "security_scopes")));
        assert!(f.contains(&("client_id", "na_client_android_welcome")));
    }

    #[test]
    fn setstate_unlock_with_bridge() {
        let v = setstate("h1", Some("Europe/Rome"), "m1", Some("00:03:50:aa:bb:cc"), unlock_state());
        assert_eq!(
            v.to_string(),
            r#"{"app_type":"app_camera","app_version":"4.1.1.3","home":{"id":"h1","modules":[{"bridge":"00:03:50:aa:bb:cc","id":"m1","lock":false}],"timezone":"Europe/Rome"}}"#
        );
    }

    #[test]
    fn setstate_on_bridge_omits_bridge_field() {
        let v = setstate("h1", None, "00:03:50:aa:bb:cc", Some("00:03:50:aa:bb:cc"), light_state(true));
        assert_eq!(
            v.to_string(),
            r#"{"app_type":"app_camera","app_version":"4.1.1.3","home":{"id":"h1","modules":[{"id":"00:03:50:aa:bb:cc","on":true}]}}"#
        );
    }

    #[test]
    fn addpushcontext_body() {
        assert_eq!(
            addpushcontext("tok", "Linux", "1").to_string(),
            r#"{"app_type":"app_camera","app_version":"500000030","credentials":"{\"device_token\":\"tok\"}","device_version":"Linux","os_version":"1","type":"Android"}"#
        );
    }

    fn sess() -> RtcSession {
        RtcSession { session_id: "s".into(), tag_id: "t".into(), device_id: "d".into(), correlation_id: "7".into() }
    }

    #[test]
    fn offer_with_module() {
        assert_eq!(
            rtc_offer("d", "1", "v=0", Some("eu")).to_string(),
            r#"{"action":"rtc","correlation_id":"1","data":{"session_description":{"module_id":"eu","sdp":"v=0","type":"call"},"type":"offer"},"device_id":"d"}"#
        );
    }

    #[test]
    fn answer_candidate_terminate_carry_session() {
        assert_eq!(
            rtc_answer(&sess(), "v=0").to_string(),
            r#"{"action":"rtc","correlation_id":"7","data":{"session_description":{"sdp":"v=0","type":"call"},"type":"answer"},"device_id":"d","session_id":"s","tag_id":"t"}"#
        );
        assert_eq!(rtc_candidate(&sess(), "candidate:1", 0)["data"]["ice_candidate"]["sdp_m_line_index"], 0);
        assert_eq!(rtc_terminate(&sess())["data"]["type"], "terminate");
    }

    #[test]
    fn session_from_push_accepts_numeric_correlation() {
        let ep = json!({"session_id":"s","tag_id":"t","device_id":"d","correlation_id":7});
        assert_eq!(session_from_push(&ep), Some(sess()));
        assert_eq!(session_from_push(&json!({"session_id":"s"})), None);
    }

    #[test]
    fn push_subscribe_has_no_filter() {
        let v = push_subscribe("tok");
        assert!(v.get("filter").is_none());
        assert_eq!(v["action"], "Subscribe");
    }
}
