//! Headless check of the GUI pipeline: Worker -> live view -> decoded frames.
//! Saves the last frame to `$1` (PNG) when built with the `gui` feature.
use bticino_easykit::actions::{Command, Update, Worker};
use std::time::{Duration, Instant};

fn main() {
    let out = std::env::args().nth(1);
    let w = Worker::spawn(|| {});
    let mut started = false;
    let mut frames = 0u32;
    let mut last = None;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(45) {
        match w.rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Update::Home(h, _)) if !started => {
                println!("home {} loaded, starting live view", h.name);
                started = true;
                w.send(Command::StartLive);
            }
            Ok(Update::LiveState(s)) => println!("live: {s}"),
            Ok(Update::Frame(f)) => {
                frames += 1;
                if frames == 1 {
                    println!("first frame {}x{} after {:?}", f.width, f.height, t0.elapsed());
                }
                if frames == 150 {
                    w.send(Command::StopLive);
                }
                last = Some(f);
            }
            Ok(Update::LiveEnded(r)) => {
                println!("ended: {:?}", r.map(|r| r.ended_by));
                break;
            }
            Ok(Update::Error(e)) => println!("error: {e}"),
            _ => {}
        }
    }
    println!("decoded frames: {frames}");
    if let (Some(p), Some(f)) = (out, last) {
        image::save_buffer(&p, &f.rgba, f.width as u32, f.height as u32, image::ColorType::Rgba8).unwrap();
        println!("saved {p}");
    }
}
