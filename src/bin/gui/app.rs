//! The egui application: Home, Live, Calls and Settings tabs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bticino_easykit::actions::{Command, Update, Worker};
use bticino_easykit::i18n::{t, tf, Lang};
use bticino_easykit::settings::Settings;
use bticino_easykit::{Home, ModuleKind};
use eframe::egui::{self, RichText};
use serde_json::Value;

use crate::{images, theme};

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
    images: images::Cache,
    viewer: Option<Viewer>,
    toast: Option<(String, bool, Instant)>,
    logo: Option<egui::TextureHandle>,
    /// Width of the live control bar in the previous frame, to centre it.
    bar_width: f32,
    shots: Option<Screenshots>,
    pub on_ring: OnRing,
}

/// Full-size photo of a call, with zoom and save.
struct Viewer {
    url: String,
    fallback: Option<String>,
    time: i64,
    title: String,
    /// `None` = fit to the window.
    zoom: Option<f32>,
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
        theme::apply(&cc.egui_ctx);
        shared.lock().unwrap().ctx = Some(cc.egui_ctx.clone());
        let settings = Settings::load();
        let shots = std::env::var_os("BTICINO_EASYKIT_SCREENSHOTS").map(|d| Screenshots {
            dir: d.into(),
            plan: Lang::ALL.iter().flat_map(|l| Tab::ALL.iter().map(move |t| (*t, *l))).collect(),
            frames: 0,
            pending: false,
            image: Arc::default(),
        });
        let logo = image::load_from_memory(include_bytes!("../../../assets/icons/bticino-easykit-64.png")).ok().map(|i| {
            let i = i.to_rgba8();
            let img = egui::ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], i.as_raw());
            cc.egui_ctx.load_texture("logo", img, egui::TextureOptions::LINEAR)
        });
        let mut images = images::Cache::default();
        let mut viewer = None;
        if std::env::var("BTICINO_EASYKIT_DEMO").is_ok_and(|v| v == "1") {
            images.insert(&cc.egui_ctx, crate::demo::DEMO_PHOTO, &crate::demo::visitor_photo());
        }
        // `BTICINO_EASYKIT_DEMO_VIEWER=1`: open the photo viewer (screenshots).
        if std::env::var("BTICINO_EASYKIT_DEMO_VIEWER").is_ok_and(|v| v == "1") {
            viewer = Some(Viewer {
                url: crate::demo::DEMO_PHOTO.into(),
                fallback: None,
                time: 1_792_600_574,
                title: "Missed — 2026-10-21 17:36".into(),
                zoom: None,
            });
        }
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
            images,
            viewer,
            toast: None,
            logo,
            bar_width: 0.0,
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

    fn notify(&mut self, text: String, error: bool) {
        self.toast = Some((text, error, Instant::now()));
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let mut rings = Vec::new();
        let mut message = None;
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
            if let Some(m) = s.state.message.take() {
                message = Some(m);
            }
        }
        if let Some((m, err)) = message {
            self.notify(m, err);
        }
        for r in rings {
            if self.settings.notify_on_ring {
                (self.on_ring)(&r, lang);
            }
            if self.settings.save_visitor_photos {
                archive_ring(&r);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.tab = Tab::Home;
        }
        self.images.poll(ctx);
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

/// Saves the snapshot of a ring (the cloud link expires) if one is attached.
pub fn archive_ring(params: &Value) {
    if let Some(url) = params["snapshot_url"].as_str().or(params["vignette_url"].as_str()) {
        let time = params["timestamp"].as_i64().unwrap_or_else(now_unix);
        images::archive(url.to_owned(), time, Settings::photos_dir());
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
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

        egui::Panel::top("tabs")
            .frame(egui::Frame::new().fill(theme::SURFACE).inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(28.0, 28.0)));
                    }
                    ui.label(RichText::new(t(lang, "app.short")).size(19.0).strong());
                    ui.add_space(18.0);
                    if logged_in == Some(true) {
                        for tab in Tab::ALL {
                            let selected = self.tab == tab;
                            let text = RichText::new(format!("{}  {}", tab.icon(), t(lang, tab.key())))
                                .size(15.0)
                                .color(if selected { egui::Color32::WHITE } else { theme::MUTED });
                            let b = egui::Button::new(text)
                                .fill(if selected { theme::ACCENT } else { egui::Color32::TRANSPARENT })
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(egui::CornerRadius::same(255));
                            if ui.add(b).clicked() {
                                self.tab = tab;
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.shared.lock().unwrap().state.busy {
                            ui.spinner();
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::same(18)))
            .show(ui, |ui| match logged_in {
                None => {
                    ui.centered_and_justified(|ui| ui.spinner());
                }
                Some(false) => self.login(ui),
                Some(true) => {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        self.ring_banner(ui);
                        match self.tab {
                            Tab::Home => self.home(ui),
                            Tab::Live => self.live(ui),
                            Tab::Events => self.events(ui),
                            Tab::Settings => self.settings_tab(ui),
                        }
                    });
                }
            });

        self.toast(&ctx);
        self.viewer(&ctx);

        if let Some((id, name)) = self.confirm.clone() {
            let r = egui::Modal::new(egui::Id::unique("confirm-open")).show(&ctx, |ui| {
                ui.set_width(340.0);
                ui.label(RichText::new("🔓").size(32.0));
                ui.label(RichText::new(tf(lang, "confirm.open", &name)).size(18.0).strong());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::big_button(ui, t(lang, "confirm.yes"), theme::OK).clicked() {
                        self.send(Command::Unlock(id.clone()));
                        self.confirm = None;
                    }
                    if ui.add(egui::Button::new(t(lang, "confirm.no")).min_size(egui::vec2(0.0, 42.0))).clicked() {
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

impl Tab {
    fn icon(self) -> &'static str {
        match self {
            Tab::Home => "🏠",
            Tab::Live => "📹",
            Tab::Events => "📞",
            Tab::Settings => "⚙",
        }
    }
}

impl App {
    fn toast(&mut self, ctx: &egui::Context) {
        let Some((text, error, at)) = self.toast.clone() else { return };
        if at.elapsed() > Duration::from_secs(6) {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -20.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(if error { theme::DANGER } else { theme::SURFACE_2 })
                    .stroke(egui::Stroke::new(1.0, if error { theme::DANGER } else { theme::BORDER }))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(egui::Color32::WHITE));
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn login(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.set_max_width(420.0);
            theme::card(ui, None, |ui| {
                ui.vertical_centered(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(56.0, 56.0)));
                    }
                    ui.label(RichText::new(t(lang, "login.title")).size(18.0).strong());
                });
                ui.add_space(8.0);
                ui.label(theme::muted(t(lang, "login.email")));
                ui.add(egui::TextEdit::singleline(&mut self.email).desired_width(f32::INFINITY));
                ui.label(theme::muted(t(lang, "login.password")));
                let pw = ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(f32::INFINITY));
                let enter = pw.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(6.0);
                let go = ui.add_sized(
                    [ui.available_width(), 42.0],
                    egui::Button::new(RichText::new(t(lang, "login.button")).size(16.0).strong().color(egui::Color32::WHITE))
                        .fill(theme::ACCENT),
                );
                if (go.clicked() || enter) && !self.email.is_empty() {
                    let password = std::mem::take(&mut self.password);
                    self.send(Command::Login { email: self.email.trim().to_owned(), password });
                }
                ui.add_space(4.0);
                ui.label(theme::muted(t(lang, "login.note")).size(12.0));
            });
        });
    }

    fn ring_banner(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let (ring, can_answer, home) = {
            let s = self.shared.lock().unwrap();
            (s.state.ring.clone(), s.state.can_answer, s.state.home.clone())
        };
        let Some((params, at)) = ring else { return };
        if at.elapsed() > Duration::from_secs(60) {
            return;
        }
        egui::Frame::new()
            .fill(theme::OK.gamma_multiply(0.15))
            .stroke(egui::Stroke::new(2.0, theme::OK))
            .corner_radius(egui::CornerRadius::same(14))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let thumb = params["vignette_url"].as_str().or(params["snapshot_url"].as_str());
                    if let Some(url) = thumb {
                        if let Some(tex) = self.images.texture(ui.ctx(), url) {
                            let r = ui.add(egui::Image::new(&tex).max_height(110.0).corner_radius(10.0).sense(egui::Sense::click()));
                            if r.on_hover_text(t(lang, "events.zoom")).clicked() {
                                self.viewer = Some(Viewer {
                                    url: params["snapshot_url"].as_str().unwrap_or(url).to_owned(),
                                    fallback: Some(url.to_owned()),
                                    time: params["timestamp"].as_i64().unwrap_or_else(now_unix),
                                    title: t(lang, "ring.title").to_owned(),
                                    zoom: None,
                                });
                            }
                        }
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("🔔  {}", t(lang, "ring.title"))).size(22.0).strong());
                        ui.horizontal_wrapped(|ui| {
                            if can_answer && theme::big_button(ui, format!("📞 {}", t(lang, "ring.answer")), theme::OK).clicked() {
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
                                if theme::big_button(ui, format!("🔓 {}", m.name), theme::ACCENT).clicked() {
                                    self.confirm = Some((m.id.clone(), m.name.clone()));
                                }
                            }
                        });
                    });
                });
            });
        ui.add_space(12.0);
        ui.ctx().request_repaint_after(Duration::from_secs(1));
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("⟳  {}", t(lang, "refresh"))).clicked() {
                    self.send(Command::Refresh);
                }
            });
        });
        ui.add_space(4.0);

        let mods = status["body"]["home"]["modules"].as_array().cloned().unwrap_or_default();
        if let Some(b) = home.bridge() {
            let st = mods.iter().find(|m| m["id"] == b.id.as_str()).cloned().unwrap_or_default();
            let online = st["reachable"].as_bool().unwrap_or(false);
            theme::card(ui, Some(t(lang, "home.intercom")), |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&b.name).size(17.0).strong());
                    if online {
                        theme::chip(ui, format!("●  {}", t(lang, "home.online")), theme::OK);
                    } else {
                        theme::chip(ui, format!("●  {}", t(lang, "home.offline")), theme::DANGER);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(theme::muted(t(lang, "home.firmware")));
                    ui.label(st["firmware_name"].as_str().unwrap_or("-"));
                    ui.add_space(18.0);
                    ui.label(theme::muted(t(lang, "home.wifi")));
                    match st["wifi_strength"].as_u64() {
                        Some(w) => ui.label(format!("{}  {}", wifi_bars(w), tf(lang, "home.signal", &w.to_string()))),
                        None => ui.label("-"),
                    };
                });
            });
        }
        ui.add_space(8.0);
        theme::card(ui, Some(t(lang, "home.locks")), |ui| {
            for m in home.by_kind(ModuleKind::DoorLock) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("🚪  {}", m.name)).size(17.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::big_button(ui, format!("🔓  {}", t(lang, "home.open")), theme::ACCENT).clicked() {
                            self.confirm = Some((m.id.clone(), m.name.clone()));
                        }
                    });
                });
            }
        });
        if !lights.is_empty() {
            ui.add_space(8.0);
            theme::card(ui, Some(t(lang, "home.light")), |ui| {
                for (id, name) in lights {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("💡  {name}")).size(17.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(t(lang, "home.off")).clicked() {
                                self.send(Command::Light(id.clone(), false));
                            }
                            if ui.button(t(lang, "home.on")).clicked() {
                                self.send(Command::Light(id.clone(), true));
                            }
                        });
                    });
                }
            });
        }
    }

    fn live(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let (running, state, frame) = {
            let s = self.shared.lock().unwrap();
            (s.state.live_running, s.state.live_state.clone(), s.state.frame.clone())
        };
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

        // Video area: 4:3, as large as the window allows.
        let avail = ui.available_size();
        let w = avail.x.min((avail.y - 80.0).max(200.0) * 4.0 / 3.0);
        let size = egui::vec2(w, w * 3.0 / 4.0);
        ui.vertical_centered(|ui| {
            egui::Frame::new()
                .fill(egui::Color32::BLACK)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(egui::CornerRadius::same(14))
                .show(ui, |ui| {
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    match (&self.texture, running) {
                        (Some((tex, _)), true) => {
                            ui.add(egui::Image::new(tex).fit_to_exact_size(size).corner_radius(14.0));
                        }
                        _ => {
                            ui.centered_and_justified(|ui| {
                                if running {
                                    ui.label(RichText::new(format!("⏳  {}", t(lang, "live.connecting"))).size(18.0).color(theme::MUTED));
                                } else {
                                    let msg = if state.is_empty() { t(lang, "live.idle").to_owned() } else { state.clone() };
                                    ui.label(RichText::new(format!("📹\n\n{msg}")).size(17.0).color(theme::MUTED));
                                }
                            });
                        }
                    }
                });
            ui.add_space(10.0);
            let bar = ui.horizontal(|ui| {
                // Centre the control bar using last frame's width.
                ui.add_space(((ui.available_width() - self.bar_width) / 2.0).max(0.0));
                let start = ui.cursor().min.x;
                if running {
                    if theme::big_button(ui, format!("■  {}", t(lang, "live.stop")), theme::DANGER).clicked() {
                        self.send(Command::StopLive);
                    }
                    let mute = if self.muted { theme::WARN } else { theme::SURFACE_2 };
                    if theme::big_button(ui, format!("🔇  {}", t(lang, "live.mute")), mute).clicked() {
                        self.muted = !self.muted;
                        self.send_quiet(Command::Mute(self.muted));
                    }
                    if theme::big_button(ui, format!("📸  {}", t(lang, "live.snapshot")), theme::ACCENT).clicked() {
                        self.save_live_frame();
                    }
                    // One click opens the microphone, another closes it.
                    let (label, color) = if self.talking {
                        (t(lang, "live.mic_on"), theme::DANGER)
                    } else {
                        (t(lang, "live.talk"), theme::SURFACE_2)
                    };
                    if theme::big_button(ui, format!("🎤  {label}"), color).clicked() {
                        self.talking = !self.talking;
                        self.send_quiet(Command::Talk(self.talking));
                    }
                } else {
                    // Every call starts with the microphone closed and audio on.
                    self.talking = false;
                    self.muted = false;
                    if theme::big_button(ui, format!("▶  {}", t(lang, "live.start")), theme::OK).clicked() {
                        let mut s = self.shared.lock().unwrap();
                        s.state.live_running = true;
                        s.state.frame = None;
                        drop(s);
                        self.texture = None;
                        self.send(Command::StartLive);
                    }
                }
                ui.cursor().min.x - start
            });
            if (bar.inner - self.bar_width).abs() > 1.0 {
                self.bar_width = bar.inner;
                ui.ctx().request_repaint();
            }
        });
    }

    fn events(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let events = self.shared.lock().unwrap().state.events.clone();
        let list = events["body"]["home"]["events"].as_array().cloned().unwrap_or_default();
        let calls: Vec<&Value> = list.iter().filter(|e| e["type"] == "outdoor").collect();
        ui.horizontal(|ui| {
            ui.heading(t(lang, "tab.events"));
            ui.label(theme::muted(tf(lang, "events.count", &calls.len().to_string())));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("⟳  {}", t(lang, "refresh"))).clicked() {
                    self.send(Command::Refresh);
                }
                if ui.button(format!("📂  {}", t(lang, "events.open_folder"))).clicked() {
                    images::open_folder(&Settings::photos_dir());
                }
            });
        });
        if calls.is_empty() {
            theme::card(ui, None, |ui| ui.label(theme::muted(t(lang, "events.empty"))));
            return;
        }
        let ctx = ui.ctx().clone();
        for e in calls {
            let sub = &e["subevents"][0];
            let (kind, color) = match sub["type"].as_str() {
                Some("accepted_call") => (t(lang, "events.accepted"), theme::OK),
                Some("missed_call") => (t(lang, "events.missed"), theme::DANGER),
                _ => (t(lang, "events.incoming"), theme::WARN),
            };
            let time = e["time"].as_i64().unwrap_or(0);
            let vignette = sub["vignette"]["url"].as_str().map(str::to_owned);
            let snapshot = sub["snapshot"]["url"].as_str().map(str::to_owned);
            theme::card(ui, None, |ui| {
                ui.horizontal(|ui| {
                    let thumb = egui::vec2(128.0, 96.0);
                    let mut open = false;
                    match vignette.as_deref().or(snapshot.as_deref()) {
                        Some(url) => match self.images.texture(&ctx, url) {
                            Some(tex) => {
                                let r = ui.add(egui::Image::new(&tex).fit_to_exact_size(thumb).corner_radius(10.0).sense(egui::Sense::click()));
                                open |= r.on_hover_text(t(lang, "events.zoom")).clicked();
                            }
                            None => {
                                ui.add_sized(thumb, egui::Spinner::new());
                            }
                        },
                        None => {
                            ui.add_sized(thumb, egui::Label::new(RichText::new("👤").size(40.0).color(theme::MUTED)));
                        }
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            theme::chip(ui, kind, color);
                            ui.label(RichText::new(format_time(time)).size(16.0).strong());
                        });
                        ui.label(theme::muted(sub["message"].as_str().unwrap_or("")));
                        if snapshot.is_some() || vignette.is_some() {
                            ui.horizontal(|ui| {
                                open |= ui.button(format!("🔍  {}", t(lang, "events.zoom"))).clicked();
                                if ui.button(format!("💾  {}", t(lang, "events.save"))).clicked() {
                                    let url = snapshot.clone().or(vignette.clone()).unwrap_or_default();
                                    self.save_photo(&url, time);
                                }
                            });
                        }
                    });
                    if open {
                        self.viewer = Some(Viewer {
                            url: snapshot.clone().or(vignette.clone()).unwrap_or_default(),
                            fallback: vignette.clone(),
                            time,
                            title: format!("{kind} — {}", format_time(time)),
                            zoom: None,
                        });
                    }
                });
            });
            ui.add_space(6.0);
        }
    }

    /// Saves the current live frame at full resolution (640×480 on the
    /// EASYKIT, while cloud snapshots are only 148×83).
    fn save_live_frame(&mut self) {
        let lang = self.lang();
        let frame = self.shared.lock().unwrap().state.frame.clone();
        let Some((w, h, rgba, _)) = frame else { return };
        let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) else { return };
        let mut png = Vec::new();
        let r = img
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())
            .and_then(|_| images::save(&Settings::photos_dir(), now_unix(), &png).map_err(|e| e.to_string()));
        match r {
            Ok(p) => self.notify(tf(lang, "events.saved", &p.display().to_string()), false),
            Err(e) => self.notify(tf(lang, "error", &e), true),
        }
    }

    /// Saves the full-size photo (downloading it first if needed).
    fn save_photo(&mut self, url: &str, time: i64) {
        let lang = self.lang();
        if let Some(bytes) = self.images.bytes(url) {
            let r = images::save(&Settings::photos_dir(), time, bytes);
            match r {
                Ok(p) => self.notify(tf(lang, "events.saved", &p.display().to_string()), false),
                Err(e) => self.notify(tf(lang, "error", &e.to_string()), true),
            }
            return;
        }
        // Not downloaded yet: fetch and save in the background.
        let (url, dir) = (url.to_owned(), Settings::photos_dir());
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let msg = match images::download(&url) {
                Some(b) => match images::save(&dir, time, &b) {
                    Ok(p) => (tf(lang, "events.saved", &p.display().to_string()), false),
                    Err(e) => (tf(lang, "error", &e.to_string()), true),
                },
                None => (t(lang, "events.expired").to_owned(), true),
            };
            let mut s = shared.lock().unwrap();
            s.state.message = Some(msg);
            if let Some(c) = &s.ctx {
                c.request_repaint();
            }
        });
    }

    fn viewer(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let Some(v) = &self.viewer else { return };
        let (url, fallback, time, title) = (v.url.clone(), v.fallback.clone(), v.time, v.title.clone());
        let mut zoom = v.zoom;
        let mut open = true;
        let mut save = false;
        let screen = ctx.content_rect().size();
        egui::Window::new(title)
            .id(egui::Id::unique("photo-viewer"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(screen * 0.85)
            .max_size(screen * 0.95)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                // Prefer the full-size snapshot; fall back to the thumbnail.
                let tex = match self.images.get(ctx, &url) {
                    images::Slot::Ready { tex, .. } => Some(tex.clone()),
                    images::Slot::Failed => fallback.as_deref().and_then(|f| self.images.texture(ctx, f)),
                    images::Slot::Loading => None,
                };
                // Zoom buttons act on the current scale, applied below.
                let mut step = 1.0f32;
                let mut fit_now = false;
                let mut actual = false;
                ui.horizontal_wrapped(|ui| {
                    if ui.button("−").clicked() {
                        step = 1.0 / 1.25;
                    }
                    if ui.button("+").clicked() {
                        step = 1.25;
                    }
                    fit_now = ui.button(t(lang, "viewer.fit")).clicked();
                    actual = ui.button("100%").clicked();
                    ui.separator();
                    save = ui.button(format!("💾  {}", t(lang, "events.save"))).clicked();
                    if ui.button(format!("📂  {}", t(lang, "events.open_folder"))).clicked() {
                        images::open_folder(&Settings::photos_dir());
                    }
                });
                ui.label(theme::muted(t(lang, "viewer.hint")).size(12.0));
                let Some(tex) = tex else {
                    ui.centered_and_justified(|ui| ui.spinner());
                    return;
                };
                let img = tex.size_vec2();
                let area = ui.available_size();
                let fit = (area.x / img.x).min(area.y / img.y).max(0.05);
                let mut scale = zoom.unwrap_or(fit);
                if fit_now {
                    zoom = None;
                    scale = fit;
                } else if actual {
                    scale = 1.0;
                    zoom = Some(scale);
                } else if step != 1.0 {
                    scale = (scale * step).clamp(0.05, 8.0);
                    zoom = Some(scale);
                }
                // Ctrl + wheel / pinch.
                let z = ui.input(|i| i.zoom_delta());
                if z != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
                    scale = (scale * z).clamp(0.05, 8.0);
                    zoom = Some(scale);
                }
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    let size = img * scale;
                    let pad = ((area - size) / 2.0).max(egui::Vec2::ZERO);
                    ui.add_space(pad.y);
                    ui.horizontal(|ui| {
                        ui.add_space(pad.x);
                        let r = ui.add(egui::Image::new(&tex).fit_to_exact_size(size).sense(egui::Sense::drag()));
                        if r.dragged() {
                            ui.scroll_with_delta(r.drag_delta());
                        }
                    });
                });
            });
        if save {
            self.save_photo(&url, time);
        }
        match (&mut self.viewer, open) {
            (Some(v), true) => v.zoom = zoom,
            _ => self.viewer = None,
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let before = self.settings.clone();
        ui.heading(t(lang, "tab.settings"));
        theme::card(ui, Some(t(lang, "settings.app")), |ui| {
            ui.horizontal(|ui| {
                ui.label(t(lang, "settings.language"));
                egui::ComboBox::from_id_salt("lang").selected_text(self.settings.lang.name()).show_ui(ui, |ui| {
                    for l in Lang::ALL {
                        ui.selectable_value(&mut self.settings.lang, l, l.name());
                    }
                });
            });
            ui.checkbox(&mut self.settings.autostart, t(lang, "settings.autostart"));
            ui.checkbox(&mut self.settings.tray, t(lang, "settings.tray"));
        });
        ui.add_space(8.0);
        theme::card(ui, Some(t(lang, "settings.privacy")), |ui| {
            ui.checkbox(&mut self.settings.notify_on_ring, t(lang, "settings.notify"));
            ui.checkbox(&mut self.settings.save_visitor_photos, t(lang, "settings.photos"));
            ui.horizontal(|ui| {
                ui.label(theme::muted(t(lang, "settings.photos_dir")));
                ui.label(images::display_path(&Settings::photos_dir()));
                if ui.button(format!("📂  {}", t(lang, "events.open_folder"))).clicked() {
                    images::open_folder(&Settings::photos_dir());
                }
            });
        });
        ui.add_space(8.0);
        theme::card(ui, Some(t(lang, "settings.account")), |ui| {
            if ui.button(format!("⎋  {}", t(lang, "logout"))).clicked() {
                self.send(Command::Logout);
            }
        });
        if self.settings != before && self.shots.is_none() {
            if self.settings.autostart != before.autostart {
                if let Err(e) = crate::autostart::set(self.settings.autostart) {
                    self.notify(tf(lang, "error", &e.to_string()), true);
                }
            }
            let _ = self.settings.save();
        }
    }
}

/// ▂▄▆█ from the Wi-Fi strength (0-100).
fn wifi_bars(strength: u64) -> &'static str {
    match strength {
        0..=25 => "▂",
        26..=50 => "▂▄",
        51..=75 => "▂▄▆",
        _ => "▂▄▆█",
    }
}

/// Local `(year, month, day, hour, minute, second)` without a time-zone crate.
pub fn civil(unix: i64) -> (i64, i64, i64, i64, i64, i64) {
    let secs = unix + local_offset_secs();
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
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Local time `YYYY-MM-DD HH:MM`.
fn format_time(unix: i64) -> String {
    let (y, m, d, h, mi, _) = civil(unix);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
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
