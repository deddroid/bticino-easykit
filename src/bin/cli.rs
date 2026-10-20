use std::io::Write;
use std::process::ExitCode;

use bticino_easykit::{Client, Home, ModuleKind, Result};
use clap::{Parser, Subcommand};

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
    /// Register for call notifications (FCM) and print them as they arrive
    Listen,
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
    let homes = c.homes()?;
    match id {
        Some(id) => homes.into_iter().find(|h| h.id == id),
        None => homes.into_iter().find(|h| h.bridge().is_some()),
    }
    .ok_or_else(|| bticino_easykit::Error::Other("no home with an intercom bridge found".into()))
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

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
