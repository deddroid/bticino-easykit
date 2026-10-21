//! Unofficial client library for the BTicino EASYKIT WiFi video door
//! entry kit (and other BTicino intercoms on the Netatmo cloud).

pub mod actions;
pub mod call;
pub mod client;
pub mod i18n;
pub mod model;
pub mod protocol;
pub mod push;
pub mod settings;
pub mod video;

pub use client::{Client, Error, Result};
pub use model::{Home, Module, ModuleKind};
