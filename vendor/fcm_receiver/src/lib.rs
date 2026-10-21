//! Vendored third-party code (see CHANGES.md): lints are not enforced here.
#![allow(deprecated, clippy::all)]

pub mod client;
pub mod consts;
pub mod crypto;
pub mod ece_backend;
pub mod encryption;
pub mod http_client;
pub mod messages;
pub mod proto;
pub mod socket_handler;
pub mod util;

mod error;

pub use error::{Error, ProtoError, ProtoErrorKind, Result};
