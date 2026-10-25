use std::{env, net::SocketAddr};

use ipnet::IpNet;

use crate::error::{Error, Result};

/// Network set seeded at first start. An install keeps its mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Mainnet,
    Testnet,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Mainnet => "mainnet",
            Mode::Testnet => "testnet",
        }
    }
    pub fn is_mainnet(self) -> bool {
        self == Mode::Mainnet
    }
}

impl std::str::FromStr for Mode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "mainnet" => Ok(Mode::Mainnet),
            "testnet" => Ok(Mode::Testnet),
            other => Err(Error::BadRequest(format!("TURMITE_MODE must be mainnet or testnet, not `{other}`"))),
        }
    }
}

/// What the process runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    /// API, checkout, dashboard and the worker in one process.
    All,
    /// API, checkout and dashboard. Holds no private key.
    Api,
    /// Watchers, sweeps, payouts, webhook delivery. Opens no port.
    Worker,
}

impl Role {
    pub fn serves_http(self) -> bool {
        matches!(self, Role::All | Role::Api)
    }
    pub fn runs_worker(self) -> bool {
        matches!(self, Role::All | Role::Worker)
    }
}

impl std::str::FromStr for Role {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "all" => Ok(Role::All),
            "api" => Ok(Role::Api),
            "worker" => Ok(Role::Worker),
            other => Err(Error::BadRequest(format!("role must be all, api or worker, not `{other}`"))),
        }
    }
}

/// SMTP settings from `TURMITE_SMTP_URL` and `TURMITE_SMTP_FROM`.
#[derive(Clone, Debug)]
pub struct Smtp {
    pub url: String,
    pub from: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    /// Public URL without a trailing slash, e.g. `https://pay.example.com` or `https://example.com/pay`.
    pub site_url: String,
    /// 32-byte server secret.
    pub secret: [u8; 32],
    pub mode: Mode,
    pub listen: SocketAddr,
    pub trusted_proxies: Vec<IpNet>,
    /// age public key that encrypts hot keys and the deposit seed.
    pub hot_recipient: Option<String>,
    /// age identity file (worker), encrypted with the passphrase.
    pub hot_identity_file: Option<String>,
    pub hot_passphrase: Option<String>,
    pub smtp: Option<Smtp>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let var = |k: &str| env::var(k).ok().filter(|v| !v.trim().is_empty());
        let database_url = var("TURMITE_DATABASE_URL").unwrap_or_else(|| "postgres://turmite:turmite@127.0.0.1:5433/turmite".to_string());
        let site_url = normalize_site_url(&var("TURMITE_SITE_URL").unwrap_or_else(|| "http://localhost:8080".into()))?;
        let secret = match var("TURMITE_SECRET") {
            Some(hex) => parse_secret(&hex)?,
            None => return Err(Error::BadRequest("set TURMITE_SECRET (64 hex characters; `turmite init` writes one)".into())),
        };
        let mode = var("TURMITE_MODE").unwrap_or_else(|| "testnet".into()).parse()?;
        let listen = var("TURMITE_LISTEN")
            .unwrap_or_else(|| "0.0.0.0:8080".into())
            .parse()
            .map_err(|e| Error::BadRequest(format!("TURMITE_LISTEN: {e}")))?;
        let trusted_proxies = match var("TURMITE_TRUSTED_PROXIES") {
            Some(list) => parse_cidrs(&list)?,
            None => default_trusted_proxies(),
        };
        let hot_passphrase = match var("TURMITE_HOT_PASSPHRASE") {
            Some(p) => Some(p),
            None => match var("TURMITE_HOT_PASSPHRASE_FILE") {
                Some(path) => Some(
                    std::fs::read_to_string(&path)
                        .map_err(|e| Error::BadRequest(format!("read {path}: {e}")))?
                        .trim_end_matches(['\r', '\n'])
                        .to_string(),
                ),
                None => None,
            },
        };
        let smtp =
            var("TURMITE_SMTP_URL").map(|url| Smtp { url, from: var("TURMITE_SMTP_FROM").unwrap_or_else(|| "turmite@localhost".into()) });
        let config = Self {
            database_url,
            site_url,
            secret,
            mode,
            listen,
            trusted_proxies,
            hot_recipient: var("TURMITE_HOT_RECIPIENT"),
            hot_identity_file: var("TURMITE_HOT_IDENTITY_FILE"),
            hot_passphrase,
            smtp,
        };
        config.check()?;
        Ok(config)
    }

    /// `--demo`: like `from_env`, but the secret is optional and the mode is testnet.
    pub fn from_env_demo() -> Result<Self> {
        if env::var("TURMITE_SECRET").map(|v| v.trim().is_empty()).unwrap_or(true) {
            // SAFETY: called once at start-up, before any thread reads the environment.
            unsafe { env::set_var("TURMITE_SECRET", crate::crypto::random_hex(32)) };
        }
        let mut c = Self::from_env()?;
        c.mode = Mode::Testnet;
        Ok(c)
    }

    /// Mainnet needs an https site URL.
    pub fn check(&self) -> Result<()> {
        if self.mode.is_mainnet() && !self.site_url.starts_with("https://") {
            return Err(Error::BadRequest("mainnet needs an https:// TURMITE_SITE_URL".into()));
        }
        Ok(())
    }

    /// Path part of the site URL (`""` or `/pay`), never with a trailing slash.
    pub fn base_path(&self) -> &str {
        site_path(&self.site_url)
    }

    /// Host of the site URL, lowercase, without the port.
    pub fn site_host(&self) -> String {
        url::Url::parse(&self.site_url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default()
    }

    pub fn secure_cookies(&self) -> bool {
        self.site_url.starts_with("https://")
    }

    /// A config for tests and `--demo`: fixed secret, given database.
    pub fn for_tests(database_url: &str, site_url: &str) -> Self {
        Self {
            database_url: database_url.into(),
            site_url: normalize_site_url(site_url).unwrap_or_else(|_| site_url.into()),
            secret: [7u8; 32],
            mode: Mode::Testnet,
            listen: "127.0.0.1:0".parse().unwrap(),
            trusted_proxies: default_trusted_proxies(),
            hot_recipient: None,
            hot_identity_file: None,
            hot_passphrase: None,
            smtp: None,
        }
    }
}

fn site_path(site_url: &str) -> &str {
    let after_scheme = site_url.split_once("://").map(|(_, r)| r).unwrap_or(site_url);
    match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => "",
    }
}

pub fn normalize_site_url(raw: &str) -> Result<String> {
    let url = url::Url::parse(raw.trim()).map_err(|e| Error::BadRequest(format!("TURMITE_SITE_URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::BadRequest("TURMITE_SITE_URL must be an http(s) URL".into()));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(Error::BadRequest("TURMITE_SITE_URL must not have a query or fragment".into()));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub fn parse_secret(hex_str: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_str.trim()).map_err(|_| Error::BadRequest("TURMITE_SECRET must be hex".into()))?;
    bytes.try_into().map_err(|_| Error::BadRequest("TURMITE_SECRET must be 64 hex characters (32 bytes)".into()))
}

fn parse_cidrs(list: &str) -> Result<Vec<IpNet>> {
    list.split([' ', ','])
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<IpNet>()
                .or_else(|_| s.parse::<std::net::IpAddr>().map(IpNet::from))
                .map_err(|_| Error::BadRequest(format!("TURMITE_TRUSTED_PROXIES: bad range `{s}`")))
        })
        .collect()
}

fn default_trusted_proxies() -> Vec<IpNet> {
    ["127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "::1/128", "fc00::/7"].iter().map(|s| s.parse().unwrap()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_url_path() {
        assert_eq!(site_path("https://pay.example.com"), "");
        assert_eq!(site_path("https://example.com/pay"), "/pay");
        assert_eq!(normalize_site_url("https://example.com/pay/").unwrap(), "https://example.com/pay");
        assert_eq!(normalize_site_url("http://localhost:8080/").unwrap(), "http://localhost:8080");
        assert!(normalize_site_url("ftp://x").is_err());
    }

    #[test]
    fn secret_length() {
        assert!(parse_secret(&"ab".repeat(32)).is_ok());
        assert!(parse_secret("abcd").is_err());
    }
}
