# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] - unreleased

### Added
- Library for the Netatmo cloud API used by the BTicino EASYKIT WiFi kit:
  sign-in, topology, status, call history, door release, staircase light.
- WebRTC live view (H264 + Opus) with two-way audio, working with the kit's
  legacy DTLS certificate and `AES_CM` SRTP profile.
- Ring notifications from the push WebSocket; FCM client registration.
- Desktop app (egui): Home, Live, Calls and Settings tabs, tray icon,
  start with the system, single instance, English and Italian.
- CLI: `login`, `list`, `status`, `events`, `open`, `light`, `live`,
  `listen`, `test-session`.
- Protocol notes in `docs/PROTOCOL.md`.
- FCM client vendored from fcm_receiver_rs without OpenSSL and audio playback
  (pure-Rust web push decryption), so Windows builds need no OpenSSL.
