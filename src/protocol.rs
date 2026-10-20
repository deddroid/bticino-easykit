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
pub const BUILD_NUMBER: &str = "4401103";
pub const APP_TYPE: &str = "app_camera";

/// User-Agent sent by the official app.
pub fn user_agent() -> String {
    format!("NetatmoApp(Security/v{APP_VERSION}/{BUILD_NUMBER}) Android(13/Google/sdk_gphone64_arm64)")
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
/// Mirrors `u10.d.d` in the official app.
pub fn addpushcontext(fcm_token: &str, device_version: &str, os_version: &str) -> Value {
    json!({
        "credentials": json!({ "device_token": fcm_token }).to_string(),
        "device_version": device_version,
        "os_version": os_version,
        "type": "Android",
        "app_version": APP_VERSION,
        "app_type": APP_TYPE,
    })
}

/// Undoes [`addpushcontext`] (`u10.d.k` in the official app).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_matches_official_app() {
        assert_eq!(
            user_agent(),
            "NetatmoApp(Security/v4.1.1.3/4401103) Android(13/Google/sdk_gphone64_arm64)"
        );
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
            r#"{"app_type":"app_camera","app_version":"4.1.1.3","credentials":"{\"device_token\":\"tok\"}","device_version":"Linux","os_version":"1","type":"Android"}"#
        );
    }

    #[test]
    fn push_subscribe_has_no_filter() {
        let v = push_subscribe("tok");
        assert!(v.get("filter").is_none());
        assert_eq!(v["action"], "Subscribe");
    }
}
