//! Actions shared by the CLI and the GUI, plus a background [`Worker`]
//! that runs them off the UI thread and reports back through a channel.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Notify;

use crate::call::{self, CallOptions, CallReport, Mode};
use crate::model::{Home, ModuleKind};
use crate::push::{self, Push};
use crate::{protocol as p, Client, Error, Result};

/// The first home that has an intercom bridge.
pub fn default_home(c: &mut Client) -> Result<Home> {
    c.homes()?
        .into_iter()
        .find(|h| h.bridge().is_some())
        .ok_or_else(|| Error::Other("no BTicino intercom found on this account".into()))
}

/// Opens a door lock (`BNDL`); the device relocks by itself.
pub fn unlock(c: &mut Client, home: &Home, module_id: &str) -> Result<Value> {
    match home.module(module_id).map(|m| m.kind) {
        Some(ModuleKind::DoorLock) => c.set_state(home, module_id, p::unlock_state()),
        _ => Err(Error::Other(format!("{module_id} is not a door lock"))),
    }
}

/// Switches a staircase light (`BNSL`).
pub fn light(c: &mut Client, home: &Home, module_id: &str, on: bool) -> Result<Value> {
    c.set_state(home, module_id, p::light_state(on))
}

/// Live view parameters for the home's external unit.
pub fn live_mode(home: &Home) -> Result<Mode> {
    let device_id = home.bridge().ok_or_else(|| Error::Other("no bridge".into()))?.id.clone();
    let module_id = home.by_kind(ModuleKind::ExternalUnit).next().map(|m| m.id.clone());
    Ok(Mode::Live { device_id, module_id })
}

/// Staircase lights, including the ones hidden in the app (`visible: false`
/// in `getconfigs`), which `homesdata` leaves out.
pub fn staircase_lights(c: &mut Client, home: &Home) -> Result<Vec<(String, String)>> {
    let ids: Vec<&str> = home.modules.iter().map(|m| m.id.as_str()).collect();
    let v = c.configs(&home.id, &ids)?;
    Ok(v["body"]["home"]["modules"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|m| m["type"] == "BNSL")
                .filter_map(|m| Some((m["id"].as_str()?.to_owned(), m["name"].as_str().unwrap_or("").to_owned())))
                .collect()
        })
        .unwrap_or_default())
}

pub enum Command {
    Login { email: String, password: String },
    Logout,
    Refresh,
    Unlock(String),
    Light(String, bool),
    StartLive,
    StopLive,
    /// Mute the external unit's audio on the speakers.
    Mute(bool),
    /// Push-to-talk: send the microphone instead of silence.
    Talk(bool),
}

pub enum Update {
    LoggedIn(bool),
    Home(Box<Home>, Value),
    Lights(Vec<(String, String)>),
    Events(Value),
    Info(String),
    Error(String),
    LiveState(String),
    #[cfg(feature = "gui")]
    Frame(crate::video::RgbaFrame),
    LiveEnded(std::result::Result<CallReport, String>),
    Ring(Push),
}

/// Owns the API client on its own thread; never blocks the caller.
pub struct Worker {
    tx: Sender<Command>,
    pub rx: Receiver<Update>,
}

impl Worker {
    /// `wake` is called after every update (e.g. to request a repaint).
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, crx) = channel::<Command>();
        let (utx, rx) = channel::<Update>();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        std::thread::Builder::new()
            .name("easykit-worker".into())
            .spawn(move || run(crx, utx, wake))
            .expect("spawn worker");
        Self { tx, rx }
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }
}

struct Ctx {
    out: Sender<Update>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Ctx {
    fn send(&self, u: Update) {
        let _ = self.out.send(u);
        (self.wake)();
    }
}

fn run(rx: Receiver<Command>, out: Sender<Update>, wake: Arc<dyn Fn() + Send + Sync>) {
    let ctx = Arc::new(Ctx { out, wake });
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return ctx.send(Update::Error(e.to_string())),
    };
    let mut client = match Client::new() {
        Ok(c) => c,
        Err(e) => return ctx.send(Update::Error(e.to_string())),
    };
    let mut home: Option<Home> = None;
    let mut live_stop: Option<Arc<Notify>> = None;
    #[cfg(feature = "gui")]
    let mut audio: Option<Arc<crate::audio::Audio>> = None;
    let mut watching = false;

    ctx.send(Update::LoggedIn(client.is_logged_in()));
    let mut pending = if client.is_logged_in() { vec![Command::Refresh] } else { vec![] };

    loop {
        let cmd = match pending.pop() {
            Some(c) => c,
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            },
        };
        let r: Result<()> = (|| {
            match cmd {
                Command::Login { email, password } => {
                    client.login(&email, &password)?;
                    ctx.send(Update::LoggedIn(true));
                    pending.push(Command::Refresh);
                }
                Command::Logout => {
                    client.logout()?;
                    home = None;
                    ctx.send(Update::LoggedIn(false));
                }
                Command::Refresh => {
                    let h = default_home(&mut client)?;
                    let status = client.homestatus(&h.id)?;
                    ctx.send(Update::Home(Box::new(h.clone()), status));
                    if let Ok(l) = staircase_lights(&mut client, &h) {
                        ctx.send(Update::Lights(l));
                    }
                    ctx.send(Update::Events(client.events(&h.id, 30)?));
                    if !watching {
                        watching = true;
                        watch_rings(&rt, ctx.clone());
                    }
                    home = Some(h);
                }
                Command::Unlock(id) => {
                    let h = home.as_ref().ok_or_else(|| Error::Other("not loaded".into()))?;
                    unlock(&mut client, h, &id)?;
                    let name = h.module(&id).map(|m| m.name.clone()).unwrap_or(id);
                    ctx.send(Update::Info(format!("unlock:{name}")));
                }
                Command::Light(id, on) => {
                    let h = home.as_ref().ok_or_else(|| Error::Other("not loaded".into()))?;
                    light(&mut client, h, &id, on)?;
                    ctx.send(Update::Info(format!("light:{on}")));
                }
                Command::StartLive => {
                    if let Some(s) = live_stop.take() {
                        s.notify_one();
                    }
                    let h = home.as_ref().ok_or_else(|| Error::Other("not loaded".into()))?;
                    let mode = live_mode(h)?;
                    let ice = call::ice_servers_from_turn(&client.turn()?);
                    let token = client.token()?;
                    let stop = Arc::new(Notify::new());
                    live_stop = Some(stop.clone());
                    #[cfg(feature = "gui")]
                    {
                        if let Some(a) = audio.take() {
                            a.stop();
                        }
                        audio = Some(crate::audio::Audio::start());
                    }
                    #[cfg(feature = "gui")]
                    let a = audio.clone();
                    #[cfg(not(feature = "gui"))]
                    let a = None;
                    start_live(&rt, token, mode, ice, stop, a, ctx.clone());
                }
                Command::StopLive => {
                    if let Some(s) = live_stop.take() {
                        s.notify_one();
                    }
                    #[cfg(feature = "gui")]
                    if let Some(a) = audio.take() {
                        a.stop();
                    }
                }
                Command::Mute(_m) => {
                    #[cfg(feature = "gui")]
                    if let Some(a) = &audio {
                        a.muted.store(_m, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Command::Talk(_t) => {
                    #[cfg(feature = "gui")]
                    if let Some(a) = &audio {
                        a.talking.store(_t, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            ctx.send(Update::Error(e.to_string()));
        }
    }
}

/// Doorbell events from the push WebSocket.
fn watch_rings(rt: &tokio::runtime::Runtime, ctx: Arc<Ctx>) {
    // The listener reads fresh tokens through its own client (they are
    // shared through tokens.toml); blocking HTTP stays off the async workers.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Push>();
    rt.spawn(async move {
        loop {
            let tx = tx.clone();
            let token = || tokio::task::block_in_place(|| Client::new().and_then(|mut c| c.token()));
            let r = push::listen_ws(token, tx).await;
            log::warn!("push WebSocket ended: {r:?}; reconnecting");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
    rt.spawn(async move {
        while let Some(p) = rx.recv().await {
            if p.push_type.ends_with("-incoming_call") {
                ctx.send(Update::Ring(p));
            }
        }
    });
}

fn start_live(
    rt: &tokio::runtime::Runtime,
    token: String,
    mode: Mode,
    ice: Vec<rtc::peer_connection::transport::RTCIceServer>,
    stop: Arc<Notify>,
    #[cfg(feature = "gui")] audio: Option<Arc<crate::audio::Audio>>,
    #[cfg(not(feature = "gui"))] audio: Option<()>,
    ctx: Arc<Ctx>,
) {
    #[cfg(feature = "gui")]
    let on_video = {
        let (ftx, frx) = channel::<(bytes::Bytes, bool)>();
        let fctx = ctx.clone();
        std::thread::spawn(move || {
            let mut asm = crate::video::FrameAssembler::default();
            let Ok(mut dec) = crate::video::Decoder::new() else { return };
            while let Ok((payload, marker)) = frx.recv() {
                if let Some(au) = asm.push(&payload, marker) {
                    if let Some(f) = dec.decode(&au) {
                        fctx.send(Update::Frame(f));
                    }
                }
            }
        });
        let ftx = std::sync::Mutex::new(ftx);
        Some(Arc::new(move |b: &bytes::Bytes, m: bool| {
            let _ = ftx.lock().unwrap().send((b.clone(), m));
        }) as call::VideoSink)
    };
    #[cfg(not(feature = "gui"))]
    let on_video = None;

    #[cfg(feature = "gui")]
    let audio_end = audio.clone();
    #[cfg(feature = "gui")]
    let (on_audio, mic) = match audio {
        Some(a) => {
            let (play, talk) = (a.clone(), a);
            (
                Some(Arc::new(move |b: &bytes::Bytes| play.play(b)) as call::AudioSink),
                Some(Arc::new(move || talk.next_mic_frame()) as call::MicSource),
            )
        }
        None => (None, None),
    };
    #[cfg(not(feature = "gui"))]
    let (on_audio, mic) = {
        let _ = audio;
        (None, None)
    };

    let lctx = ctx.clone();
    let log: call::Log = Arc::new(move |_src, v| {
        if let Some(s) = v["connection_state"].as_str() {
            lctx.send(Update::LiveState(s.to_owned()));
        }
    });
    let opts = CallOptions {
        out_dir: None,
        max_duration: Duration::from_secs(10 * 60),
        ice_servers: ice,
        on_video,
        on_audio,
        mic,
        stop: Some(stop),
    };
    ctx.send(Update::LiveState("starting".into()));
    rt.spawn(async move {
        let r = call::run(token, mode, opts, log).await.map_err(|e| e.to_string());
        #[cfg(feature = "gui")]
        if let Some(a) = audio_end {
            a.stop();
        }
        ctx.send(Update::LiveEnded(r));
    });
}
