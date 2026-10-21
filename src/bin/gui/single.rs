//! Single instance: the first copy listens on a loopback port; a second
//! copy asks it to show its window and exits.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::time::Duration;

const ADDR: &str = "127.0.0.1:47613";
const MAGIC: &[u8] = b"bticino-easykit:show";

pub enum Instance {
    First,
    Other,
}

/// Becomes the primary instance (sending `()` on `show` when another copy
/// starts) or tells the running one to show itself.
pub fn claim(show: Sender<()>) -> Instance {
    match TcpListener::bind(ADDR) {
        Ok(l) => {
            std::thread::spawn(move || {
                for mut s in l.incoming().flatten() {
                    let mut buf = [0u8; 32];
                    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
                    if let Ok(n) = s.read(&mut buf) {
                        if &buf[..n] == MAGIC {
                            let _ = show.send(());
                        }
                    }
                }
            });
            Instance::First
        }
        Err(_) => {
            if let Ok(mut s) = TcpStream::connect_timeout(&ADDR.parse().unwrap(), Duration::from_secs(1)) {
                let _ = s.write_all(MAGIC);
            }
            Instance::Other
        }
    }
}
