//! Application settings (`settings.toml`), separate from the device: the
//! intercom's own state is always read back from the cloud, so no device
//! profile has to be stored locally.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::client::config_dir;
use crate::i18n::Lang;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub lang: Lang,
    pub autostart: bool,
    pub tray: bool,
    pub notify_on_ring: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { lang: Lang::from_env(), autostart: false, tray: true, notify_on_ring: true }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.toml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        let s = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(Self::path(), s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = toml::from_str("lang = \"it\"").unwrap();
        assert_eq!(s.lang, Lang::It);
        assert!(s.tray && s.notify_on_ring && !s.autostart);
    }

    #[test]
    fn roundtrip() {
        let s = Settings { lang: Lang::En, autostart: true, tray: false, notify_on_ring: false };
        let back: Settings = toml::from_str(&toml::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }
}
