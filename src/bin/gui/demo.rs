//! Fake data for `BTICINO_EASYKIT_DEMO=1` (screenshots without real homes).

use bticino_easykit::model::parse_homes;
use serde_json::json;

use crate::app::State;

/// Fake photo URL served from the cache in demo mode.
pub const DEMO_PHOTO: &str = "demo://visitor";

/// A synthetic "visitor": silhouette on a gradient (no real person).
pub fn visitor_photo() -> image::RgbaImage {
    let (w, h) = (640u32, 480u32);
    image::RgbaImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let head = ((fx - 320.0).powi(2) + (fy - 170.0).powi(2)).sqrt() < 70.0;
        let body = fy > 250.0 && (fx - 320.0).abs() < 150.0 - (fy - 250.0) * -0.25;
        if head || body {
            image::Rgba([40, 46, 56, 255])
        } else {
            let t = fy / h as f32;
            image::Rgba([(120.0 + 60.0 * t) as u8, (150.0 + 40.0 * t) as u8, (180.0 - 20.0 * t) as u8, 255])
        }
    })
}

pub fn state() -> State {
    let topo = json!({"body": {"homes": [{
        "id": "demo", "name": "Demo home", "timezone": "Europe/Rome",
        "modules": [
            {"id": "00:03:50:00:00:01", "type": "BDIY", "name": "Indoor monitor"},
            {"id": "eu-1", "type": "BNEU", "name": "Street panel", "bridge": "00:03:50:00:00:01"},
            {"id": "dl-1", "type": "BNDL", "name": "Pedestrian gate", "bridge": "00:03:50:00:00:01"},
            {"id": "dl-2", "type": "BNDL", "name": "Garage gate", "bridge": "00:03:50:00:00:01"}
        ]}]}});
    let status = json!({"body": {"home": {"modules": [
        {"id": "00:03:50:00:00:01", "reachable": true, "firmware_name": "1.00.11", "wifi_strength": 48}
    ]}}});
    let call = |time: i64, kind: &str, msg: &str| {
        json!({"type": "outdoor", "time": time, "subevents": [{"type": kind, "message": msg,
            "vignette": {"url": DEMO_PHOTO}, "snapshot": {"url": DEMO_PHOTO}}]})
    };
    let events = json!({"body": {"home": {"events": [
        call(1792600574, "missed_call", "Missed call"),
        call(1792506393, "accepted_call", "Answered by Indoor monitor"),
        call(1791307246, "accepted_call", "Answered by Indoor monitor"),
    ]}}});
    // A synthetic 640x480 test pattern stands in for the camera.
    let (w, h) = (640usize, 480usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let band = (x * 7 / w) as u8;
            let c = [[200, 200, 200], [200, 200, 40], [40, 200, 200], [40, 200, 40], [200, 40, 200], [200, 40, 40], [40, 40, 200]][band as usize];
            let shade = 0.6 + 0.4 * (y as f32 / h as f32);
            rgba.extend_from_slice(&[(c[0] as f32 * shade) as u8, (c[1] as f32 * shade) as u8, (c[2] as f32 * shade) as u8, 255]);
        }
    }
    State {
        logged_in: Some(true),
        home: parse_homes(&topo).into_iter().next(),
        status,
        lights: vec![("sl-1".into(), "Staircase light".into())],
        events,
        live_state: "connected".into(),
        live_running: true,
        frame: Some((w, h, rgba, 0)),
        ..Default::default()
    }
}
