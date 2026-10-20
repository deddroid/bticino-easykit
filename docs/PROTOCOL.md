# BTicino EASYKIT WiFi protocol notes

Consolidated from the projects listed under [Sources](#sources). Items are
tagged **[verified]** once confirmed on the maintainer's own 310913 kit,
**[others]** when confirmed on real hardware by another project, and
**[unverified]** otherwise.

## Device family

The EASYKIT WiFi (BTicino 310913, Legrand 360910) has no local API, USB or
HID interface. The 7" internal monitor connects to the Netatmo cloud over
WiFi and is controlled through the same backend as the official
*Home + Security* app (`com.netatmo.camera`).

| `type` | Role | Notes |
|--------|------|-------|
| `BDIY` | Bridge (internal monitor) | id is a MAC `00:03:50:…`; fw `1.00.11`, hw 199 **[verified]** |
| `BNEU` | External unit (entrance panel, camera) | **[verified]** |
| `BNDL` | Door lock / electric strike (up to 2; the second may report `appliance_type: "gate"`) | **[verified]** |
| `BNSL` | Staircase light / auxiliary relay | **[others]** |

Unlike Classe 100X/300X (`BNC1`/`BNC3`/`BNCX`), EASYKIT modules have **no
`variant` field in `homesdata`**, so the role must be taken from `type`
(bticino_intercom #76). `homestatus` *does* report `variant`
(`BNEU:bneu_external_unit`, `BNDL:bndl_doorlock`) **[verified]**. Firmware
(`firmware_name`, `firmware_revision`), `local_ipv4`, `wifi_strength` and
`uptime` are only in `homestatus` **[verified]**. `BNDL` status has no `lock`
field. **[verified]**

## Authentication

`POST https://app.netatmo.net/oauth2/token`, form-encoded:

| field | value |
|-------|-------|
| grant_type | `password` (or `refresh_token`) |
| username / password | Home + Security account |
| client_id | `na_client_android_welcome` |
| client_secret | embedded in the Android app |
| scope | `security_scopes` |
| app_version | `4.1.1.3` |

Password and refresh grants **[verified]**. Responses carry `access_token`, `refresh_token`, `expires_in` (≈10800 s).
REST calls use `Authorization: Bearer …` and the app User-Agent
`NetatmoApp(Security/v4.1.1.3/4401103) Android(13/…)`. An expired token
yields HTTP 403; refresh and retry once.

### Events on EASYKIT **[verified]**

Calls are `type: "outdoor"` events on the bridge, with one subevent
(`accepted_call`, `missed_call`, …) carrying `session_id`, `message`, and
`snapshot`/`vignette` URLs with `expires_at`. `video_status` is `deleted`
when no clip is kept. Bridge online/offline arrive as `connection` /
`disconnection`.

## REST endpoints

All are JSON `POST`s on `https://app.netatmo.net`.

| path | body | purpose |
|------|------|---------|
| `/api/homesdata` | `{app_type:"app_camera", app_version, sync_measurements:false}` | topology **[verified]** |
| `/syncapi/v1/homestatus` | `{app_type, app_version, home_id}` | live status **[verified]** |
| `/syncapi/v1/setstate` | see below | commands |
| `/syncapi/v1/getconfigs` | `{module_ids:[…], home_id, app_identifier:"app_camera"}` | module config |
| `/api/getevents` | `{app_type, app_version, home_id, size}` | call history **[verified]** |
| `https://app-turn.netatmo.net/api/getcredentials` | `{client_type:"user"}` | TURN servers for WebRTC |

### setstate

```json
{"app_type":"app_camera","app_version":"4.1.1.3",
 "home":{"id":"<home>","timezone":"Europe/Rome",
         "modules":[{"id":"<module>","bridge":"<bridge mac>", "lock":false}]}}
```

- Door release: `BNDL` + `{"lock": false}` (relocks by itself) **[others]**
- Staircase light: `BNSL` + `{"on": true|false}` **[others]**
- Success: `{"status":"ok", …}`.

## Push WebSocket

`wss://app-ws.netatmo.net/ws/`, first message:

```json
{"action":"Subscribe","access_token":"…","app_type":"app_camera","platform":"Android","version":"4.1.1.3"}
```

Never send a `filter` field (it suppresses call events) and no
application-level pings (the server drops the link after ~10 min).
Re-subscribe with a fresh token before it expires.

Events arrive with `push_type = "<bridge type>-<event>"`. On EASYKIT:

| push_type | |
|-----------|-|
| `BDIY-incoming_call` | doorbell rang; `snapshot_url`, `vignette_url`, `session_id` **[others]** |
| `BDIY-missed_call` / `BDIY-accepted_call` | **[others]** |
| `BDIY-connection` / `BDIY-disconnection` | bridge online/offline **[unverified]** |
| `BDIY-rtc` | **never received on EASYKIT** (bticino_intercom #72) |

## Call pushes over FCM (the EASYKIT key finding)

From the official app 26.10.1.1, decompiled with REA + JADX:

- The app subscribes to the push WebSocket with `"filter": "silent"`
  (`v10.v.onSuccess`), so it does **not** get calls from the WebSocket.
- Calls come from **Firebase Cloud Messaging**. `fo0.e.t` registers one
  handler (`kf1.a`) for `BNC1|BDIY|BNC3` × `-rtc`, `-incoming_call`,
  `-missed_call`. The FCM data keys are `push_type`, `extra_params` (JSON,
  same shape as the WebSocket `extra_params`) and `alert`.
- `p30.v.a` parses `extra_params.data` as an RTC message: `offer` (stores
  `session_id` + `tag_id` per `device_id`, needs `session_description.sdp`),
  `rescind`, `terminate` (optional `error.code` / `error.message`).
- So on EASYKIT the `BDIY-rtc` offer is sent **only to FCM tokens**, which
  explains bticino_intercom #72.

Firebase project of the app (`res/values/strings.xml`): project
`security-7228d`, sender `514810499773`, app id
`1:514810499773:android:ed5e6854097137cb`, package `com.netatmo.camera`,
signing cert SHA-1 `cdaf5d982a79b76743546599a87f7c45bac269d8`.

Registration (`u10.d.d` / `u10.d.k`), JSON POST:

```json
/api/addpushcontext  {"credentials":"{\"device_token\":\"<fcm token>\"}","device_version":"…","os_version":"…","type":"Android","app_version":"…","app_type":"app_camera"}
/api/droppushcontext {"credentials":"{\"device_token\":\"<fcm token>\"}","type":"Android"}
```

Status: **[unverified]** until a ring is received with `bticino-easykit listen`.

## WebRTC video/audio

Signaling on `wss://app-ws.netatmo.net/appws/` (`app_type: app_security`),
documented in pybticino `docs/webrtc-signaling.md`. On Classe X the device
sends a `…-rtc` offer on ring and the client answers. **On EASYKIT this
offer never arrives**, and an on-demand offer from the client reportedly
streams for ~10 s only. How the official app gets EASYKIT call video is
explained above: the offer goes to FCM. Answering it should follow the
Classe X answer flow on `/appws/` with the pushed `session_id`/`tag_id`.
**[unverified]**

## Sources

- [k-the-hidden-hero/pybticino](https://github.com/k-the-hidden-hero/pybticino) — API, push and signaling, notes from the decompiled app
- [k-the-hidden-hero/bticino_intercom](https://github.com/k-the-hidden-hero/bticino_intercom) — Home Assistant integration; EASYKIT reports in issues #72 and #76
- Official *Home + Security* Android app 26.10.1.1 (`com.netatmo.camera`), analysed with [REA](https://github.com/morluto/rea) + jadx-headless-mcp and apktool
- [agusibrahim/fcm_receiver.rs](https://github.com/agusibrahim/fcm_receiver.rs) — Android FCM client emulation
- [jabesq-org/pyatmo](https://github.com/jabesq-org/pyatmo) — Netatmo module types
