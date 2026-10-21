//! The egui application: Home, Live, Calls and Settings tabs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bticino_easykit::actions::{Command, Update, Worker};
use bticino_easykit::i18n::{t, tf, Lang};
use bticino_easykit::settings::Settings;
use bticino_easykit::{Home, ModuleKind};
use eframe::egui::{self, Color32, RichText};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Home,
    Live,
    Events,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Home, Tab::Live, Tab::Events, Tab::Settings];

    fn key(self) -> &'static str {
        match self {
            Tab::Home => "tab.home",
            Tab::Live => "tab.live",
            Tab::Events => "tab.events",
            Tab::Settings => "tab.settings",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Tab::Home => "home",
            Tab::Live => "live",
            Tab::Events => "calls",
            Tab::Settings => "settings",
        }
    }
}

/// State that survives closing the window to the tray.
pub struct Shared {
    pub worker: Option<Worker>,
    pub ctx: Option<egui::Context>,
    pub state: State,
    pub quit: bool,
}

/// Everything the UI shows, fed by worker updates.
#[derive(Default)]
pub struct State {
    pub logged_in: Option<bool>,
    pub home: Option<Home>,
    pub status: Value,
    pub lights: Vec<(String, String)>,
    pub events: Value,
    pub message: Option<(String, bool)>,
    pub live_state: String,
    pub live_running: bool,
    pub frame: Option<(usize, usize, Vec<u8>, u64)>,
    pub ring: Option<(Value, Instant)>,
    pub can_answer: bool,
    pub busy: bool,
}

pub type SharedRef = Arc<Mutex<Shared>>;
/// Called when someone rings (desktop notification).
pub type OnRing = Arc<dyn Fn(&Value, Lang) + Send + Sync>;

/// Applies one worker update; returns `true` for a doorbell ring.
pub fn apply(state: &mut State, u: Update, lang: Lang) -> bool {
    match u {
        Update::LoggedIn(b) => {
            state.logged_in = Some(b);
            state.busy = false;
            if !b {
                state.home = None;
            }
        }
        Update::Home(h, s) => {
            state.home = Some(*h);
            state.status = s;
            state.busy = false;
        }
        Update::Lights(l) => state.lights = l,
        Update::Events(e) => state.events = e,
        Update::Info(i) => {
            state.busy = false;
            let msg = match i.split_once(':') {
                Some(("unlock", name)) => tf(lang, "opened", name),
                _ => i,
            };
            state.message = Some((msg, false));
        }
        Update::Error(e) => {
            state.busy = false;
            state.message = Some((tf(lang, "error", &e), true));
        }
        Update::LiveState(s) => state.live_state = s,
        Update::Frame(f) => {
            let n = state.frame.as_ref().map(|f| f.3 + 1).unwrap_or(0);
            state.frame = Some((f.width, f.height, f.rgba, n));
        }
        Update::LiveEnded(r) => {
            state.live_running = false;
            state.live_state = match r {
                Ok(rep) if rep.ended_by.contains("Max number of peers") => t(lang, "live.busy").to_owned(),
                Ok(rep) => tf(lang, "live.ended", &rep.ended_by),
                Err(e) => tf(lang, "error", &e),
            };
        }
        Update::Ring(p) => {
            state.ring = Some((p.extra_params, Instant::now()));
            return true;
        }
        Update::Offer => state.can_answer = true,
        Update::CallGone => {
            state.can_answer = false;
            state.ring = None;
        }
    }
    false
}

pub struct App {
    shared: SharedRef,
    settings: Settings,
    tab: Tab,
    email: String,
    password: String,
    confirm: Option<(String, String)>,
    texture: Option<(egui::TextureHandle, u64)>,
    muted: bool,
    talking: bool,
    vignettes: HashMap<String, VignetteState>,
    vignette_tx: std::sync::mpsc::Sender<(String, Option<egui::ColorImage>)>,
    vignette_rx: std::sync::mpsc::Receiver<(String, Option<egui::ColorImage>)>,
    shots: Option<Screenshots>,
    pub on_ring: OnRing,
}

enum VignetteState {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

/// `BTICINO_EASYKIT_SCREENSHOTS=<dir>`: shoot every tab in both languages, then quit.
struct Screenshots {
    dir: PathBuf,
    plan: Vec<(Tab, Lang)>,
    frames: u32,
    pending: bool,
    image: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: SharedRef, on_ring: OnRing) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        shared.lock().unwrap().ctx = Some(cc.egui_ctx.clone());
        let settings = Settings::load();
        let (vignette_tx, vignette_rx) = std::sync::mpsc::channel();
        let shots = std::env::var_os("BTICINO_EASYKIT_SCREENSHOTS").map(|d| Screenshots {
            dir: d.into(),
            plan: Lang::ALL.iter().flat_map(|l| Tab::ALL.iter().map(move |t| (*t, *l))).collect(),
            frames: 0,
            pending: false,
            image: Arc::default(),
        });
        Self {
            shared,
            settings,
            tab: Tab::Home,
            email: String::new(),
            password: String::new(),
            confirm: None,
            texture: None,
            muted: false,
            talking: false,
            vignettes: HashMap::new(),
            vignette_tx,
            vignette_rx,
            shots,
            on_ring,
        }
    }

    fn lang(&self) -> Lang {
        self.settings.lang
    }

    fn send(&self, c: Command) {
        let mut s = self.shared.lock().unwrap();
        s.state.busy = true;
        if let Some(w) = &s.worker {
            w.send(c);
        }
    }

    /// Like [`Self::send`] without the busy spinner (instant toggles).
    fn send_quiet(&self, c: Command) {
        if let Some(w) = &self.shared.lock().unwrap().worker {
            w.send(c);
        }
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let mut rings = Vec::new();
        {
            let mut s = self.shared.lock().unwrap();
            let updates: Vec<Update> = match &s.worker {
                Some(w) => w.rx.try_iter().collect(),
                None => Vec::new(),
            };
            for u in updates {
                if apply(&mut s.state, u, lang) {
                    rings.push(s.state.ring.as_ref().map(|r| r.0.clone()).unwrap_or_default());
                }
            }
        }
        for r in rings {
            if self.settings.notify_on_ring {
                (self.on_ring)(&r, lang);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.tab = Tab::Home;
        }
        while let Ok((url, img)) = self.vignette_rx.try_recv() {
            let v = match img {
                Some(img) => VignetteState::Ready(ctx.load_texture(&url, img, egui::TextureOptions::LINEAR)),
                None => VignetteState::Failed,
            };
            self.vignettes.insert(url, v);
        }
    }

    fn vignette(&mut self, url: &str) -> Option<egui::TextureHandle> {
        match self.vignettes.get(url) {
            Some(VignetteState::Ready(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                self.vignettes.insert(url.to_owned(), VignetteState::Loading);
                let (tx, url) = (self.vignette_tx.clone(), url.to_owned());
                let ctx = self.shared.lock().unwrap().ctx.clone();
                std::thread::spawn(move || {
                    let img = fetch_image(&url);
                    let _ = tx.send((url, img));
                    if let Some(c) = ctx {
                        c.request_repaint();
                    }
                });
                None
            }
        }
    }

    fn screenshots(&mut self, ctx: &egui::Context) {
        let Some(sh) = &mut self.shots else { return };
        let shot = sh.image.lock().unwrap().take();
        if let Some(img) = shot {
            if let Some((tab, lang)) = sh.plan.first().copied() {
                let _ = std::fs::create_dir_all(&sh.dir);
                let path = sh.dir.join(format!("{}-{}.png", tab.slug(), if lang == Lang::It { "it" } else { "en" }));
                let [w, h] = img.size;
                let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let _ = image::save_buffer(&path, &raw, w as u32, h as u32, image::ColorType::Rgba8);
                eprintln!("screenshot {}", path.display());
                sh.plan.remove(0);
            }
            sh.pending = false;
            sh.frames = 0;
        }
        match sh.plan.first().copied() {
            None => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Some((tab, lang)) => {
                self.tab = tab;
                self.settings.lang = lang;
                sh.frames += 1;
                if sh.frames > 5 && !sh.pending {
                    sh.pending = true;
                    let (slot, c) = (sh.image.clone(), ctx.clone());
                    ctx.request_screenshot(move |img| {
                        *slot.lock().unwrap() = Some(img);
                        c.request_repaint();
                    });
                }
                ctx.request_repaint();
            }
        }
    }
}

fn fetch_image(url: &str) -> Option<egui::ColorImage> {
    let bytes = reqwest::blocking::get(url).ok()?.bytes().ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain(ctx);
        self.screenshots(ctx);
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.settings.tray {
            self.shared.lock().unwrap().quit = true;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let lang = self.lang();
        let ctx = ui.ctx().clone();
        let logged_in = self.shared.lock().unwrap().state.logged_in;

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(t(lang, "app.title"));
                ui.separator();
                if logged_in == Some(true) {
                    for tab in Tab::ALL {
                        ui.selectable_value(&mut self.tab, tab, t(lang, tab.key()));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.shared.lock().unwrap().state.busy {
                        ui.spinner();
                    }
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            let msg = self.shared.lock().unwrap().state.message.clone();
            match msg {
                Some((m, true)) => ui.colored_label(Color32::from_rgb(220, 80, 80), m),
                Some((m, false)) => ui.label(m),
                None => ui.label(""),
            };
        });

        egui::CentralPanel::default().show(ui, |ui| match logged_in {
            None => {
                ui.spinner();
            }
            Some(false) => self.login(ui),
            Some(true) => {
                self.ring_banner(ui);
                match self.tab {
                    Tab::Home => self.home(ui),
                    Tab::Live => self.live(ui),
                    Tab::Events => self.events(ui),
                    Tab::Settings => self.settings_tab(ui),
                }
            }
        });

        if let Some((id, name)) = self.confirm.clone() {
            let r = egui::Modal::new(egui::Id::unique("confirm-open")).show(&ctx, |ui| {
                ui.label(RichText::new(tf(lang, "confirm.open", &name)).size(18.0));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(t(lang, "confirm.yes")).clicked() {
                        self.send(Command::Unlock(id.clone()));
                        self.confirm = None;
                    }
                    if ui.button(t(lang, "confirm.no")).clicked() {
                        self.confirm = None;
                    }
                });
            });
            if r.should_close() {
                self.confirm = None;
            }
        }
    }
}

impl App {
    fn login(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.heading(t(lang, "login.title"));
            ui.add_space(12.0);
            egui::Grid::new("login").num_columns(2).show(ui, |ui| {
                ui.label(t(lang, "login.email"));
                ui.text_edit_singleline(&mut self.email);
                ui.end_row();
                ui.label(t(lang, "login.password"));
                let pw = ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                ui.end_row();
                let enter = pw.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button(t(lang, "login.button")).clicked() || enter) && !self.email.is_empty() {
                    let password = std::mem::take(&mut self.password);
                    self.send(Command::Login { email: self.email.trim().to_owned(), password });
                }
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.weak(t(lang, "login.note"));
        });
    }

    fn ring_banner(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let (ring, can_answer, home) = {
            let s = self.shared.lock().unwrap();
            (s.state.ring.clone(), s.state.can_answer, s.state.home.clone())
        };
        if let Some((params, at)) = ring {
            if at.elapsed() > Duration::from_secs(60) {
                return;
            }
            egui::Frame::new().fill(Color32::from_rgb(46, 157, 91)).corner_radius(6.0).inner_margin(10.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(url) = params["vignette_url"].as_str().or(params["snapshot_url"].as_str()) {
                        if let Some(tex) = self.vignette(url) {
                            ui.add(egui::Image::new(&tex).max_height(90.0));
                        }
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(t(lang, "ring.title")).size(20.0).color(Color32::WHITE));
                        ui.horizontal_wrapped(|ui| {
                            if can_answer && ui.button(format!("📞 {}", t(lang, "ring.answer"))).clicked() {
                                {
                                    let mut s = self.shared.lock().unwrap();
                                    s.state.can_answer = false;
                                    s.state.live_running = true;
                                    s.state.frame = None;
                                }
                                self.texture = None;
                                self.tab = Tab::Live;
                                self.send(Command::Answer);
                            }
                            for m in home.iter().flat_map(|h| h.by_kind(ModuleKind::DoorLock)) {
                                if ui.button(format!("🔓 {}", m.name)).clicked() {
                                    self.confirm = Some((m.id.clone(), m.name.clone()));
                                }
                            }
                        });
                    });
                });
            });
            ui.add_space(8.0);
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let (home, status, lights) = {
            let s = self.shared.lock().unwrap();
            (s.state.home.clone(), s.state.status.clone(), s.state.lights.clone())
        };
        let Some(home) = home else {
            ui.spinner();
            return;
        };
        ui.horizontal(|ui| {
            ui.heading(&home.name);
            if ui.button(format!("⟳ {}", t(lang, "refresh"))).clicked() {
                self.send(Command::Refresh);
            }
        });
        let mods = status["body"]["home"]["modules"].as_array().cloned().unwrap_or_default();
        if let Some(b) = home.bridge() {
            let st = mods.iter().find(|m| m["id"] == b.id.as_str()).cloned().unwrap_or_default();
            let online = st["reachable"].as_bool().unwrap_or(false);
            egui::Grid::new("bridge").num_columns(2).show(ui, |ui| {
                ui.label(&b.name);
                ui.colored_label(
                    if online { Color32::from_rgb(46, 157, 91) } else { Color32::from_rgb(220, 80, 80) },
                    format!("● {}", t(lang, if online { "home.online" } else { "home.offline" })),
                );
                ui.end_row();
                ui.label(t(lang, "home.firmware"));
                ui.label(st["firmware_name"].as_str().unwrap_or("-"));
                ui.end_row();
                ui.label(t(lang, "home.wifi"));
                ui.label(st["wifi_strength"].as_u64().map(|w| format!("{w}")).unwrap_or_else(|| "-".into()));
                ui.end_row();
            });
        }
        ui.add_space(12.0);
        ui.heading(t(lang, "home.locks"));
        for m in home.by_kind(ModuleKind::DoorLock) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&m.name).size(16.0));
                if ui.add(egui::Button::new(RichText::new(format!("🔓 {}", t(lang, "home.open"))).size(16.0))).clicked() {
                    self.confirm = Some((m.id.clone(), m.name.clone()));
                }
            });
        }
        if !lights.is_empty() {
            ui.add_space(12.0);
            ui.heading(t(lang, "home.light"));
            for (id, name) in lights {
                ui.horizontal(|ui| {
                    ui.label(&name);
                    if ui.button(t(lang, "home.on")).clicked() {
                        self.send(Command::Light(id.clone(), true));
                    }
                    if ui.button(t(lang, "home.off")).clicked() {
                        self.send(Command::Light(id.clone(), false));
                    }
                });
            }
        }
    }

    fn live(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let (running, state, frame) = {
            let s = self.shared.lock().unwrap();
            (s.state.live_running, s.state.live_state.clone(), s.state.frame.clone())
        };
        ui.horizontal(|ui| {
            if running {
                if ui.button(format!("■ {}", t(lang, "live.stop"))).clicked() {
                    self.send(Command::StopLive);
                }
            } else if ui.button(format!("▶ {}", t(lang, "live.start"))).clicked() {
                let mut s = self.shared.lock().unwrap();
                s.state.live_running = true;
                s.state.frame = None;
                drop(s);
                self.texture = None;
                self.send(Command::StartLive);
            }
            if running && state != "connected" {
                ui.spinner();
                ui.label(t(lang, "live.connecting"));
            } else if !running {
                ui.label(&state);
            }
            if running {
                ui.separator();
                if ui.toggle_value(&mut self.muted, format!("🔇 {}", t(lang, "live.mute"))).changed() {
                    self.send_quiet(Command::Mute(self.muted));
                }
                let talk = ui.add(egui::Button::new(format!("🎤 {}", t(lang, "live.talk"))).selected(self.talking));
                let down = talk.is_pointer_button_down_on();
                if down != self.talking {
                    self.talking = down;
                    self.send_quiet(Command::Talk(down));
                }
            }
        });
        if let Some((w, h, rgba, n)) = frame {
            if self.texture.as_ref().map(|t| t.1) != Some(n) {
                let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                match &mut self.texture {
                    Some((tex, seq)) => {
                        tex.set(img, egui::TextureOptions::LINEAR);
                        *seq = n;
                    }
                    None => self.texture = Some((ui.ctx().load_texture("live", img, egui::TextureOptions::LINEAR), n)),
                }
            }
        }
        if let Some((tex, _)) = &self.texture {
            ui.add(egui::Image::new(tex).max_width(ui.available_width()).max_height(ui.available_height()).maintain_aspect_ratio(true));
        }
    }

    fn events(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let events = self.shared.lock().unwrap().state.events.clone();
        let list = events["body"]["home"]["events"].as_array().cloned().unwrap_or_default();
        let calls: Vec<&Value> = list.iter().filter(|e| e["type"] == "outdoor").collect();
        if calls.is_empty() {
            ui.label(t(lang, "events.empty"));
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for e in calls {
                let sub = &e["subevents"][0];
                let kind = match sub["type"].as_str() {
                    Some("accepted_call") => t(lang, "events.accepted"),
                    Some("missed_call") => t(lang, "events.missed"),
                    _ => t(lang, "events.incoming"),
                };
                ui.horizontal(|ui| {
                    if let Some(url) = sub["vignette"]["url"].as_str() {
                        match self.vignette(url) {
                            Some(tex) => ui.add(egui::Image::new(&tex).max_height(72.0)),
                            None => ui.add_sized([96.0, 72.0], egui::Spinner::new()),
                        };
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(kind).strong());
                        ui.label(format_time(e["time"].as_i64().unwrap_or(0)));
                        ui.weak(sub["message"].as_str().unwrap_or(""));
                    });
                });
                ui.separator();
            }
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let before = self.settings.clone();
        egui::Grid::new("settings").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label(t(lang, "settings.language"));
            egui::ComboBox::from_id_salt("lang").selected_text(self.settings.lang.name()).show_ui(ui, |ui| {
                for l in Lang::ALL {
                    ui.selectable_value(&mut self.settings.lang, l, l.name());
                }
            });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.settings.autostart, t(lang, "settings.autostart"));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.settings.tray, t(lang, "settings.tray"));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.settings.notify_on_ring, t(lang, "settings.notify"));
            ui.end_row();
            ui.label(t(lang, "settings.account"));
            if ui.button(t(lang, "logout")).clicked() {
                self.send(Command::Logout);
            }
            ui.end_row();
        });
        if self.settings != before && self.shots.is_none() {
            if self.settings.autostart != before.autostart {
                if let Err(e) = crate::autostart::set(self.settings.autostart) {
                    self.shared.lock().unwrap().state.message = Some((tf(lang, "error", &e.to_string()), true));
                }
            }
            let _ = self.settings.save();
        }
    }
}

/// Local time `YYYY-MM-DD HH:MM` without pulling a time-zone crate.
fn format_time(unix: i64) -> String {
    let offset = local_offset_secs();
    let secs = unix + offset;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn local_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(read_offset)
}

fn read_offset() -> i64 {
    // `date +%z` is good enough on Linux; Windows falls back to UTC.
    #[cfg(unix)]
    if let Ok(o) = std::process::Command::new("date").arg("+%z").output() {
        let s = String::from_utf8_lossy(&o.stdout);
        let s = s.trim();
        if s.len() == 5 {
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let h: i64 = s[1..3].parse().unwrap_or(0);
            let m: i64 = s[3..5].parse().unwrap_or(0);
            return sign * (h * 3600 + m * 60);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date() {
        // 2026-10-21 12:00:00 UTC
        let s = format_time(1792584000 - local_offset_secs());
        assert_eq!(s, "2026-10-21 12:00");
    }
}
