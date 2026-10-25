use std::io::IsTerminal;

use clap::{Parser, Subcommand};
use turmite::{App, Config, Options, chains::registry, config::Role};

#[derive(Parser)]
#[command(name = "turmite", version, about = "Self-hosted crypto payment gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (the default command).
    Serve {
        /// all: everything in one process. api: HTTP only, no private keys. worker: chains, sweeps, webhooks.
        #[arg(long, default_value = "all")]
        role: String,
        /// Simulated chains and a ready project, for a local trial.
        #[arg(long)]
        demo: bool,
    },
    /// Write deploy/.env and the hot-wallet identity for a new server.
    Init(turmite::init::InitArgs),
    /// Run database migrations and exit.
    Migrate,
    /// Account tools.
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Deposit address counters.
    Counters {
        #[command(subcommand)]
        command: CounterCommand,
    },
    /// Print the version.
    Version,
    /// Exit with 0 when the server on TURMITE_LISTEN answers /healthz (container health check).
    Healthcheck,
}

#[derive(Subcommand)]
enum UserCommand {
    /// Turn off two-factor for a user who lost their device.
    Reset2fa { email: String },
}

#[derive(Subcommand)]
enum CounterCommand {
    /// After a database restore: skip indexes that were maybe handed out after the dump,
    /// and delete the free pool addresses. Stop the worker first.
    Raise {
        /// Indexes to skip on each counter. Use more than the number of payments made after the dump.
        #[arg(long)]
        by: i64,
    },
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn,tower_http=warn"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::io::stdout().is_terminal() {
        builder.init();
    } else {
        builder.json().flatten_event(true).init();
    }
}

/// GET /healthz on the local listener. The image has no curl, so the binary checks itself.
fn healthcheck() -> turmite::Result<()> {
    use std::{
        io::{Read, Write},
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
        time::Duration,
    };
    let fail = |m: String| turmite::Error::Internal(format!("health check: {m}"));
    let listen: SocketAddr = std::env::var("TURMITE_LISTEN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "0.0.0.0:8080".into())
        .parse()
        .map_err(|e| fail(format!("TURMITE_LISTEN: {e}")))?;
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let timeout = Duration::from_secs(4);
    let mut stream = TcpStream::connect_timeout(&SocketAddr::new(ip, listen.port()), timeout).map_err(|e| fail(e.to_string()))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| fail(e.to_string()))?;
    stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n").map_err(|e| fail(e.to_string()))?;
    let mut body = String::new();
    stream.take(4096).read_to_string(&mut body).map_err(|e| fail(e.to_string()))?;
    let status_ok = body.split_whitespace().nth(1) == Some("200");
    if status_ok && body.trim_end().ends_with("ok") { Ok(()) } else { Err(fail(body.lines().next().unwrap_or("no answer").to_string())) }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> turmite::Result<()> {
    match cli.command.unwrap_or(Command::Serve { role: "all".into(), demo: false }) {
        Command::Version => {
            println!("turmite {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Healthcheck => healthcheck(),
        Command::Init(args) => turmite::init::run(args),
        Command::Migrate => {
            init_logging();
            let config = Config::from_env()?;
            turmite::db::PgStore::connect(&config.database_url).await?;
            println!("migrations applied");
            Ok(())
        }
        Command::User { command: UserCommand::Reset2fa { email } } => {
            let config = Config::from_env()?;
            let store = turmite::db::PgStore::connect(&config.database_url).await?;
            let user =
                store.user_by_email(&email).await?.ok_or_else(|| turmite::Error::BadRequest(format!("no user with email {email}")))?;
            store.disable_totp(user.user_id).await?;
            store.delete_user_sessions(user.user_id).await?;
            store
                .insert_audit(
                    None,
                    None,
                    "system",
                    "user.2fa_reset",
                    "user",
                    Some(&user.user_id.to_string()),
                    &serde_json::json!({ "via": "cli" }),
                    None,
                )
                .await?;
            println!("two-factor is off for {email}; they sign in with their password and set it up again");
            Ok(())
        }
        Command::Counters { command: CounterCommand::Raise { by } } => {
            if !(1..=1_000_000).contains(&by) {
                return Err(turmite::Error::BadRequest("--by must be 1 to 1000000".into()));
            }
            let config = Config::from_env()?;
            let store = turmite::db::PgStore::connect(&config.database_url).await?;
            let deleted = store.raise_address_counters(by).await?;
            let data = serde_json::json!({ "by": by, "pool_deleted": deleted, "via": "cli" });
            store.insert_audit(None, None, "system", "counters.raise", "network", None, &data, None).await?;
            println!("address counters moved {by} on; {deleted} free pool addresses deleted. The worker makes new ones.");
            Ok(())
        }
        Command::Serve { role, demo } => {
            init_logging();
            let role: Role = role.parse()?;
            let config = if demo { Config::from_env_demo()? } else { Config::from_env()? };
            let rates = demo.then(|| std::sync::Arc::new(turmite::rates::FixedRateProvider::demo()) as _);
            let app = App::start(config, Options { role: Some(role), demo, rates, ..Default::default() }).await?;
            if demo {
                tracing::info!("demo mode: open {}/dashboard to create the first account", app.config.site_url);
            }
            if role.runs_worker() {
                turmite::worker::spawn(app.clone());
            } else {
                // The API needs working drivers too (address checks, derivations).
                tokio::spawn(registry::health_loop(app.clone(), registry::API_PROBE));
            }
            if role.serves_http() {
                turmite::http::serve(app).await
            } else {
                turmite::http::shutdown_signal().await;
                Ok(())
            }
        }
    }
}
