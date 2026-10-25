//! `turmite init`: secrets and settings for a new server.
//!
//! Writes `<dir>/.env` (read by Docker Compose) and `<dir>/secrets/`:
//! - `hot-identity.age`: the hot-wallet identity, encrypted with the passphrase;
//! - `hot_passphrase`: the passphrase, so the worker restarts unattended (skip with
//!   `--no-store-passphrase` and pass `TURMITE_HOT_PASSPHRASE` at start-up instead).
//!
//! The paths in `.env` are the paths inside the worker container (`/secrets/...`), as
//! `deploy/docker-compose.yml` mounts them. In the image the working directory is `/data`:
//! `docker run --rm -it --user 0:0 -v /opt/turmite:/data loruorg/turmite init --site-url ...`

use std::{
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

use clap::Args;

use crate::{
    config::{Mode, normalize_site_url},
    crypto::{new_hot_identity, random_hex},
    error::{Error, Result},
};

/// User and group of the worker in the image (distroless `nonroot`).
pub const IMAGE_USER: &str = "65532:65532";

#[derive(Args, Debug)]
pub struct InitArgs {
    /// Directory with docker-compose.yml. In the image: /data.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    /// mainnet or testnet.
    #[arg(long, default_value = "testnet")]
    pub mode: String,
    /// Public URL of the server, for example https://pay.example.com.
    #[arg(long)]
    pub site_url: String,
    /// Do not store the hot-wallet passphrase on disk.
    #[arg(long)]
    pub no_store_passphrase: bool,
    /// When run as root, give secrets/ to this user:group (the worker user of the image).
    #[arg(long, default_value = IMAGE_USER)]
    pub owner: String,
    /// Overwrite an existing .env and identity.
    #[arg(long)]
    pub force: bool,
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::Internal(format!("{}: {e}", path.display()))
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(io_err(path))?;
    f.write_all(data).map_err(io_err(path))
}

fn passphrase() -> Result<String> {
    if let Ok(p) = std::env::var("TURMITE_HOT_PASSPHRASE")
        && !p.is_empty()
    {
        return Ok(p);
    }
    if !std::io::stdin().is_terminal() {
        return Err(Error::BadRequest("set TURMITE_HOT_PASSPHRASE or run in a terminal (docker run -it)".into()));
    }
    let a = rpassword::prompt_password("Hot-wallet passphrase (12+ characters): ").map_err(|e| Error::Internal(e.to_string()))?;
    let b = rpassword::prompt_password("Repeat: ").map_err(|e| Error::Internal(e.to_string()))?;
    if a != b {
        return Err(Error::BadRequest("the passphrases do not match".into()));
    }
    Ok(a)
}

/// Caddy site address for a site URL. A URL with a path or a local host gets plain HTTP on
/// port 80: another proxy serves the public name, or there is no public name.
pub fn caddy_address(site_url: &str) -> String {
    let Ok(url) = url::Url::parse(site_url) else { return "http://:80".into() };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let local = host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local");
    if url.scheme() != "https" || url.path() != "/" || local { "http://:80".into() } else { host }
}

/// `uid:gid`.
fn parse_owner(s: &str) -> Result<(u32, u32)> {
    let (u, g) = s.split_once(':').unwrap_or((s, s));
    match (u.parse(), g.parse()) {
        (Ok(u), Ok(g)) => Ok((u, g)),
        _ => Err(Error::BadRequest(format!("--owner must be uid:gid, not `{s}`"))),
    }
}

/// True when the effective user is root (Linux; elsewhere false).
fn running_as_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).and_then(|l| l.split_whitespace().nth(2)).map(|uid| uid == "0"))
        .unwrap_or(false)
}

/// Give `secrets/` and its files to the worker user. Only root can, and only root needs to.
#[cfg(unix)]
fn hand_over(secrets: &Path, owner: (u32, u32)) -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, chown};
    fs::set_permissions(secrets, fs::Permissions::from_mode(0o700)).map_err(io_err(secrets))?;
    if !running_as_root() {
        return Ok(());
    }
    for entry in fs::read_dir(secrets).map_err(io_err(secrets))? {
        let path = entry.map_err(io_err(secrets))?.path();
        chown(&path, Some(owner.0), Some(owner.1)).map_err(io_err(&path))?;
    }
    chown(secrets, Some(owner.0), Some(owner.1)).map_err(io_err(secrets))
}

#[cfg(not(unix))]
fn hand_over(_secrets: &Path, _owner: (u32, u32)) -> Result<()> {
    Ok(())
}

pub fn run(args: InitArgs) -> Result<()> {
    let mode: Mode = args.mode.parse()?;
    let site_url = normalize_site_url(&args.site_url)?;
    if mode.is_mainnet() && !site_url.starts_with("https://") {
        return Err(Error::BadRequest("mainnet needs an https:// site URL".into()));
    }
    let owner = parse_owner(&args.owner)?;
    let env_path = args.dir.join(".env");
    let secrets = args.dir.join("secrets");
    let identity_path = secrets.join("hot-identity.age");
    if !args.force && (env_path.exists() || identity_path.exists()) {
        return Err(Error::BadRequest(format!(
            "{} or {} exists; pass --force to replace them (the old hot keys then become unreadable)",
            env_path.display(),
            identity_path.display()
        )));
    }
    let pass = passphrase()?;
    if pass.chars().count() < 12 {
        return Err(Error::BadRequest("use a passphrase of at least 12 characters".into()));
    }
    fs::create_dir_all(&secrets).map_err(io_err(&secrets))?;
    let (recipient, identity) = new_hot_identity(&pass)?;
    write_private(&identity_path, &identity)?;
    let pass_line = if args.no_store_passphrase {
        let _ = fs::remove_file(secrets.join("hot_passphrase"));
        "# The passphrase is not stored. Export TURMITE_HOT_PASSPHRASE before `docker compose up`.\n".to_string()
    } else {
        write_private(&secrets.join("hot_passphrase"), pass.as_bytes())?;
        "TURMITE_HOT_PASSPHRASE_FILE=/secrets/hot_passphrase\n".to_string()
    };
    hand_over(&secrets, owner)?;
    let db_password = random_hex(16);
    let env = format!(
        "# Written by `turmite init`. Keep a copy offline with secrets/.\n\
         # Image tag that docker compose runs. install.sh --upgrade changes it.\n\
         TURMITE_VERSION={version}\n\
         TURMITE_MODE={mode}\n\
         TURMITE_SITE_URL={site_url}\n\
         TURMITE_SECRET={secret}\n\
         TURMITE_HOT_RECIPIENT={recipient}\n\
         TURMITE_HOT_IDENTITY_FILE=/secrets/hot-identity.age\n\
         {pass_line}\
         POSTGRES_PASSWORD={db_password}\n\
         TURMITE_DATABASE_URL=postgres://turmite:{db_password}@postgres:5432/turmite\n\
         # Caddy site address: the host name gets automatic TLS; http://:80 is plain HTTP.\n\
         TURMITE_CADDY_ADDRESS={caddy}\n\
         # Proxies in front of Caddy (for example Cloudflare), with the private ranges, space-separated.\n\
         # TURMITE_TRUSTED_PROXIES=10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 173.245.48.0/20 ...\n\
         # Optional: email for payout approvals and security notices.\n\
         # TURMITE_SMTP_URL=smtps://user:password@smtp.example.com:465\n\
         # TURMITE_SMTP_FROM=Turmite <pay@example.com>\n",
        version = env!("CARGO_PKG_VERSION"),
        mode = mode.as_str(),
        secret = random_hex(32),
        caddy = caddy_address(&site_url),
    );
    write_private(&env_path, env.as_bytes())?;
    println!("Wrote {} and {}.", env_path.display(), secrets.display());
    println!("Back up both and the passphrase. Without them the hot keys and deposit seed cannot be read.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caddy_addresses() {
        assert_eq!(caddy_address("https://pay.example.com"), "pay.example.com");
        assert_eq!(caddy_address("https://Pay.Example.com/"), "pay.example.com");
        assert_eq!(caddy_address("https://example.com/pay"), "http://:80");
        assert_eq!(caddy_address("http://localhost"), "http://:80");
        assert_eq!(caddy_address("http://pay.example.com"), "http://:80");
        assert_eq!(caddy_address("https://turmite.localhost"), "http://:80");
    }

    #[test]
    fn owners() {
        assert_eq!(parse_owner("65532:65532").unwrap(), (65532, 65532));
        assert_eq!(parse_owner("1000").unwrap(), (1000, 1000));
        assert!(parse_owner("root").is_err());
    }
}
