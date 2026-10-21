//! System tray icon with Show / Quit: `ksni` (StatusNotifierItem, tokio
//! backend) on Linux, `tray-icon` with its own Win32 message loop on Windows.

use std::sync::mpsc::Sender;

use bticino_easykit::i18n::{t, Lang};

#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    Show,
    Quit,
}

fn icon_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(include_bytes!("../../../assets/icons/bticino-easykit-64.png")).ok()?.to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

#[cfg(target_os = "linux")]
pub fn spawn(tx: Sender<TrayEvent>, lang: Lang) {
    use ksni::TrayMethods;

    struct Tray {
        tx: Sender<TrayEvent>,
        lang: Lang,
    }

    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "bticino-easykit".into()
        }
        fn title(&self) -> String {
            t(self.lang, "app.title").into()
        }
        fn icon_name(&self) -> String {
            "bticino-easykit".into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            // ksni wants ARGB32 in network byte order.
            icon_rgba()
                .map(|(w, h, rgba)| {
                    let data = rgba.as_chunks::<4>().0.iter().flat_map(|[r, g, b, a]| [*a, *r, *g, *b]).collect();
                    vec![ksni::Icon { width: w as i32, height: h as i32, data }]
                })
                .unwrap_or_default()
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.tx.send(TrayEvent::Show);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            vec![
                StandardItem {
                    label: t(self.lang, "tray.show").into(),
                    activate: Box::new(|s: &mut Self| {
                        let _ = s.tx.send(TrayEvent::Show);
                    }),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: t(self.lang, "tray.quit").into(),
                    activate: Box::new(|s: &mut Self| {
                        let _ = s.tx.send(TrayEvent::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(async move {
            match (Tray { tx, lang }).spawn().await {
                // Keep the handle (and the D-Bus service) alive forever.
                Ok(_handle) => std::future::pending::<()>().await,
                Err(e) => log::warn!("tray unavailable: {e}"),
            }
        });
    });
}

#[cfg(windows)]
pub fn spawn(tx: Sender<TrayEvent>, lang: Lang) {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};

    std::thread::spawn(move || {
        // The icon must live on a thread that pumps Win32 messages.
        let show = MenuItem::new(t(lang, "tray.show"), true, None);
        let quit = MenuItem::new(t(lang, "tray.quit"), true, None);
        let menu = Menu::new();
        let _ = menu.append(&show);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&quit);
        let mut builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(t(lang, "app.title"));
        if let Some(icon) = icon_rgba().and_then(|(w, h, rgba)| tray_icon::Icon::from_rgba(rgba, w, h).ok()) {
            builder = builder.with_icon(icon);
        }
        let Ok(_tray) = builder.build() else { return };
        loop {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(e) = MenuEvent::receiver().try_recv() {
                let ev = if e.id == *show.id() { TrayEvent::Show } else if e.id == *quit.id() { TrayEvent::Quit } else { continue };
                if tx.send(ev).is_err() {
                    return;
                }
            }
            while let Ok(e) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = e {
                    let _ = tx.send(TrayEvent::Show);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn spawn(_tx: Sender<TrayEvent>, _lang: Lang) {}
