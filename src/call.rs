//! WebRTC video/audio calls with the external unit.
//!
//! Two modes, as in the official app:
//! - **live**: we send an offer on the signaling WebSocket (`/appws/`) and
//!   the device answers (`p30.g`);
//! - **answer**: the device rang and pushed its offer (`*-rtc`, over FCM on
//!   EASYKIT); we answer it (`p30.i`).
//!
//! The firmware stamps fixed payload types (Opus 111, H264 109), so the
//! media engine registers exactly those. A local audio track sending Opus
//! silence is added like the app does; without it the device keeps its
//! microphone off.

use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use rtc::interceptor::Registry;
use rtc::media::io::h26x_writer::H26xWriter;
use rtc::media::io::ogg_writer::OggWriter;
use rtc::media::io::Writer;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS};
use rtc::peer_connection::configuration::setting_engine::SettingEngineBuilder;
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::{RTCIceCandidateInit, RTCIceServer};
use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use rtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};
use webrtc::runtime::default_runtime;

use crate::protocol::{self as p, RtcSession};

/// Event sink: `(source, message)`. Used for the session log.
pub type Log = Arc<dyn Fn(&str, &Value) + Send + Sync>;

pub const OPUS_PT: u8 = 111;
pub const H264_PT: u8 = 109;
/// One 20 ms Opus frame of silence.
const OPUS_SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

pub enum Mode {
    Live { device_id: String, module_id: Option<String> },
    Answer { session: RtcSession, offer_sdp: String },
}

/// Receives every video RTP packet (payload, marker bit).
pub type VideoSink = Arc<dyn Fn(&Bytes, bool) + Send + Sync>;
/// Receives every Opus payload from the external unit.
pub type AudioSink = Arc<dyn Fn(&Bytes) + Send + Sync>;
/// Next 20 ms Opus frame to send; `None` sends silence.
pub type MicSource = Arc<dyn Fn() -> Option<Vec<u8>> + Send + Sync>;

#[derive(Clone)]
pub struct CallOptions {
    /// Where to save `video.h264`, `audio.ogg` and the SDPs; `None` saves nothing.
    pub out_dir: Option<PathBuf>,
    /// Hang up after this long (if the device has not already).
    pub max_duration: Duration,
    pub ice_servers: Vec<RTCIceServer>,
    pub on_video: Option<VideoSink>,
    pub on_audio: Option<AudioSink>,
    pub mic: Option<MicSource>,
    /// Hang up as soon as this is notified.
    pub stop: Option<Arc<Notify>>,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CallReport {
    pub session_id: Option<String>,
    pub connected_after_ms: Option<u64>,
    pub video_packets: u64,
    pub video_bytes: u64,
    pub audio_packets: u64,
    pub ended_by: String,
    pub duration_ms: u64,
}

/// Converts the `iceServers` returned by `getcredentials`.
pub fn ice_servers_from_turn(resp: &Value) -> Vec<RTCIceServer> {
    let body = if resp["body"].is_object() { &resp["body"] } else { resp };
    body["iceServers"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| RTCIceServer {
                    urls: match &s["urls"] {
                        Value::Array(u) => u.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                        Value::String(u) => vec![u.clone()],
                        _ => vec![],
                    },
                    username: s["username"].as_str().unwrap_or_default().to_owned(),
                    credential: s["credential"].as_str().unwrap_or_default().to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn codecs() -> (RTCRtpCodecParameters, RTCRtpCodecParameters) {
    let audio = RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
            rtcp_feedback: vec![],
        },
        payload_type: OPUS_PT,
    };
    let video = RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_owned(),
            rtcp_feedback: vec![],
        },
        payload_type: H264_PT,
    };
    (audio, video)
}

struct Handler {
    log: Log,
    started: Instant,
    gathered: Arc<Notify>,
    state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
    cand_tx: mpsc::UnboundedSender<String>,
    video: Arc<Mutex<Option<H26xWriter<File>>>>,
    audio: Arc<Mutex<Option<OggWriter<File>>>>,
    on_video: Option<VideoSink>,
    on_audio: Option<AudioSink>,
    stats: Arc<Stats>,
}

#[derive(Default)]
struct Stats {
    video_packets: AtomicU64,
    video_bytes: AtomicU64,
    audio_packets: AtomicU64,
    connected_ms: AtomicU64,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.notify_one();
        }
    }

    async fn on_ice_candidate(&self, event: webrtc::peer_connection::RTCPeerConnectionIceEvent) {
        // The app forwards UDP candidates only, never loopback ones.
        if let Ok(init) = event.candidate.to_json() {
            let c = init.candidate;
            if c.contains(" udp ") && !c.contains(" 127.") {
                let _ = self.cand_tx.send(c);
            }
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        (self.log)("webrtc", &json!({ "connection_state": state.to_string() }));
        if state == RTCPeerConnectionState::Connected {
            self.stats.connected_ms.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        let _ = self.state_tx.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let kind = track.kind().await;
        (self.log)("webrtc", &json!({ "track": format!("{kind}") }));
        let stats = self.stats.clone();
        if kind == RtpCodecKind::Video {
            if let Some(ssrc) = track.ssrcs().await.first().copied() {
                let pli = track.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let pkt = PictureLossIndication { sender_ssrc: 0, media_ssrc: ssrc };
                        if pli.write_rtcp(vec![Box::new(pkt)]).await.is_err() {
                            break;
                        }
                    }
                });
            }
            let w = self.video.clone();
            let sink = self.on_video.clone();
            tokio::spawn(async move {
                while let Some(ev) = track.poll().await {
                    if let TrackRemoteEvent::OnRtpPacket(pkt) = ev {
                        stats.video_packets.fetch_add(1, Ordering::Relaxed);
                        stats.video_bytes.fetch_add(pkt.payload.len() as u64, Ordering::Relaxed);
                        if let Some(sink) = &sink {
                            sink(&pkt.payload, pkt.header.marker);
                        }
                        if let Some(w) = w.lock().unwrap().as_mut() {
                            let _ = w.write_rtp(&pkt);
                        }
                    }
                }
            });
        } else {
            let w = self.audio.clone();
            let sink = self.on_audio.clone();
            tokio::spawn(async move {
                while let Some(ev) = track.poll().await {
                    if let TrackRemoteEvent::OnRtpPacket(pkt) = ev {
                        stats.audio_packets.fetch_add(1, Ordering::Relaxed);
                        if let Some(sink) = &sink {
                            sink(&pkt.payload);
                        }
                        if let Some(w) = w.lock().unwrap().as_mut() {
                            let _ = w.write_rtp(&pkt);
                        }
                    }
                }
            });
        }
    }
}

/// DTLS/SRTP settings the EASYKIT needs:
/// - its certificate has an RSA key under 2048 bits, which rtc-dtls rejects
///   by default (`ErrKeySignatureMismatch` on CertificateVerify); the SDP
///   fingerprint check stays enabled;
/// - with `SRTP_AEAD_AES_128_GCM` negotiated (webrtc-rs's first choice) the
///   handshake completes but the device never sends media, so only
///   `SRTP_AES128_CM_HMAC_SHA1_80` is offered.
fn setting_engine() -> rtc::peer_connection::configuration::setting_engine::SettingEngine {
    use rtc::dtls::extension::extension_use_srtp::SrtpProtectionProfile as P;
    SettingEngineBuilder::new()
        .with_allow_insecure_verification_algorithm(true)
        .with_srtp_protection_profiles(vec![P::Srtp_Aes128_Cm_Hmac_Sha1_80])
        .build()
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Opens `/appws/` and subscribes.
async fn connect_signaling(token: &str, log: &Log) -> crate::Result<Ws> {
    let (mut ws, _) = tokio_tungstenite::connect_async(p::SIGNALING_WS_URL)
        .await
        .map_err(|e| crate::Error::Other(format!("signaling connect: {e}")))?;
    ws.send(Message::Text(p::signaling_subscribe(token).to_string().into()))
        .await
        .map_err(|e| crate::Error::Other(format!("signaling send: {e}")))?;
    let first = tokio::time::timeout(Duration::from_secs(20), ws.next())
        .await
        .map_err(|_| crate::Error::Other("signaling subscribe timeout".into()))?;
    let v = match first {
        Some(Ok(Message::Text(t))) => serde_json::from_str::<Value>(&t).unwrap_or(Value::Null),
        other => return Err(crate::Error::Other(format!("signaling subscribe: {other:?}"))),
    };
    log("signaling<", &v);
    if v["status"] != "ok" {
        return Err(crate::Error::Other(format!("signaling subscribe rejected: {v}")));
    }
    Ok(ws)
}

async fn send(ws: &mut Ws, log: &Log, v: Value) -> crate::Result<()> {
    let mut shown = v.clone();
    if let Some(sdp) = shown["data"]["session_description"]["sdp"].as_str() {
        shown["data"]["session_description"]["sdp"] = json!(format!("<{} bytes>", sdp.len()));
    }
    log("signaling>", &shown);
    ws.send(Message::Text(v.to_string().into()))
        .await
        .map_err(|e| crate::Error::Other(format!("signaling send: {e}")))
}

/// Runs one call until the device hangs up, the connection fails or
/// `max_duration` elapses or `stop` fires. Media can be saved to `out_dir`.
pub async fn run(token: String, mode: Mode, opts: CallOptions, log: Log) -> crate::Result<CallReport> {
    let started = Instant::now();
    let other = |e: webrtc::error::Error| crate::Error::Other(format!("webrtc: {e}"));
    let (video, audio) = match &opts.out_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            (
                Some(H26xWriter::new(File::create(dir.join("video.h264"))?, false)),
                Some(OggWriter::new(File::create(dir.join("audio.ogg"))?, 48000, 2).map_err(|e| crate::Error::Other(e.to_string()))?),
            )
        }
        None => (None, None),
    };
    let (video, audio) = (Arc::new(Mutex::new(video)), Arc::new(Mutex::new(audio)));
    let save = |name: &str, text: &str| {
        if let Some(dir) = &opts.out_dir {
            let _ = std::fs::write(dir.join(name), text);
        }
    };

    let (audio_codec, video_codec) = codecs();
    let mut me = MediaEngine::default();
    me.register_codec(audio_codec.clone(), RtpCodecKind::Audio).map_err(other)?;
    me.register_codec(video_codec, RtpCodecKind::Video).map_err(other)?;
    let registry = register_default_interceptors(Registry::new(), &mut me).map_err(other)?;
    let config = RTCConfigurationBuilder::new().with_ice_servers(opts.ice_servers.clone()).build();

    let gathered = Arc::new(Notify::new());
    let (state_tx, mut state_rx) = mpsc::unbounded_channel();
    let (cand_tx, mut cand_rx) = mpsc::unbounded_channel::<String>();
    let mut pending_cands: Vec<String> = Vec::new();
    // Live: candidates may only follow the device answer ("Received ICE
    // candidate before answer", code 13). Answer mode: right away.
    let mut can_trickle = false;
    let stats = Arc::new(Stats::default());
    let handler = Arc::new(Handler {
        log: log.clone(),
        started,
        gathered: gathered.clone(),
        state_tx,
        cand_tx,
        video: video.clone(),
        audio: audio.clone(),
        on_video: opts.on_video.clone(),
        on_audio: opts.on_audio.clone(),
        stats: stats.clone(),
    });
    let runtime = default_runtime().ok_or_else(|| crate::Error::Other("no async runtime".into()))?;
    let pc = PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_media_engine(me)
        .with_setting_engine(setting_engine())
        .with_interceptor_registry(registry)
        .with_handler(handler as Arc<dyn PeerConnectionEventHandler>)
        .with_runtime(runtime)
        .with_udp_addrs(vec!["0.0.0.0:0"])
        .build()
        .await
        .map_err(other)?;

    let mut ws = connect_signaling(&token, &log).await?;

    // Local audio track (silence) and, in live mode, a receive-only video.
    let ssrc: u32 = rand_u32();
    let mic = Arc::new(
        TrackLocalStaticSample::new(
            Instant::now(),
            MediaStreamTrack::new(
                "bticino-easykit".into(),
                "ARDAMSa0".into(),
                "ARDAMSa0".into(),
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(ssrc), ..Default::default() },
                    codec: audio_codec.rtp_codec.clone(),
                    ..Default::default()
                }],
            ),
        )
        .map_err(other)?,
    );

    let mut session = match mode {
        Mode::Live { device_id, module_id } => {
            pc.add_track(mic.clone() as Arc<dyn TrackLocal>).await.map_err(other)?;
            pc.add_transceiver_from_kind(
                RtpCodecKind::Video,
                Some(RTCRtpTransceiverInit { direction: RTCRtpTransceiverDirection::Recvonly, ..Default::default() }),
            )
            .await
            .map_err(other)?;
            let offer = pc.create_offer(None).await.map_err(other)?;
            pc.set_local_description(offer).await.map_err(other)?;
            let _ = tokio::time::timeout(Duration::from_secs(5), gathered.notified()).await;
            let sdp = pc.local_description().await.map(|d| d.sdp).unwrap_or_default();
            save("sdp-local.txt", &sdp);
            let corr = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()).to_string();
            send(&mut ws, &log, p::rtc_offer(&device_id, &corr, &sdp, module_id.as_deref())).await?;
            RtcSession { session_id: String::new(), tag_id: String::new(), device_id, correlation_id: corr }
        }
        Mode::Answer { session, offer_sdp } => {
            save("sdp-remote.txt", &offer_sdp);
            pc.set_remote_description(RTCSessionDescription::offer(offer_sdp).map_err(other)?).await.map_err(other)?;
            pc.add_track(mic.clone() as Arc<dyn TrackLocal>).await.map_err(other)?;
            let answer = pc.create_answer(None).await.map_err(other)?;
            pc.set_local_description(answer).await.map_err(other)?;
            let _ = tokio::time::timeout(Duration::from_secs(5), gathered.notified()).await;
            let sdp = pc.local_description().await.map(|d| d.sdp).unwrap_or_default();
            save("sdp-local.txt", &sdp);
            send(&mut ws, &log, p::rtc_answer(&session, &sdp)).await?;
            can_trickle = true;
            session
        }
    };

    // Local audio track: microphone frames when available, otherwise
    // silence (the device keeps its own microphone off without a sender).
    let connected = Arc::new(Notify::new());
    let mic_task = {
        let mic = mic.clone();
        let connected = connected.clone();
        let source = opts.mic.clone();
        tokio::spawn(async move {
            connected.notified().await;
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            loop {
                tick.tick().await;
                let data = source.as_ref().and_then(|f| f()).map(Bytes::from).unwrap_or_else(|| Bytes::from_static(&OPUS_SILENCE));
                let s = Sample { data, duration: Duration::from_millis(20), ..Sample::new(Instant::now()) };
                if mic.sample_writer(ssrc, OPUS_PT).write_sample(&s).await.is_err() {
                    break;
                }
            }
        })
    };

    let deadline = tokio::time::sleep(opts.max_duration);
    tokio::pin!(deadline);
    let stop = opts.stop.clone().unwrap_or_default();
    let ended_by = loop {
        tokio::select! {
            _ = &mut deadline => break "timeout (we hung up)".to_owned(),
            _ = stop.notified() => break "stopped (we hung up)".to_owned(),
            Some(c) = cand_rx.recv() => {
                // Trickle ICE: the device sends none of its own and needs ours
                // on the signaling channel once the session ids are known.
                if !can_trickle {
                    pending_cands.push(c);
                } else {
                    send(&mut ws, &log, p::rtc_candidate(&session, &c, 0)).await?;
                }
            }
            st = state_rx.recv() => match st {
                Some(RTCPeerConnectionState::Connected) => connected.notify_one(),
                Some(RTCPeerConnectionState::Failed) => break "webrtc failed".to_owned(),
                Some(RTCPeerConnectionState::Closed) => break "webrtc closed".to_owned(),
                None => break "webrtc gone".to_owned(),
                _ => {}
            },
            msg = ws.next() => {
                let v = match msg {
                    Some(Ok(Message::Text(t))) => serde_json::from_str::<Value>(&t).unwrap_or(Value::Null),
                    Some(Ok(Message::Close(c))) => break format!("signaling closed {c:?}"),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => break format!("signaling error {e}"),
                    None => break "signaling ended".to_owned(),
                };
                let mut shown = v.clone();
                if let Some(sdp) = shown["data"]["session_description"]["sdp"].as_str() {
                    shown["data"]["session_description"]["sdp"] = json!(format!("<{} bytes>", sdp.len()));
                }
                log("signaling<", &shown);
                if v["type"] == "ack" {
                    // Only the offer ack carries ids; later acks have nulls.
                    if let Some(s) = v["session_id"].as_str() { session.session_id = s.to_owned(); }
                    if let Some(t) = v["tag_id"].as_str() { session.tag_id = t.to_owned(); }
                    continue;
                }
                match v["data"]["type"].as_str() {
                    Some("answer") => {
                        let sdp = v["data"]["session_description"]["sdp"].as_str().unwrap_or_default().to_owned();
                        save("sdp-remote.txt", &sdp);
                        if let Err(e) = async { pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await }.await {
                            break format!("bad answer: {e}");
                        }
                        can_trickle = true;
                        for c in std::mem::take(&mut pending_cands) {
                            send(&mut ws, &log, p::rtc_candidate(&session, &c, 0)).await?;
                        }
                    }
                    Some("candidate") => {
                        let c = &v["data"]["ice_candidate"];
                        let init = RTCIceCandidateInit {
                            candidate: c["candidate"].as_str().unwrap_or_default().to_owned(),
                            sdp_mline_index: c["sdp_m_line_index"].as_u64().map(|i| i as u16),
                            ..Default::default()
                        };
                        if let Err(e) = pc.add_ice_candidate(init).await {
                            log("webrtc", &json!({ "add_ice_candidate_error": e.to_string() }));
                        }
                    }
                    Some(t @ ("terminate" | "rescind")) => break format!("device {t}: {}", v["data"]["error"]),
                    _ => {}
                }
            }
        }
    };

    if !session.session_id.is_empty() && !ended_by.starts_with("device") {
        let _ = send(&mut ws, &log, p::rtc_terminate(&session)).await;
    }
    mic_task.abort();
    let _ = pc.close().await;
    if let Some(mut w) = video.lock().unwrap().take() {
        let _ = w.close();
    }
    if let Some(mut w) = audio.lock().unwrap().take() {
        let _ = w.close();
    }
    let connected = stats.connected_ms.load(Ordering::Relaxed);
    Ok(CallReport {
        session_id: (!session.session_id.is_empty()).then_some(session.session_id),
        connected_after_ms: (connected > 0).then_some(connected),
        video_packets: stats.video_packets.load(Ordering::Relaxed),
        video_bytes: stats.video_bytes.load(Ordering::Relaxed),
        audio_packets: stats.audio_packets.load(Ordering::Relaxed),
        ended_by,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn rand_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    h.finish() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_response_to_ice_servers() {
        let v = json!({"body":{"iceServers":[
            {"urls":["turn:a:3478?transport=udp","stun:a:3478"],"username":"u","credential":"c"},
            {"urls":"stun:b"}
        ]}});
        let s = ice_servers_from_turn(&v);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].urls.len(), 2);
        assert_eq!(s[0].credential, "c");
        assert_eq!(s[1].urls, vec!["stun:b".to_owned()]);
    }
}
