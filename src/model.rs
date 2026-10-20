//! Typed view of the `homesdata` topology.

use serde_json::Value;

/// Module families seen on BTicino door entry systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    /// EASYKIT WiFi bridge (the 7" internal monitor).
    EasykitBridge,
    /// Classe 100X / 300X bridges (`BNC1`, `BNC3`, `BNCX`).
    ClasseBridge,
    /// External unit (entrance panel with camera).
    ExternalUnit,
    /// Door lock / electric strike.
    DoorLock,
    /// Staircase light.
    StaircaseLight,
    Other,
}

impl ModuleKind {
    /// EASYKIT modules carry only `type`; Classe X modules also carry a
    /// `variant` such as `BNDL:bndl_doorlock`, which wins when present.
    pub fn from_module(type_: &str, variant: Option<&str>) -> Self {
        let key = variant.and_then(|v| v.split(':').next()).unwrap_or(type_);
        match key {
            "BDIY" => Self::EasykitBridge,
            "BNC1" | "BNC3" | "BNCX" => Self::ClasseBridge,
            "BNEU" => Self::ExternalUnit,
            "BNDL" => Self::DoorLock,
            "BNSL" => Self::StaircaseLight,
            _ => Self::Other,
        }
    }

    pub fn is_bridge(self) -> bool {
        matches!(self, Self::EasykitBridge | Self::ClasseBridge)
    }
}

#[derive(Debug, Clone)]
pub struct Module {
    pub id: String,
    pub type_: String,
    pub name: String,
    pub bridge: Option<String>,
    pub kind: ModuleKind,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct Home {
    pub id: String,
    pub name: String,
    pub timezone: Option<String>,
    pub modules: Vec<Module>,
}

impl Home {
    pub fn bridge(&self) -> Option<&Module> {
        self.modules.iter().find(|m| m.kind.is_bridge())
    }

    pub fn module(&self, id: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.id == id)
    }

    pub fn by_kind(&self, kind: ModuleKind) -> impl Iterator<Item = &Module> {
        self.modules.iter().filter(move |m| m.kind == kind)
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

/// Parses a full `homesdata` response.
pub fn parse_homes(resp: &Value) -> Vec<Home> {
    let homes = resp["body"]["homes"].as_array().cloned().unwrap_or_default();
    homes
        .iter()
        .filter_map(|h| {
            let modules = h["modules"]
                .as_array()
                .map(|ms| {
                    ms.iter()
                        .filter_map(|m| {
                            let type_ = s(m, "type").unwrap_or_default();
                            Some(Module {
                                id: s(m, "id")?,
                                kind: ModuleKind::from_module(&type_, m["variant"].as_str()),
                                name: s(m, "name").unwrap_or_default(),
                                bridge: s(m, "bridge"),
                                type_,
                                raw: m.clone(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Home {
                id: s(h, "id")?,
                name: s(h, "name").unwrap_or_default(),
                timezone: s(h, "timezone"),
                modules,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn easykit_topology_without_variant() {
        let resp = json!({"body": {"homes": [{
            "id": "h1", "name": "Casa", "timezone": "Europe/Rome",
            "modules": [
                {"id": "00:03:50:aa:bb:cc", "type": "BDIY", "name": "EasyKit"},
                {"id": "eu-1", "type": "BNEU", "bridge": "00:03:50:aa:bb:cc"},
                {"id": "dl-1", "type": "BNDL", "bridge": "00:03:50:aa:bb:cc"},
                {"id": "sl-1", "type": "BNSL", "bridge": "00:03:50:aa:bb:cc"}
            ]}]}});
        let homes = parse_homes(&resp);
        let h = &homes[0];
        assert_eq!(h.bridge().unwrap().kind, ModuleKind::EasykitBridge);
        assert_eq!(h.by_kind(ModuleKind::DoorLock).count(), 1);
        assert_eq!(h.module("sl-1").unwrap().kind, ModuleKind::StaircaseLight);
    }

    #[test]
    fn variant_wins_over_type() {
        assert_eq!(
            ModuleKind::from_module("BNEU", Some("BNDL:bndl_doorlock")),
            ModuleKind::DoorLock
        );
    }
}
