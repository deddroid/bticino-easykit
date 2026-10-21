# Changes from upstream fcm_receiver_rs 0.1.1

Upstream: https://github.com/agusibrahim/fcm_receiver.rs (MIT OR Apache-2.0),
copyright Agus Ibrahim. Vendored into bticino-easykit and modified:

- removed the `fun` module and the `rodio` dependency (sound playback);
- MCS socket: `rustls` with webpki roots instead of `native-tls`;
- `ece` without its OpenSSL backend; `src/ece_backend.rs` provides a
  RustCrypto backend (validated with `ece::crypto::test_cryptographer`);
- reqwest 0.12 with rustls; unused dependencies dropped.
