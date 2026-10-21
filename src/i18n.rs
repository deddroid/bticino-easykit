//! User-facing strings, English by default with Italian selectable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    It,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::It];

    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::It => "Italiano",
        }
    }

    /// From `LC_ALL` / `LC_MESSAGES` / `LANG`.
    pub fn from_env() -> Self {
        let v = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
            .unwrap_or_default();
        if v.starts_with("it") {
            Lang::It
        } else {
            Lang::En
        }
    }
}

/// `(key, english, italian)`.
const TABLE: &[(&str, &str, &str)] = &[
    ("app.title", "BTicino EASYKIT", "BTicino EASYKIT"),
    ("tab.home", "Home", "Home"),
    ("tab.live", "Live", "Live"),
    ("tab.events", "Calls", "Chiamate"),
    ("tab.settings", "Settings", "Impostazioni"),
    ("login.title", "Sign in with your Home + Security account", "Accedi con il tuo account Home + Security"),
    ("login.email", "Email", "Email"),
    ("login.password", "Password", "Password"),
    ("login.button", "Sign in", "Accedi"),
    ("login.note", "The password is not stored, only the session tokens.", "La password non viene salvata, solo i token di sessione."),
    ("logout", "Sign out", "Esci"),
    ("refresh", "Refresh", "Aggiorna"),
    ("busy", "Working…", "In corso…"),
    ("home.online", "Online", "Online"),
    ("home.offline", "Offline", "Non raggiungibile"),
    ("home.firmware", "Firmware", "Firmware"),
    ("home.wifi", "Wi-Fi signal", "Segnale Wi-Fi"),
    ("home.locks", "Doors and gates", "Porte e cancelli"),
    ("home.open", "Open", "Apri"),
    ("home.light", "Staircase light", "Luce scale"),
    ("home.on", "On", "Accendi"),
    ("home.off", "Off", "Spegni"),
    ("confirm.open", "Open \"{}\" now?", "Aprire \"{}\" adesso?"),
    ("confirm.yes", "Yes, open", "Sì, apri"),
    ("confirm.no", "Cancel", "Annulla"),
    ("opened", "Open command sent to \"{}\".", "Comando di apertura inviato a \"{}\"."),
    ("live.start", "Start live view", "Avvia visione dal vivo"),
    ("live.stop", "Stop", "Ferma"),
    ("live.connecting", "Connecting…", "Connessione…"),
    ("live.ended", "Ended: {}", "Terminata: {}"),
    ("live.mute", "Mute", "Muto"),
    ("live.talk", "Hold to talk", "Tieni premuto per parlare"),
    ("live.busy", "The intercom is busy (ringing or in a call).", "Il citofono è occupato (sta squillando o è in chiamata)."),
    ("events.empty", "No calls yet.", "Nessuna chiamata."),
    ("events.accepted", "Answered", "Risposta"),
    ("events.missed", "Missed", "Persa"),
    ("events.incoming", "Ringing", "Squillo"),
    ("ring.title", "Someone is at the door", "Qualcuno ha suonato"),
    ("ring.answer", "Answer", "Rispondi"),
    ("settings.language", "Language", "Lingua"),
    ("settings.autostart", "Start with the system", "Avvia all'accensione"),
    ("settings.tray", "Keep running in the tray when closed", "Resta nell'area di notifica quando chiudi"),
    ("settings.notify", "Desktop notification when someone rings", "Notifica sul desktop quando suonano"),
    ("settings.account", "Account", "Account"),
    ("tray.show", "Show", "Mostra"),
    ("tray.quit", "Quit", "Esci"),
    ("error", "Error: {}", "Errore: {}"),
];

/// Translated string; unknown keys come back unchanged.
pub fn t(lang: Lang, key: &'static str) -> &'static str {
    TABLE
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, en, it)| if lang == Lang::It { *it } else { *en })
        .unwrap_or(key)
}

/// [`t`] with the first `{}` replaced by `arg`.
pub fn tf(lang: Lang, key: &'static str, arg: &str) -> String {
    t(lang, key).replacen("{}", arg, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_translated_and_unique() {
        let mut keys = std::collections::HashSet::new();
        for (k, en, it) in TABLE {
            assert!(keys.insert(*k), "duplicate key {k}");
            assert!(!en.is_empty() && !it.is_empty(), "empty text for {k}");
            assert_eq!(en.matches("{}").count(), it.matches("{}").count(), "placeholder mismatch in {k}");
        }
    }

    #[test]
    fn formats_and_falls_back() {
        assert_eq!(tf(Lang::It, "opened", "Cancello"), "Comando di apertura inviato a \"Cancello\".");
        assert_eq!(t(Lang::En, "no.such.key"), "no.such.key");
    }
}
