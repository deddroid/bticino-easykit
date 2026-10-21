//! `test-session`: every check that needs someone at the intercom, in one go.
//!
//! 1. registers for call pushes (FCM + push WebSocket) and logs everything;
//! 2. waits for a ring and answers it (or, if no SDP offer arrives, opens a
//!    live view during the ring), recording video/audio;
//! 3. opens each door lock only after the user typed "pronto"/"ready" and a
//!    visible countdown, then asks whether it really opened;
//! 4. removes the push registration and writes `summary.json`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bticino_easykit::call::{self, CallOptions, Log, Mode};
use bticino_easykit::push::{self, Push};
use bticino_easykit::{protocol as p, Client, ModuleKind, Result};
use serde_json::{json, Value};

fn italian() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .is_some_and(|v| v.starts_with("it"))
}

fn tr<'a>(en: &'a str, it: &'a str) -> &'a str {
    if italian() {
        it
    } else {
        en
    }
}

fn say(msg: &str) {
    println!("\n>>> {msg}");
}

/// With `--answers-dir`, answers come from files instead of the terminal:
/// question N waits for `<dir>/answer-N` (e.g. written by an assistant
/// relaying the user's replies from a phone). Otherwise stdin is used.
static ANSWERS: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
static ASKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

async fn ask(prompt: String) -> String {
    if let Some(Some(dir)) = ANSWERS.get() {
        let n = ASKED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let f = dir.join(format!("answer-{n}"));
        println!("{prompt} [waiting for {}]", f.display());
        loop {
            if let Ok(s) = std::fs::read_to_string(&f) {
                let a = s.trim().to_lowercase();
                println!("    <- {a}");
                return a;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    tokio::task::spawn_blocking(move || {
        print!("{prompt} ");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        s.trim().to_lowercase()
    })
    .await
    .unwrap_or_default()
}

fn make_log(dir: &Path) -> Log {
    let file = Arc::new(Mutex::new(
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("events.jsonl")).ok(),
    ));
    let start = Instant::now();
    Arc::new(move |src: &str, v: &Value| {
        let t = start.elapsed().as_secs_f32();
        let line = json!({ "t": t, "src": src, "msg": v });
        if let Some(f) = file.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{line}");
        }
        let mut s = v.to_string();
        if s.len() > 160 {
            s.truncate(160);
            s.push('…');
        }
        println!("[{t:7.1}s {src}] {s}");
    })
}

/// Runs a blocking API call off the async runtime.
async fn api<T: Send + 'static>(
    c: &Arc<Mutex<Client>>,
    f: impl FnOnce(&mut Client) -> Result<T> + Send + 'static,
) -> Result<T> {
    let c = c.clone();
    tokio::task::spawn_blocking(move || f(&mut c.lock().unwrap()))
        .await
        .map_err(|e| bticino_easykit::Error::Other(e.to_string()))?
}

pub fn run(out: Option<PathBuf>, wait_minutes: u64, unlock_delay: u64, answers_dir: Option<PathBuf>) -> Result<()> {
    if let Some(d) = &answers_dir {
        std::fs::create_dir_all(d)?;
    }
    let _ = ANSWERS.set(answers_dir);
    let dir = out.unwrap_or_else(|| format!("captures/session-{}", super::unix_now()).into());
    std::fs::create_dir_all(&dir)?;
    // The blocking HTTP client must be created and dropped outside the
    // async runtime, so it outlives it here.
    let client = Arc::new(Mutex::new(Client::new()?));
    let rt = tokio::runtime::Runtime::new()?;
    let r = rt.block_on(session(client.clone(), dir, wait_minutes, unlock_delay));
    drop(rt);
    drop(client);
    r
}

async fn session(client: Arc<Mutex<Client>>, dir: PathBuf, wait_minutes: u64, unlock_delay: u64) -> Result<()> {
    let log = make_log(&dir);
    let mut summary = json!({ "dir": dir.display().to_string() });

    let home = api(&client, |c| {
        c.homes()?
            .into_iter()
            .find(|h| h.bridge().is_some())
            .ok_or_else(|| bticino_easykit::Error::Other("no intercom found".into()))
    })
    .await?;
    let bridge = home.bridge().map(|b| b.id.clone()).unwrap_or_default();
    let unit = home.by_kind(ModuleKind::ExternalUnit).next().map(|m| m.id.clone());

    // 1. Call pushes from both channels.
    say(tr("Registering for call notifications…", "Registrazione alle notifiche di chiamata…"));
    let (fcm, st, _) = tokio::task::spawn_blocking(push::load_or_register)
        .await
        .map_err(|e| bticino_easykit::Error::Other(e.to_string()))??;
    let token = st.fcm_token.clone();
    let r = api(&client, move |c| push::add_push_context(c, &token)).await?;
    log("addpushcontext", &r);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, Push)>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = push::listen(fcm, move |p| {
                let _ = tx.send(("fcm".into(), p));
            });
            eprintln!("FCM listener stopped: {r:?}");
        });
    }
    {
        let (wtx, mut wrx) = tokio::sync::mpsc::unbounded_channel();
        let c = client.clone();
        tokio::spawn(async move {
            // Blocking HTTP is only safe off the async workers.
            let r = push::listen_ws(move || tokio::task::block_in_place(|| c.lock().unwrap().token()), wtx).await;
            eprintln!("push WebSocket stopped: {r:?}");
        });
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(p) = wrx.recv().await {
                let _ = tx.send(("ws".into(), p));
            }
        });
    }

    // 2. Ring and answer.
    say(&format!(
        "{} {wait_minutes} min.\n    {}",
        tr("Ready. Go to the entrance panel and RING THE DOORBELL. Waiting up to", "Pronto. Vai al posto esterno e SUONA IL CAMPANELLO. Aspetto fino a"),
        tr(
            "While the call is open (about 30 s), speak in front of the panel: the audio is recorded.",
            "Durante la chiamata (circa 30 s) parla davanti al posto esterno: registro l'audio."
        ),
    ));
    let deadline = Instant::now() + Duration::from_secs(wait_minutes * 60);
    let mut ring: Option<Instant> = None;
    let mut offer: Option<(String, Push)> = None;
    let mut pushes = Vec::new();
    loop {
        let left = if let Some(t) = ring {
            // Give the SDP offer a few seconds to follow the ring.
            Duration::from_secs(8).saturating_sub(t.elapsed())
        } else {
            deadline.saturating_duration_since(Instant::now())
        };
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some((src, p))) => {
                log(&format!("push:{src}"), &json!({ "push_type": p.push_type, "fields": p.fields, "extra_params": p.extra_params }));
                pushes.push(json!({ "src": src, "push_type": p.push_type }));
                if p.push_type.ends_with("-incoming_call") && ring.is_none() {
                    ring = Some(Instant::now());
                }
                if p.rtc_type() == Some("offer") {
                    ring.get_or_insert_with(Instant::now);
                    offer = Some((src, p));
                    break;
                }
            }
            _ => break,
        }
    }
    summary["pushes_until_call"] = json!(pushes);
    summary["ring_received"] = json!(ring.is_some());

    let ice = call::ice_servers_from_turn(&api(&client, |c| c.turn()).await?);
    let token = api(&client, |c| c.token()).await?;
    let opts = |sub: &str| CallOptions {
        out_dir: Some(dir.join(sub)),
        max_duration: Duration::from_secs(30),
        ice_servers: ice.clone(),
        on_video: None,
        on_audio: None,
        mic: None,
        stop: None,
    };
    if let Some((src, p)) = offer {
        say(tr("SDP offer received: answering the call from this PC…", "Offerta SDP ricevuta: rispondo alla chiamata da questo PC…"));
        summary["offer_channel"] = json!(src);
        let sdp = p.extra_params["data"]["session_description"]["sdp"].as_str().unwrap_or_default().to_owned();
        let report = match p::session_from_push(&p.extra_params) {
            Some(session) => call::run(token.clone(), Mode::Answer { session, offer_sdp: sdp }, opts("answer"), log.clone()).await,
            None => Err(bticino_easykit::Error::Other("offer push without session ids".into())),
        };
        summary["answer"] = report_json(report);
    } else if ring.is_some() {
        say(tr(
            "Ring received but no SDP offer: trying a live view during the ring…",
            "Squillo ricevuto ma nessuna offerta SDP: provo la visione dal vivo durante lo squillo…",
        ));
        let report = call::run(token.clone(), Mode::Live { device_id: bridge.clone(), module_id: unit.clone() }, opts("live-during-ring"), log.clone()).await;
        summary["live_during_ring"] = report_json(report);
    } else {
        say(tr("No ring received.", "Nessuno squillo ricevuto."));
    }
    // Late pushes (missed/accepted call, terminate...) keep being logged.
    while let Ok(Some((src, p))) = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await {
        log(&format!("push:{src}"), &json!({ "push_type": p.push_type, "extra_params": p.extra_params }));
    }

    // 3. Door locks, one at a time, never without "ready".
    let mut locks = Vec::new();
    for m in home.by_kind(ModuleKind::DoorLock) {
        let name = if m.name.is_empty() { m.id.clone() } else { m.name.clone() };
        say(&format!(
            "{} \"{name}\".\n    {} {unlock_delay} s.\n    {}",
            tr("Next: OPEN", "Prossimo passo: APRO"),
            tr(
                "Go where you can see it, then type 'ready' (or 'skip') and Enter. It opens after a countdown of",
                "Mettiti dove lo vedi, poi scrivi 'pronto' (oppure 'salta') e premi Invio. Si apre dopo un conto alla rovescia di",
            ),
            tr("Nothing is sent before you confirm.", "Non invio nulla prima della tua conferma."),
        ));
        let a = ask("> ".into()).await;
        if !matches!(a.as_str(), "pronto" | "ready") {
            locks.push(json!({ "id": m.id, "name": name, "result": "skipped" }));
            continue;
        }
        for i in (1..=unlock_delay).rev() {
            print!("\r    {i:3} s ");
            let _ = std::io::stdout().flush();
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        println!();
        say(&format!("{} \"{name}\" ADESSO / NOW", tr("Opening", "Apro")));
        let (h, id) = (home.clone(), m.id.clone());
        let r = api(&client, move |c| c.set_state(&h, &id, p::unlock_state())).await;
        let sent = match &r {
            Ok(v) => {
                log("setstate", v);
                json!({ "ok": true, "response": v })
            }
            Err(e) => {
                log("setstate", &json!({ "error": e.to_string() }));
                json!({ "ok": false, "error": e.to_string() })
            }
        };
        let seen = ask(tr("Did it open? (y/n)", "Si è aperto? (s/n)").to_owned()).await;
        locks.push(json!({ "id": m.id, "name": name, "sent": sent, "user_saw_open": seen.starts_with('s') || seen.starts_with('y') }));
    }
    summary["locks"] = json!(locks);

    // 4. Cleanup.
    let r = api(&client, push::drop_push_context).await;
    summary["droppushcontext"] = match r {
        Ok(v) => json!(v),
        Err(e) => json!({ "error": e.to_string() }),
    };
    let s = serde_json::to_string_pretty(&summary).unwrap_or_default();
    std::fs::write(dir.join("summary.json"), &s)?;
    say(tr("Done. Summary:", "Finito. Riepilogo:"));
    println!("{s}");
    Ok(())
}

fn report_json(r: Result<call::CallReport>) -> Value {
    match r {
        Ok(rep) => serde_json::to_value(rep).unwrap_or(Value::Null),
        Err(e) => json!({ "error": e.to_string() }),
    }
}
