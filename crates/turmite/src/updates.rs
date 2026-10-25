//! Update checks: the latest release and the published security advisories of
//! loruorg/turmite on GitHub. A minute after the worker starts, then once a day.
//! The requests carry `User-Agent: turmite/<version>` and no other data.
//!
//! A build whose version has a `dev` pre-release part (`0.2.0-dev`) is a development
//! build: advisories never apply to it.

use std::{cmp::Ordering, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    app::App,
    error::{Error, Result},
};

pub const RELEASE_URL: &str = "https://api.github.com/repos/loruorg/turmite/releases/latest";
pub const ADVISORIES_URL: &str = "https://api.github.com/repos/loruorg/turmite/security-advisories?state=published";
pub const RELEASES_PAGE: &str = "https://github.com/loruorg/turmite/releases";
pub const UPGRADE_COMMAND: &str = "curl -fsSL https://turmite.com/install.sh | sudo sh -s -- --upgrade";

/// Settings: check on or off (default on), last result, dismissed release banner.
const ENABLED_KEY: &str = "update_check";
const STATE_KEY: &str = "update_state";
const DISMISSED_KEY: &str = "update_dismissed";

/// Package names in an advisory that mean this server (others are the SDK or the plugin).
const PACKAGES: &[&str] = &["turmite", "loruorg/turmite", "ghcr.io/loruorg/turmite"];

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---- versions ------------------------------------------------------------------

/// A semantic version (`1.2.3`, `v1.2.3-rc.1+build`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pre: Vec<Ident>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    // Numeric identifiers sort before alphanumeric ones.
    Num(u64),
    Alpha(String),
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut parts = core.split('.');
        let mut num = || -> Option<u64> {
            match parts.next() {
                Some(p) => p.parse().ok(),
                None => Some(0),
            }
        };
        let (major, minor, patch) = (num()?, num()?, num()?);
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            Some("") => return None,
            Some(p) => p
                .split('.')
                .map(|i| match i.parse() {
                    Ok(n) => Ident::Num(n),
                    Err(_) => Ident::Alpha(i.to_string()),
                })
                .collect(),
            None => vec![],
        };
        Some(Self { major, minor, patch, pre })
    }

    /// A development build (`0.2.0-dev`).
    pub fn is_dev(&self) -> bool {
        self.pre.iter().any(|i| matches!(i, Ident::Alpha(a) if a.eq_ignore_ascii_case("dev")))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| {
            // A release sorts after its pre-releases.
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (i, p) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match p {
                Ident::Num(n) => write!(f, "{n}")?,
                Ident::Alpha(a) => f.write_str(a)?,
            }
        }
        Ok(())
    }
}

/// Does `v` fall in a GitHub `vulnerable_version_range` (`>= 1.0.0, < 1.2.3`, `= 0.4.1`,
/// `<= 2.0`)? Constraints joined by `,` must all hold; `||` joins alternatives.
/// `None` when the range cannot be read.
pub fn range_matches(range: &str, v: &Version) -> Option<bool> {
    let mut any = false;
    for alt in range.split("||") {
        let mut all = true;
        for c in alt.split(',').map(str::trim).filter(|c| !c.is_empty()) {
            if c == "*" {
                continue;
            }
            let (op, rest) =
                [">=", "<=", "==", ">", "<", "="].iter().find_map(|op| c.strip_prefix(op).map(|r| (*op, r))).unwrap_or(("=", c));
            let bound = Version::parse(rest)?;
            let ord = v.cmp(&bound);
            all &= match op {
                ">=" => ord != Ordering::Less,
                "<=" => ord != Ordering::Greater,
                ">" => ord == Ordering::Greater,
                "<" => ord == Ordering::Less,
                _ => ord == Ordering::Equal,
            };
        }
        any |= all;
    }
    Some(any)
}

// ---- stored state ----------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateState {
    pub checked_at: Option<DateTime<Utc>>,
    /// Error of the last check, when it failed.
    pub error: Option<String>,
    pub latest: Option<Release>,
    pub advisories: Vec<Advisory>,
    /// Advisories already emailed to the admins.
    #[serde(default)]
    pub emailed: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Advisory {
    pub id: String,
    pub summary: String,
    /// critical, high, medium or low.
    pub severity: String,
    pub url: String,
    /// Affected version ranges; empty means every version.
    pub ranges: Vec<String>,
    pub patched: Vec<String>,
    pub published_at: Option<DateTime<Utc>>,
}

impl Advisory {
    /// True when the advisory applies to `v`. Development builds get none. A range that
    /// cannot be read counts as a match.
    pub fn affects(&self, v: &Version) -> bool {
        if v.is_dev() {
            return false;
        }
        self.ranges.is_empty() || self.ranges.iter().any(|r| range_matches(r, v).unwrap_or(true))
    }

    pub fn is_severe(&self) -> bool {
        matches!(self.severity.as_str(), "critical" | "high")
    }
}

fn parse_release(v: &Value) -> Option<Release> {
    let tag = v.get("tag_name")?.as_str()?;
    Version::parse(tag)?;
    Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: v.get("html_url").and_then(Value::as_str).unwrap_or(RELEASES_PAGE).to_string(),
        published_at: v.get("published_at").and_then(Value::as_str).and_then(|s| s.parse().ok()),
    })
}

/// Advisories for this server from the GitHub repository advisories API.
fn parse_advisories(v: &Value) -> Vec<Advisory> {
    let text = |a: &Value, k: &str| a.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|a| a.get("withdrawn_at").is_none_or(Value::is_null))
        .filter_map(|a| {
            let vulns: Vec<&Value> = a.get("vulnerabilities").and_then(Value::as_array).map(|v| v.iter().collect()).unwrap_or_default();
            let ours: Vec<&Value> = vulns
                .iter()
                .copied()
                .filter(|x| match x.pointer("/package/name").and_then(Value::as_str) {
                    Some(name) if !name.is_empty() => PACKAGES.contains(&name.to_ascii_lowercase().as_str()),
                    _ => true,
                })
                .collect();
            if !vulns.is_empty() && ours.is_empty() {
                return None;
            }
            let field = |k: &str| -> Vec<String> {
                ours.iter().filter_map(|x| x.get(k).and_then(Value::as_str)).filter(|s| !s.trim().is_empty()).map(str::to_string).collect()
            };
            Some(Advisory {
                id: text(a, "ghsa_id"),
                summary: text(a, "summary"),
                severity: text(a, "severity").to_ascii_lowercase(),
                url: text(a, "html_url"),
                ranges: field("vulnerable_version_range"),
                patched: field("patched_versions"),
                published_at: a.get("published_at").and_then(Value::as_str).and_then(|s| s.parse().ok()),
            })
        })
        .filter(|a| !a.id.is_empty())
        .collect()
}

pub async fn enabled(app: &App) -> bool {
    app.store.setting::<bool>(ENABLED_KEY).await.ok().flatten().unwrap_or(true)
}

pub async fn set_enabled(app: &App, on: bool) -> Result<()> {
    app.store.set_setting(ENABLED_KEY, &on).await
}

pub async fn state(app: &App) -> UpdateState {
    app.store.setting::<UpdateState>(STATE_KEY).await.ok().flatten().unwrap_or_default()
}

/// Hide the banner for `version` (a newer release shows it again).
pub async fn dismiss(app: &App, version: &str) -> Result<()> {
    app.store.set_setting(DISMISSED_KEY, &version).await
}

// ---- checking ----------------------------------------------------------------------

async fn get_json(app: &App, url: &str) -> Result<Option<Value>> {
    let resp = app
        .http
        .get(url)
        .header(reqwest::header::USER_AGENT, format!("turmite/{}", current()))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::Unavailable(format!("GitHub: {e}")))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(Error::Unavailable(format!("GitHub answered {}", resp.status())));
    }
    resp.json().await.map(Some).map_err(|e| Error::Unavailable(format!("GitHub: {e}")))
}

/// Ask GitHub now, save the result and email new severe advisories to the admins.
pub async fn check(app: &App) -> UpdateState {
    let mut state = state(app).await;
    let fetched = async {
        let release = get_json(app, RELEASE_URL).await?;
        let advisories = get_json(app, ADVISORIES_URL).await?;
        Ok::<_, Error>((release.as_ref().and_then(parse_release), advisories.as_ref().map(parse_advisories).unwrap_or_default()))
    }
    .await;
    state.checked_at = Some(Utc::now());
    match fetched {
        Ok((latest, advisories)) => {
            state.error = None;
            state.latest = latest;
            state.advisories = advisories;
        }
        Err(e) => {
            tracing::warn!(error = %e, "update check failed");
            state.error = Some(e.to_string());
        }
    }
    notify(app, &mut state).await;
    if let Err(e) = app.store.set_setting(STATE_KEY, &state).await {
        tracing::warn!(error = %e, "cannot save the update check");
    }
    state
}

/// Email each critical or high advisory that affects this version once, when SMTP is set.
async fn notify(app: &App, state: &mut UpdateState) {
    let Some(cur) = Version::parse(current()) else { return };
    if !app.mailer.is_configured() {
        return;
    }
    let due: Vec<Advisory> =
        state.advisories.iter().filter(|a| a.is_severe() && a.affects(&cur) && !state.emailed.contains(&a.id)).cloned().collect();
    if due.is_empty() {
        return;
    }
    let admins = match app.store.admin_emails().await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "cannot list admins for the advisory email");
            return;
        }
    };
    for a in due {
        let subject = format!("Turmite security advisory {} ({})", a.id, a.severity);
        let body = format!(
            "A security advisory affects the Turmite server at {site}.\n\n\
             {id}: {summary}\n\
             Severity: {severity}\n\
             Affected versions: {ranges}\n\
             Fixed in: {patched}\n\
             Details: {url}\n\n\
             This server runs Turmite {cur}. Upgrade on the server with:\n\
             {cmd}\n\n\
             Admins of this server get this email once per advisory.\n",
            site = app.config.site_url,
            id = a.id,
            summary = a.summary,
            severity = a.severity,
            ranges = if a.ranges.is_empty() { "all".to_string() } else { a.ranges.join("; ") },
            patched = if a.patched.is_empty() { "not yet".to_string() } else { a.patched.join("; ") },
            url = a.url,
            cmd = UPGRADE_COMMAND,
        );
        let mut sent = false;
        for to in &admins {
            match app.mailer.send(to, &subject, &body).await {
                Ok(()) => sent = true,
                Err(e) => tracing::warn!(error = %e, advisory = %a.id, "advisory email failed"),
            }
        }
        if sent {
            state.emailed.push(a.id);
        }
    }
}

/// Worker loop: a minute after start, then daily. A worker that restarts does not ask
/// again when the last check is less than an hour old.
pub async fn run(app: App) {
    tokio::time::sleep(Duration::from_secs(60)).await;
    let mut every = chrono::Duration::hours(1);
    loop {
        if enabled(&app).await && state(&app).await.checked_at.is_none_or(|t| Utc::now() - t >= every) {
            check(&app).await;
        }
        every = chrono::Duration::hours(24);
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

// ---- dashboard banners ---------------------------------------------------------------

/// A line at the top of every dashboard page for admins.
pub struct Banner {
    pub text: String,
    pub href: &'static str,
    pub link: &'static str,
    /// Red; a security advisory. It stays until the server runs a fixed version.
    pub bad: bool,
    /// Shows a "Dismiss" button (new release banner).
    pub dismiss: bool,
}

pub async fn banners(app: &App, is_admin: bool) -> Vec<Banner> {
    if !is_admin || !enabled(app).await {
        return vec![];
    }
    let Some(cur) = Version::parse(current()) else { return vec![] };
    let state = state(app).await;
    let mut out: Vec<Banner> = state
        .advisories
        .iter()
        .filter(|a| a.affects(&cur))
        .map(|a| Banner {
            text: format!("Security advisory {} ({}) affects this version: {}", a.id, a.severity, a.summary),
            href: "/dashboard/admin/updates",
            link: "Upgrade",
            bad: true,
            dismiss: false,
        })
        .collect();
    if let Some(latest) = state.latest.as_ref()
        && Version::parse(&latest.version).is_some_and(|l| l > cur)
        && app.store.setting::<String>(DISMISSED_KEY).await.ok().flatten().as_deref() != Some(latest.version.as_str())
    {
        out.push(Banner {
            text: format!("Turmite {} is available. This server runs {cur}.", latest.version),
            href: "/dashboard/admin/updates",
            link: "Updates",
            bad: false,
            dismiss: true,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_versions() {
        assert_eq!(v("1.2.3"), Version { major: 1, minor: 2, patch: 3, pre: vec![] });
        assert_eq!(v("v0.4.0"), v("0.4.0"));
        assert_eq!(v("1.2"), v("1.2.0"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert_eq!(v("1.0.0-rc.1").to_string(), "1.0.0-rc.1");
        assert!(Version::parse("").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("1.x").is_none());
        assert!(Version::parse("1.0.0-").is_none());
        assert!(v("0.2.0-dev").is_dev());
        assert!(!v("0.2.0").is_dev());
        assert!(!v("0.2.0-rc.1").is_dev());
    }

    #[test]
    fn orders_versions() {
        let order = [
            "0.1.0",
            "0.1.1",
            "0.2.0-alpha",
            "0.2.0-alpha.1",
            "0.2.0-alpha.beta",
            "0.2.0-beta.2",
            "0.2.0-beta.11",
            "0.2.0-rc.1",
            "0.2.0",
            "1.0.0",
            "1.10.0",
        ];
        for w in order.windows(2) {
            assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        }
        assert_eq!(v("1.0.0").cmp(&v("v1.0.0")), Ordering::Equal);
    }

    #[test]
    fn matches_ranges() {
        let x = v("1.4.2");
        assert_eq!(range_matches("< 1.4.3", &x), Some(true));
        assert_eq!(range_matches("<= 1.4.2", &x), Some(true));
        assert_eq!(range_matches("< 1.4.2", &x), Some(false));
        assert_eq!(range_matches(">= 1.0.0, < 1.4.3", &x), Some(true));
        assert_eq!(range_matches(">= 1.5.0, < 2.0.0", &x), Some(false));
        assert_eq!(range_matches("= 1.4.2", &x), Some(true));
        assert_eq!(range_matches("1.4.2", &x), Some(true));
        assert_eq!(range_matches("> 1.4.2", &x), Some(false));
        assert_eq!(range_matches("< 1.0 || >= 1.4.0, < 1.5", &x), Some(true));
        assert_eq!(range_matches(">=1.4.0,<1.4.1", &x), Some(false));
        assert_eq!(range_matches("*", &x), Some(true));
        assert_eq!(range_matches("< banana", &x), None);
        // A pre-release of the fixed version is still affected.
        assert_eq!(range_matches("< 1.5.0", &v("1.5.0-rc.1")), Some(true));
    }

    #[test]
    fn reads_github_answers() {
        let release = serde_json::json!({ "tag_name": "v0.3.1", "html_url": "https://github.com/loruorg/turmite/releases/tag/v0.3.1", "published_at": "2026-10-01T10:00:00Z" });
        let r = parse_release(&release).unwrap();
        assert_eq!(r.version, "0.3.1");
        assert!(r.published_at.is_some());
        assert!(parse_release(&serde_json::json!({ "tag_name": "nightly" })).is_none());

        let list = serde_json::json!([
            { "ghsa_id": "GHSA-aaaa-bbbb-cccc", "summary": "Webhook signature bypass", "severity": "high",
              "html_url": "https://github.com/loruorg/turmite/security/advisories/GHSA-aaaa-bbbb-cccc",
              "published_at": "2026-09-01T00:00:00Z", "withdrawn_at": null,
              "vulnerabilities": [{ "package": { "ecosystem": "other", "name": "turmite" },
                                    "vulnerable_version_range": ">= 0.1.0, < 0.3.1", "patched_versions": "0.3.1" }] },
            { "ghsa_id": "GHSA-sdk", "summary": "SDK issue", "severity": "critical", "html_url": "x",
              "vulnerabilities": [{ "package": { "ecosystem": "npm", "name": "@turmite/sdk" }, "vulnerable_version_range": "< 9.0.0" }] },
            { "ghsa_id": "GHSA-gone", "summary": "Withdrawn", "severity": "low", "html_url": "x", "withdrawn_at": "2026-09-02T00:00:00Z" },
            { "ghsa_id": "GHSA-all", "summary": "No package data", "severity": "medium", "html_url": "x" }
        ]);
        let a = parse_advisories(&list);
        assert_eq!(a.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["GHSA-aaaa-bbbb-cccc", "GHSA-all"]);
        assert_eq!(a[0].ranges, [">= 0.1.0, < 0.3.1"]);
        assert!(a[0].is_severe());
        assert!(a[0].affects(&v("0.3.0")));
        assert!(!a[0].affects(&v("0.3.1")));
        assert!(!a[0].affects(&v("0.3.0-dev")), "development builds get no advisories");
        assert!(a[1].affects(&v("0.3.1")), "no range means every version");
    }
}
