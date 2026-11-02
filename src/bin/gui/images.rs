//! Visitor photos: download, cache as textures, save to disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;

pub enum Slot {
    Loading,
    Ready { tex: egui::TextureHandle, bytes: Vec<u8> },
    Failed,
}

type Loaded = (String, Option<(egui::ColorImage, Vec<u8>)>);

/// URL → texture cache, filled by background downloads.
pub struct Cache {
    slots: HashMap<String, Slot>,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl Default for Cache {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { slots: HashMap::new(), tx, rx }
    }
}

impl Cache {
    /// Moves finished downloads into textures.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((url, r)) = self.rx.try_recv() {
            let slot = match r {
                Some((img, bytes)) => Slot::Ready { tex: ctx.load_texture(&url, img, egui::TextureOptions::LINEAR), bytes },
                None => Slot::Failed,
            };
            self.slots.insert(url, slot);
        }
    }

    /// The slot for `url`, starting a download the first time.
    pub fn get(&mut self, ctx: &egui::Context, url: &str) -> &Slot {
        if !self.slots.contains_key(url) {
            self.slots.insert(url.to_owned(), Slot::Loading);
            let (tx, url, ctx) = (self.tx.clone(), url.to_owned(), ctx.clone());
            std::thread::spawn(move || {
                let r = download(&url).and_then(|b| decode(&b).map(|img| (img, b)));
                let _ = tx.send((url, r));
                ctx.request_repaint();
            });
        }
        &self.slots[url]
    }

    /// Puts an image in the cache without downloading it (demo mode).
    pub fn insert(&mut self, ctx: &egui::Context, url: &str, rgba: &image::RgbaImage) {
        let mut png = Vec::new();
        let _ = rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png);
        let img = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
        let tex = ctx.load_texture(url, img, egui::TextureOptions::LINEAR);
        self.slots.insert(url.to_owned(), Slot::Ready { tex, bytes: png });
    }

    /// Original bytes of an already downloaded image.
    pub fn bytes(&self, url: &str) -> Option<&[u8]> {
        match self.slots.get(url) {
            Some(Slot::Ready { bytes, .. }) => Some(bytes),
            _ => None,
        }
    }

    pub fn texture(&mut self, ctx: &egui::Context, url: &str) -> Option<egui::TextureHandle> {
        match self.get(ctx, url) {
            Slot::Ready { tex, .. } => Some(tex.clone()),
            _ => None,
        }
    }
}

pub fn download(url: &str) -> Option<Vec<u8>> {
    let r = reqwest::blocking::get(url).ok()?;
    if !r.status().is_success() {
        return None;
    }
    Some(r.bytes().ok()?.to_vec())
}

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// File name for a photo taken at `unix` (local time).
pub fn file_name(unix: i64, bytes: &[u8]) -> String {
    let (y, mo, d, h, mi, s) = crate::app::civil(unix);
    let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpg" };
    format!("visitor_{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}.{ext}")
}

/// Writes the original bytes (no re-encoding) into `dir`; returns the path.
pub fn save(dir: &Path, unix: i64, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(unix, bytes));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Downloads and saves a ring snapshot in the background.
pub fn archive(url: String, unix: i64, dir: PathBuf) {
    std::thread::spawn(move || match download(&url) {
        Some(b) => {
            if let Err(e) = save(&dir, unix, &b) {
                log::warn!("could not save visitor photo: {e}");
            }
        }
        None => log::warn!("could not download visitor photo"),
    });
}

pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(windows)]
    let r = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(not(windows))]
    let r = std::process::Command::new("xdg-open").arg(dir).spawn();
    if let Err(e) = r {
        log::warn!("could not open {}: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_sort_by_time_and_keep_the_format() {
        let jpg = file_name(1_792_584_000, &[0xff, 0xd8, 0xff]);
        let png = file_name(1_792_584_000, &[0x89, b'P', b'N', b'G']);
        assert!(jpg.starts_with("visitor_2026-10-2") && jpg.ends_with(".jpg"), "{jpg}");
        assert!(png.ends_with(".png"));
        assert!(file_name(1_792_584_001, &[]) > file_name(1_792_584_000, &[]));
    }

    #[test]
    fn save_keeps_the_original_bytes() {
        let dir = std::env::temp_dir().join(format!("easykit-save-test-{}", std::process::id()));
        let bytes = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
        let p = save(&dir, 1_792_584_000, &bytes).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
