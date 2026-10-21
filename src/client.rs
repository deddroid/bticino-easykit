//! Blocking HTTP client for the Netatmo cloud API.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{parse_homes, Home};
use crate::protocol as p;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not logged in: run `bticino-easykit login` first")]
    NotLoggedIn,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("API error {status}: {body}")]
    Api { status: u16, body: String },
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// OAuth tokens persisted between runs. The password is never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `$BTICINO_EASYKIT_CONFIG_DIR` or `<config_dir>/bticino-easykit`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("BTICINO_EASYKIT_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| ".".into()).join("bticino-easykit"))
}

pub fn debug_enabled() -> bool {
    std::env::var("BTICINO_EASYKIT_DEBUG").is_ok_and(|v| v == "1")
}

/// Hides secrets before raw traffic is printed in debug mode.
pub fn redact(s: &str) -> String {
    let mut v: Value = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(_) => return s.to_owned(),
    };
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if matches!(k.as_str(), "access_token" | "refresh_token" | "password" | "credential") {
                        *x = Value::String("<redacted>".into());
                    } else {
                        walk(x);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v.to_string()
}

pub struct Client {
    http: reqwest::blocking::Client,
    tokens: Option<Tokens>,
    token_path: PathBuf,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(p::user_agent())
            .timeout(Duration::from_secs(30))
            .build()?;
        let token_path = config_dir().join("tokens.toml");
        let tokens = std::fs::read_to_string(&token_path).ok().and_then(|s| toml::from_str(&s).ok());
        Ok(Self { http, tokens, token_path })
    }

    pub fn is_logged_in(&self) -> bool {
        self.tokens.is_some()
    }

    fn save_tokens(&self) -> Result<()> {
        let Some(t) = &self.tokens else { return Ok(()) };
        std::fs::create_dir_all(self.token_path.parent().unwrap())?;
        let body = toml::to_string(t).map_err(|e| Error::Other(e.to_string()))?;
        write_private(&self.token_path, body.as_bytes())
    }

    pub fn logout(&mut self) -> Result<()> {
        self.tokens = None;
        match std::fs::remove_file(&self.token_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn token_request(&mut self, form: &[(&str, &str)]) -> Result<()> {
        let url = format!("{}{}", p::BASE_URL, p::TOKEN_PATH);
        if debug_enabled() {
            eprintln!(">> POST {url} (form, grant_type={})", form[0].1);
        }
        let resp = self.http.post(&url).form(form).send()?;
        let status = resp.status().as_u16();
        let text = resp.text()?;
        if debug_enabled() {
            eprintln!("<< {status} {}", redact(&text));
        }
        if status >= 400 {
            return Err(Error::Auth(text));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| Error::Auth(e.to_string()))?;
        let access = v["access_token"].as_str().ok_or_else(|| Error::Auth("no access_token".into()))?;
        let refresh = v["refresh_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.tokens.as_ref().map(|t| t.refresh_token.clone()))
            .ok_or_else(|| Error::Auth("no refresh_token".into()))?;
        self.tokens = Some(Tokens {
            access_token: access.to_owned(),
            refresh_token: refresh,
            expires_at: now() + v["expires_in"].as_u64().unwrap_or(10800),
        });
        self.save_tokens()
    }

    pub fn login(&mut self, username: &str, password: &str) -> Result<()> {
        self.token_request(&p::password_grant(username, password))
    }

    fn refresh(&mut self) -> Result<()> {
        let rt = self.tokens.as_ref().ok_or(Error::NotLoggedIn)?.refresh_token.clone();
        self.token_request(&p::refresh_grant(&rt))
    }

    fn access_token(&mut self) -> Result<String> {
        let t = self.tokens.as_ref().ok_or(Error::NotLoggedIn)?;
        if now() + 60 >= t.expires_at {
            self.refresh()?;
        }
        Ok(self.tokens.as_ref().unwrap().access_token.clone())
    }

    /// Fresh access token, e.g. for the WebSocket subscribe message.
    pub fn token(&mut self) -> Result<String> {
        self.access_token()
    }

    /// Authenticated JSON POST. Refreshes once and retries on an auth error.
    pub fn post(&mut self, path: &str, body: &Value) -> Result<Value> {
        let mut retried = false;
        loop {
            let token = self.access_token()?;
            let url = format!("{}{}", p::BASE_URL, path);
            if debug_enabled() {
                eprintln!(">> POST {url} {}", redact(&body.to_string()));
            }
            let resp = self.http.post(&url).bearer_auth(token).json(body).send()?;
            let status = resp.status().as_u16();
            let text = resp.text()?;
            if debug_enabled() {
                eprintln!("<< {status} {}", redact(&text));
            }
            if (status == 401 || status == 403) && !retried {
                retried = true;
                self.refresh()?;
                continue;
            }
            if status >= 400 {
                return Err(Error::Api { status, body: text });
            }
            return serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()));
        }
    }

    /// Authenticated form-encoded POST (the app's `a40.a` parameter maps).
    pub fn post_form(&mut self, path: &str, form: &[(&str, String)]) -> Result<Value> {
        let mut retried = false;
        loop {
            let token = self.access_token()?;
            let url = format!("{}{}", p::BASE_URL, path);
            if debug_enabled() {
                eprintln!(">> POST {url} (form) {:?}", form.iter().map(|(k, _)| *k).collect::<Vec<_>>());
            }
            let resp = self.http.post(&url).bearer_auth(token).form(form).send()?;
            let status = resp.status().as_u16();
            let text = resp.text()?;
            if debug_enabled() {
                eprintln!("<< {status} {}", redact(&text));
            }
            if (status == 401 || status == 403) && !retried {
                retried = true;
                self.refresh()?;
                continue;
            }
            if status >= 400 {
                return Err(Error::Api { status, body: text });
            }
            return serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()));
        }
    }

    pub fn homesdata_raw(&mut self) -> Result<Value> {
        self.post(p::HOMESDATA_PATH, &p::homesdata())
    }

    pub fn homes(&mut self) -> Result<Vec<Home>> {
        Ok(parse_homes(&self.homesdata_raw()?))
    }

    pub fn homestatus(&mut self, home_id: &str) -> Result<Value> {
        self.post(p::HOMESTATUS_PATH, &p::homestatus(home_id))
    }

    pub fn events(&mut self, home_id: &str, size: u32) -> Result<Value> {
        self.post(p::GETEVENTS_PATH, &p::getevents(home_id, size))
    }

    pub fn configs(&mut self, home_id: &str, module_ids: &[&str]) -> Result<Value> {
        self.post(p::GETCONFIGS_PATH, &p::getconfigs(home_id, module_ids))
    }

    /// TURN/STUN servers for WebRTC (`getcredentials`).
    pub fn turn(&mut self) -> Result<Value> {
        let token = self.access_token()?;
        if debug_enabled() {
            eprintln!(">> POST {}", p::TURN_URL);
        }
        let resp = self.http.post(p::TURN_URL).bearer_auth(token).json(&p::turn_request()).send()?;
        let status = resp.status().as_u16();
        let text = resp.text()?;
        if debug_enabled() {
            eprintln!("<< {status} {}", redact(&text));
        }
        if status >= 400 {
            return Err(Error::Api { status, body: text });
        }
        serde_json::from_str(&text).map_err(|e| Error::Other(e.to_string()))
    }

    pub fn set_state(&mut self, home: &Home, module_id: &str, state: Value) -> Result<Value> {
        let bridge = home
            .module(module_id)
            .and_then(|m| m.bridge.clone())
            .or_else(|| home.bridge().map(|b| b.id.clone()));
        let body = p::setstate(&home.id, home.timezone.as_deref(), module_id, bridge.as_deref(), state);
        let v = self.post(p::SETSTATE_PATH, &body)?;
        if v["status"] != "ok" {
            return Err(Error::Other(format!("device rejected command: {v}")));
        }
        Ok(v)
    }
}

#[cfg(unix)]
pub(crate) fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(data)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn write_private(path: &std::path::Path, data: &[u8]) -> Result<()> {
    Ok(std::fs::write(path, data)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_hides_tokens_recursively() {
        let r = redact(r#"{"access_token":"x","body":{"refresh_token":"y","n":1}}"#);
        assert!(!r.contains("\"x\"") && !r.contains("\"y\""));
        assert!(r.contains("\"n\":1"));
    }
}
