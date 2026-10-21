use std::io::Write;
use std::process::ExitCode;

use bticino_easykit::{Client, Home, ModuleKind, Result};
use clap::{Parser, Subcommand};

#[path = "cli/session.rs"]
mod session;

#[derive(Parser)]
#[command(version, about = "Control a BTicino EASYKIT WiFi video door entry kit")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Log in with your Home + Security account (password is not stored)
    Login {
        #[arg(long)]
        email: Option<String>,
    },
    /// Forget the stored tokens
    Logout,
    /// List homes and modules (read-only)
    List {
        /// Print the raw homesdata JSON
        #[arg(long)]
        raw: bool,
    },
    /// Show live module status (read-only)
    Status {
        #[arg(long)]
        home: Option<String>,
    },
    /// Open a door lock (by name or id); asks for confirmation unless --yes
    Open {
        /// Lock name (e.g. "Cancello Pedonale") or module id; omit to list locks
        lock: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Switch a staircase light on or off
    Light {
        #[arg(value_parser = ["on", "off"])]
        state: String,
        /// Light name or id (default: the first one)
        #[arg(long)]
        id: Option<String>,
    },
    /// Register for call notifications (FCM) and print them as they arrive
    Listen,
    /// Live view of the external unit: saves video.h264 + audio.ogg
    Live {
        /// Seconds before hanging up
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        /// Output directory (default: ./live-<timestamp>)
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Guided on-site test: ring, answer, record, open each lock (asks first)
    TestSession {
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Minutes to wait for the ring
        #[arg(long, default_value_t = 10)]
        wait_minutes: u64,
        /// Countdown before each lock opens, after you confirm
        #[arg(long, default_value_t = 10)]
        unlock_delay: u64,
        /// Read confirmations from <dir>/answer-N files instead of the terminal
        #[arg(long)]
        answers_dir: Option<std::path::PathBuf>,
    },
    /// Stop call notifications to this computer (undoes `listen`)
    UnregisterPush,
    /// Show recent events: calls, door openings... (read-only)
    Events {
        #[arg(long)]
        home: Option<String>,
        #[arg(long, default_value_t = 10)]
        size: u32,
    },
}

fn pick_home(c: &mut Client, id: Option<&str>) -> Result<Home> {
    match id {
        Some(id) => c
            .homes()?
            .into_iter()
            .find(|h| h.id == id)
            .ok_or_else(|| bticino_easykit::Error::Other(format!("no home {id}"))),
        None => bticino_easykit::actions::default_home(c),
    }
}

fn run(cli: Cli) -> Result<()> {
    let mut c = Client::new()?;
    match cli.cmd {
        Cmd::Login { email } => {
            let email = match email {
                Some(e) => e,
                None => {
                    print!("Email: ");
                    std::io::stdout().flush()?;
                    let mut s = String::new();
                    std::io::stdin().read_line(&mut s)?;
                    s.trim().to_owned()
                }
            };
            let pw = rpassword::prompt_password("Password: ")?;
            c.login(&email, &pw)?;
            println!("Logged in.");
        }
        Cmd::Logout => {
            c.logout()?;
            println!("Tokens removed.");
        }
        Cmd::List { raw } => {
            if raw {
                println!("{:#}", c.homesdata_raw()?);
                return Ok(());
            }
            for h in c.homes()? {
                println!("Home \"{}\" id={} tz={}", h.name, h.id, h.timezone.as_deref().unwrap_or("-"));
                for m in &h.modules {
                    let fw = m.raw["firmware_revision"].as_u64().map(|f| f.to_string()).unwrap_or_default();
                    println!(
                        "  {:<5} {:<15} {:<28} {:?} fw={} bridge={}",
                        m.type_,
                        m.raw["variant"].as_str().unwrap_or("-"),
                        m.id,
                        m.kind,
                        fw,
                        m.bridge.as_deref().unwrap_or("-")
                    );
                    if !m.name.is_empty() {
                        println!("        name: {}", m.name);
                    }
                }
                if h.by_kind(ModuleKind::EasykitBridge).next().is_some() {
                    println!("  -> EASYKIT (BDIY) variant detected");
                }
            }
        }
        Cmd::Open { lock, yes } => {
            let h = bticino_easykit::actions::default_home(&mut c)?;
            let locks: Vec<_> = h.by_kind(ModuleKind::DoorLock).collect();
            let Some(lock) = lock else {
                for m in &locks {
                    println!("{}  {}", m.id, m.name);
                }
                return Ok(());
            };
            let m = locks
                .iter()
                .find(|m| m.id == lock || m.name.eq_ignore_ascii_case(&lock))
                .ok_or_else(|| bticino_easykit::Error::Other(format!("no lock named {lock:?}")))?;
            if !yes {
                print!("Open \"{}\"? [y/N] ", m.name);
                std::io::stdout().flush()?;
                let mut a = String::new();
                std::io::stdin().read_line(&mut a)?;
                if !matches!(a.trim().to_lowercase().as_str(), "y" | "yes" | "s" | "si" | "sì") {
                    println!("Cancelled.");
                    return Ok(());
                }
            }
            bticino_easykit::actions::unlock(&mut c, &h, &m.id)?;
            println!("Open command sent to \"{}\".", m.name);
        }
        Cmd::Light { state, id } => {
            let h = bticino_easykit::actions::default_home(&mut c)?;
            let lights = bticino_easykit::actions::staircase_lights(&mut c, &h)?;
            let (lid, name) = match &id {
                Some(i) => lights.iter().find(|(lid, n)| lid == i || n.eq_ignore_ascii_case(i)),
                None => lights.first(),
            }
            .cloned()
            .ok_or_else(|| bticino_easykit::Error::Other("no staircase light found".into()))?;
            bticino_easykit::actions::light(&mut c, &h, &lid, state == "on")?;
            println!("Light \"{name}\" {state}.");
        }
        Cmd::Listen => {
            let (fcm, st, new) = bticino_easykit::push::load_or_register()?;
            println!("FCM {} (android_id {}).", if new { "registered" } else { "identity loaded" }, st.android_id);
            let r = bticino_easykit::push::add_push_context(&mut c, &st.fcm_token)?;
            println!("addpushcontext: {r}");
            println!("Listening... ring the doorbell (Ctrl+C to stop).");
            bticino_easykit::push::listen(fcm, |p| {
                println!("\n== {} rtc={:?}", p.push_type, p.rtc_type());
                for (k, v) in &p.fields {
                    if k != "extra_params" {
                        println!("  {k}: {v}");
                    }
                }
                println!("  extra_params: {:#}", p.extra_params);
            })?;
        }
        Cmd::Live { seconds, out } => {
            let h = pick_home(&mut c, None)?;
            let bridge = h.bridge().ok_or_else(|| bticino_easykit::Error::Other("no bridge".into()))?.id.clone();
            let unit = h.by_kind(ModuleKind::ExternalUnit).next().map(|m| m.id.clone());
            let ice = bticino_easykit::call::ice_servers_from_turn(&c.turn()?);
            let token = c.token()?;
            let out = out.unwrap_or_else(|| format!("live-{}", unix_now()).into());
            println!("Live view from {} ({} ICE servers) -> {}", unit.as_deref().unwrap_or("default unit"), ice.len(), out.display());
            let log: bticino_easykit::call::Log = std::sync::Arc::new(|src, v| println!("[{src}] {v}"));
            let rt = tokio::runtime::Runtime::new()?;
            let report = rt.block_on(bticino_easykit::call::run(
                token,
                bticino_easykit::call::Mode::Live { device_id: bridge, module_id: unit },
                bticino_easykit::call::CallOptions {
                    out_dir: Some(out),
                    max_duration: std::time::Duration::from_secs(seconds),
                    ice_servers: ice,
                    on_video: None,
                    on_audio: None,
                    mic: None,
                    stop: None,
                },
                log,
            ))?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        }
        Cmd::TestSession { out, wait_minutes, unlock_delay, answers_dir } => {
            session::run(out, wait_minutes, unlock_delay, answers_dir)?
        }
        Cmd::UnregisterPush => match bticino_easykit::push::drop_push_context(&mut c)? {
            Some(r) => println!("droppushcontext: {r}"),
            None => println!("No push registration saved."),
        },
        Cmd::Status { home } => {
            let h = pick_home(&mut c, home.as_deref())?;
            println!("{:#}", c.homestatus(&h.id)?);
        }
        Cmd::Events { home, size } => {
            let h = pick_home(&mut c, home.as_deref())?;
            println!("{:#}", c.events(&h.id, size)?);
        }
    }
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn main() -> ExitCode {
    // RUST_LOG=webrtc=debug,rtc=debug shows ICE/DTLS internals.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off")).try_init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
