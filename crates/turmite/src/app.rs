//! Shared state of a running Turmite process.

use std::{ops::Deref, sync::Arc};

use zeroize::Zeroizing;

use crate::{
    auth::limits::Limits,
    chains::{
        DriverRegistry, PaymentDriver,
        registry::{self, BuildContext, Health},
    },
    config::{Config, Role},
    crypto::{self, HotIdentity, Sealer},
    db::PgStore,
    error::{Error, Result},
    events::EventBus,
    mail::Mailer,
    rates::{RateProvider, default_rate_provider},
};

/// Logged at start by a worker without `TURMITE_HOT_IDENTITY_FILE`.
pub const NO_HOT_IDENTITY: &str = "TURMITE_HOT_IDENTITY_FILE is not set: this worker has no hot-wallet identity. \
     It cannot make Tron, Solana or TON deposit addresses, sweep deposits or send payouts.";

#[derive(Clone)]
pub struct App(Arc<Inner>);

pub struct Inner {
    pub config: Config,
    pub store: PgStore,
    pub rates: Arc<dyn RateProvider>,
    pub events: EventBus,
    pub sealer: Sealer,
    pub mailer: Mailer,
    pub limits: Limits,
    pub role: Role,
    /// Mock chains and the simulate-deposit tool.
    pub demo: bool,
    pub http: reqwest::Client,
    /// Exchange API base other than the public one (tests and `--demo` use a stub).
    pub exchange_base: Option<String>,
    hot: Option<HotIdentity>,
    deposit_seed: Option<Zeroizing<String>>,
    drivers: std::sync::RwLock<(i64, DriverRegistry)>,
    fixed_drivers: bool,
    /// Which node serves each network (node health checks and failover).
    pub health: Health,
}

impl Deref for App {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

/// How to start an [`App`]. Tests set `drivers` and `rates`.
#[derive(Default)]
pub struct Options {
    pub role: Option<Role>,
    pub demo: bool,
    pub rates: Option<Arc<dyn RateProvider>>,
    /// Use these drivers instead of the nodes in the database.
    pub drivers: Option<DriverRegistry>,
    /// Unlocked hot identity. When absent, the worker loads it from the config.
    pub hot: Option<HotIdentity>,
    /// Exchange API base for tests. `--demo` starts a local stub.
    pub exchange_base: Option<String>,
}

impl App {
    pub async fn start(config: Config, opts: Options) -> Result<Self> {
        let role = opts.role.unwrap_or(Role::All);
        let store = PgStore::connect(&config.database_url).await?;
        store.seed_catalog(config.mode).await?;
        let sealer = Sealer::new(&config.secret);
        let mailer = Mailer::new(config.smtp.as_ref(), config.mode)?;

        let hot = match opts.hot {
            Some(h) => Some(h),
            None if role.runs_worker() => match (&config.hot_identity_file, &config.hot_passphrase) {
                (Some(path), Some(pass)) => Some(HotIdentity::load(path, pass)?),
                (Some(_), None) => {
                    return Err(Error::BadRequest(
                        "set TURMITE_HOT_PASSPHRASE or TURMITE_HOT_PASSPHRASE_FILE to unlock the hot-wallet identity".into(),
                    ));
                }
                _ => {
                    if !opts.demo {
                        tracing::warn!("{NO_HOT_IDENTITY}");
                    }
                    None
                }
            },
            None => None,
        };
        let recipient = hot.as_ref().map(HotIdentity::recipient).or_else(|| config.hot_recipient.clone());
        let deposit_seed = if opts.demo { None } else { ensure_deposit_seed(&store, recipient.as_deref(), hot.as_ref()).await? };

        let fixed_drivers = opts.drivers.is_some() || opts.demo;
        let health = Health::default();
        let drivers = match opts.drivers {
            Some(d) => d,
            None if opts.demo => DriverRegistry::all_mock(),
            None => {
                let ctx = BuildContext { store: &store, sealer: &sealer, deposit_seed: deposit_seed.as_deref().map(|s| s.as_str()) };
                registry::build_bound(&ctx, &health).await?
            }
        };
        let version = store.nodes_version().await?;
        let exchange_base = match opts.exchange_base {
            Some(base) => Some(base),
            None if opts.demo => Some(crate::exchange::stub::Stub::start(true).await?.base),
            None => None,
        };

        let app = App(Arc::new(Inner {
            rates: opts.rates.unwrap_or_else(default_rate_provider),
            events: EventBus::new(),
            limits: Limits::default(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .map_err(|e| Error::Internal(e.to_string()))?,
            demo: opts.demo,
            exchange_base,
            role,
            hot,
            deposit_seed,
            drivers: std::sync::RwLock::new((version, drivers)),
            fixed_drivers,
            health,
            config,
            store,
            sealer,
            mailer,
        }));
        if app.demo {
            crate::demo::setup(&app).await?;
        }
        Ok(app)
    }

    /// The current drivers. Cheap: the registry holds `Arc`s.
    pub fn drivers(&self) -> DriverRegistry {
        self.drivers.read().unwrap_or_else(|e| e.into_inner()).1.clone()
    }

    /// Rebuild the drivers when the nodes changed in the dashboard.
    pub async fn refresh_drivers(&self) -> Result<()> {
        if self.fixed_drivers {
            return Ok(());
        }
        let version = self.store.nodes_version().await?;
        if self.drivers.read().unwrap_or_else(|e| e.into_inner()).0 == version {
            return Ok(());
        }
        let ctx = BuildContext { store: &self.store, sealer: &self.sealer, deposit_seed: self.deposit_seed() };
        let reg = registry::build_bound(&ctx, &self.health).await?;
        *self.drivers.write().unwrap_or_else(|e| e.into_inner()) = (version, reg);
        Ok(())
    }

    /// Put `driver` in place of the current driver of its chain (node failover).
    pub fn replace_driver(&self, driver: Arc<dyn PaymentDriver>) {
        self.drivers.write().unwrap_or_else(|e| e.into_inner()).1.insert(driver);
    }

    /// True when the drivers come from tests or the demo, not from the nodes in the database.
    pub fn drivers_fixed(&self) -> bool {
        self.fixed_drivers
    }

    /// The install deposit seed (worker only).
    pub(crate) fn deposit_seed(&self) -> Option<&str> {
        self.deposit_seed.as_deref().map(|s| s.as_str())
    }

    pub fn hot_identity(&self) -> Option<&HotIdentity> {
        self.hot.as_ref()
    }

    /// age recipient for new hot keys.
    pub fn hot_recipient(&self) -> Option<String> {
        self.hot.as_ref().map(HotIdentity::recipient).or_else(|| self.config.hot_recipient.clone())
    }

    /// Absolute URL of a path under the site URL.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.site_url, path)
    }

    /// Absolute path (for links and redirects) of a path under the site URL.
    pub fn path(&self, path: &str) -> String {
        format!("{}{}", self.config.base_path(), path)
    }
}

/// Load the install deposit seed, creating it on first start. Seed-derived chains
/// (Tron, Solana, TON) derive deposit keys from it. Stored age-encrypted.
async fn ensure_deposit_seed(store: &PgStore, recipient: Option<&str>, hot: Option<&HotIdentity>) -> Result<Option<Zeroizing<String>>> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let stored: Option<String> = store.setting("deposit_seed").await?;
    let stored = match (stored, recipient) {
        (Some(s), _) => s,
        (None, Some(recipient)) => {
            let seed = Zeroizing::new(crypto::random_hex(32));
            let ct = crypto::hot_encrypt(recipient, seed.as_bytes())?;
            store.init_setting("deposit_seed", &b64.encode(ct)).await?.unwrap_or_default()
        }
        (None, None) => return Ok(None),
    };
    let Some(hot) = hot else { return Ok(None) };
    let ct = b64.decode(stored).map_err(|e| Error::Internal(format!("deposit seed: {e}")))?;
    let plain = hot
        .decrypt(&ct)
        .map_err(|_| Error::BadRequest("the hot-wallet identity cannot read the deposit seed (wrong identity file?)".into()))?;
    let seed = String::from_utf8(plain.to_vec()).map_err(|_| Error::Internal("deposit seed is not UTF-8".into()))?;
    Ok(Some(Zeroizing::new(seed)))
}
