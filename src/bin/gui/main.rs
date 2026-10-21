//! Desktop app for the BTicino EASYKIT.
//!
//! Environment:
//! - `BTICINO_EASYKIT_DEMO=1`: fake data, no network (for screenshots);
//! - `BTICINO_EASYKIT_SCREENSHOTS=<dir>`: save every tab in both languages, then quit;
//! - `BTICINO_EASYKIT_CONFIG_DIR`, `BTICINO_EASYKIT_DEBUG=1`: see the library.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod autostart;
mod demo;
mod fonts;
mod single;
mod tray;

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app::{App, Shared, SharedRef, State};
use bticino_easykit::actions::Worker;
use bticino_easykit::i18n::{t, Lang};
use bticino_easykit::settings::Settings;
use eframe::egui;
use serde_json::Value;
use tray::TrayEvent;

fn notify_ring(params: &Value, lang: Lang) {
    let body = params["home_name"].as_str().unwrap_or("BTicino");
    let r = notify_rust::Notification::new()
        .summary(t(lang, "ring.title"))
        .body(body)
        .icon("bticino-easykit")
        .appname("Door entry for BTicino EASYKIT")
        .show();
    if let Err(e) = r {
        log::warn!("notification failed: {e}");
    }
}

fn native_options() -> eframe::NativeOptions {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icons/bticino-easykit-256.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Door entry for BTicino EASYKIT")
        .with_app_id("bticino-easykit")
        .with_inner_size([820.0, 620.0])
        .with_min_inner_size([480.0, 380.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    eframe::NativeOptions { viewport, ..Default::default() }
}

fn main() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).try_init();
    let demo = std::env::var("BTICINO_EASYKIT_DEMO").is_ok_and(|v| v == "1");
    let screenshots = std::env::var_os("BTICINO_EASYKIT_SCREENSHOTS").is_some();
    let start_hidden = std::env::args().any(|a| a == "--minimized");

    let (ctl_tx, ctl_rx) = channel::<TrayEvent>();
    if !screenshots {
        let (show_tx, show_rx) = channel::<()>();
        if let single::Instance::Other = single::claim(show_tx) {
            return;
        }
        let tx = ctl_tx.clone();
        std::thread::spawn(move || {
            while show_rx.recv().is_ok() {
                let _ = tx.send(TrayEvent::Show);
            }
        });
    }

    let shared: SharedRef = Arc::new(Mutex::new(Shared { worker: None, ctx: None, state: State::default(), quit: false }));
    if demo {
        shared.lock().unwrap().state = demo::state();
    } else {
        let s = shared.clone();
        let worker = Worker::spawn(move || {
            // Repaint the window if open; otherwise the hidden loop polls.
            if let Some(ctx) = s.try_lock().ok().and_then(|g| g.ctx.clone()) {
                ctx.request_repaint();
            }
        });
        shared.lock().unwrap().worker = Some(worker);
    }

    let settings = Settings::load();
    if !screenshots && settings.tray {
        tray::spawn(ctl_tx.clone(), settings.lang);
    }
    let on_ring: app::OnRing = Arc::new(notify_ring);

    let mut hidden = start_hidden && settings.tray;
    loop {
        if !hidden {
            let (sh, ring) = (shared.clone(), on_ring.clone());
            let r = eframe::run_native(
                "bticino-easykit",
                native_options(),
                Box::new(move |cc| Ok(Box::new(App::new(cc, sh, ring)))),
            );
            shared.lock().unwrap().ctx = None;
            if let Err(e) = r {
                eprintln!("window error: {e}");
                return;
            }
            let s = Settings::load();
            if screenshots || shared.lock().unwrap().quit || !s.tray {
                return;
            }
        }
        // Window closed to the tray (closing and re-creating it also works on
        // Wayland, where a window cannot be hidden). Keep watching for rings.
        hidden = true;
        while hidden {
            match ctl_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(TrayEvent::Show) => hidden = false,
                Ok(TrayEvent::Quit) => return,
                Err(_) => {}
            }
            let lang = Settings::load().lang;
            let mut g = shared.lock().unwrap();
            let updates: Vec<_> = g.worker.as_ref().map(|w| w.rx.try_iter().collect()).unwrap_or_default();
            for u in updates {
                if app::apply(&mut g.state, u, lang) {
                    let p = g.state.ring.as_ref().map(|r| r.0.clone()).unwrap_or_default();
                    if Settings::load().notify_on_ring {
                        notify_ring(&p, lang);
                    }
                    hidden = false;
                }
            }
        }
    }
}
