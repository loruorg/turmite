use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use serde::Deserialize;

use super::{DashResult, Proj, ProjectRole, Shell, User, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    crypto::random_hex,
    db::{ApiKeyRow, MemberRow, Page, PaymentFilter},
    http::views::{PaymentLine, StuckLine, payment_lines, stuck_lines},
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/projects/new", get(new_page).post(create))
        .route("/p/{pid}", get(overview))
        .route("/p/{pid}/settings", get(settings).post(save_settings))
        .route("/p/{pid}/settings/ipn-secret", post(reveal_ipn_secret))
        .route("/p/{pid}/settings/ipn-secret/rotate", post(rotate_ipn_secret))
        .route("/p/{pid}/team", get(team).post(add_member))
        .route("/p/{pid}/team/{uid}/remove", post(remove_member))
        .route("/p/{pid}/keys", get(keys).post(create_key))
        .route("/p/{pid}/keys/{kid}/revoke", post(revoke_key))
}

// ---- new project ---------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/project_new.html")]
struct NewPage {
    shell: Shell,
}

async fn new_page(State(app): State<App>, user: User) -> DashResult {
    user.require_admin()?;
    let shell = Shell::new(&app, &user, None, "", "New project").await;
    Ok(page(&app, &NewPage { shell }))
}

#[derive(Deserialize)]
struct NewForm {
    name: String,
}

async fn create(State(app): State<App>, user: User, Form(f): Form<NewForm>) -> DashResult {
    user.require_admin()?;
    let name = f.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Ok(err_redirect(&app, &app.path("/dashboard/projects/new"), "Use a name of 1 to 80 characters."));
    }
    let secret = app.sealer.seal_str("ipn_secret", &random_hex(24));
    let id = app.store.create_project(name, &secret).await?;
    user.audit(&app, Some(id), "project.create", "project", Some(&id.to_string()), serde_json::json!({ "name": name })).await;
    Ok(ok_redirect(&app, &app.path(&format!("/dashboard/p/{id}/wallets")), "Project created. Add a wallet to start taking payments."))
}

// ---- overview ------------------------------------------------------------------

struct Step {
    done: bool,
    label: &'static str,
    href: String,
}

struct Bar {
    x: i64,
    created: i64,
    paid: i64,
    delay: usize,
    title: String,
}

#[derive(Template)]
#[template(path = "dashboard/overview.html")]
struct OverviewPage {
    shell: Shell,
    pid: i64,
    volume: String,
    other_currencies: usize,
    paid: i64,
    open: i64,
    expired: i64,
    bars: Vec<Bar>,
    created_30d: i64,
    steps: Vec<Step>,
    setup_done: bool,
    recent: Vec<PaymentLine>,
    stuck: Vec<StuckLine>,
}

async fn overview(State(app): State<App>, p: Proj) -> DashResult {
    let pid = p.project.project_id;
    let since = Utc::now() - Duration::days(30);
    let counts = app.store.payment_status_counts(pid, since).await?;
    let count = |names: &[&str]| counts.iter().filter(|(s, _)| names.contains(&s.as_str())).map(|(_, n)| n).sum::<i64>();
    let base = p.base(&app);
    let wallets = app.store.project_wallets(pid).await?;
    let keys = app.store.list_api_keys(pid).await?;
    let hooks = app.store.list_webhook_endpoints(pid).await?;
    let steps = vec![
        Step { done: p.user.user.totp_enabled, label: "Turn on two-factor authentication", href: app.path("/dashboard/account") },
        Step {
            done: wallets.iter().any(|w| w.is_enabled),
            label: "Add a wallet for each coin you accept",
            href: format!("{base}/wallets"),
        },
        Step { done: keys.iter().any(|k| k.revoked_at.is_none()), label: "Create an API key for your shop", href: format!("{base}/keys") },
        Step { done: !hooks.is_empty(), label: "Add a webhook endpoint", href: format!("{base}/webhooks") },
    ];
    let setup_done = steps.iter().all(|s| s.done);
    let (rows, _) = app.store.list_payments(pid, &PaymentFilter::default(), Page::new(6, 0)).await?;
    let recent = payment_lines(&app, &rows, &base).await?;
    let stuck = stuck_lines(&app, pid, &base).await?;
    let daily = app.store.payment_daily(pid, 30).await?;
    let peak = daily.iter().map(|d| d.1.max(d.2)).max().unwrap_or(0).max(1);
    // Bars fill a 300 x 64 view box; a day with activity is at least 2 units high.
    let scale = |n: i64| if n == 0 { 0 } else { (n * 64 / peak).max(2) };
    let bars = daily
        .iter()
        .enumerate()
        .map(|(i, (day, created, paid))| Bar {
            x: i as i64 * 10,
            delay: i * 18,
            created: scale(*created),
            paid: scale(*paid),
            title: format!("{}: {created} created, {paid} paid", day.format("%b %-d")),
        })
        .collect();
    let volume = app.store.paid_volume(pid, since).await?;
    let (volume_text, other_currencies) = match volume.first() {
        Some((cur, sum)) => (format!("{:.2} {}", sum, cur.to_ascii_uppercase()), volume.len() - 1),
        None => ("0".to_string(), 0),
    };
    let shell = Shell::new(&app, &p.user, Some(&p.project), "overview", &p.project.name).await;
    Ok(page(
        &app,
        &OverviewPage {
            shell,
            pid,
            volume: volume_text,
            other_currencies,
            paid: count(&["finished"]),
            open: count(&["waiting", "confirming", "confirmed", "sending", "partially_paid"]),
            expired: count(&["expired", "failed"]),
            created_30d: daily.iter().map(|d| d.1).sum(),
            bars,
            steps,
            setup_done,
            recent,
            stuck,
        },
    ))
}

// ---- settings ------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/settings.html")]
struct SettingsPage {
    shell: Shell,
    pid: i64,
    name: String,
    signature_header: String,
    payment_expiry: i32,
    invoice_expiry: i32,
    can_edit: bool,
    ipn_secret: String,
}

async fn settings_view(app: &App, p: &Proj, ipn_secret: String) -> DashResult {
    let shell = Shell::new(app, &p.user, Some(&p.project), "settings", "Settings").await;
    Ok(page(
        app,
        &SettingsPage {
            shell,
            pid: p.project.project_id,
            name: p.project.name.clone(),
            signature_header: p.project.signature_header.clone(),
            payment_expiry: p.project.payment_expiry_minutes,
            invoice_expiry: p.project.invoice_expiry_minutes,
            can_edit: p.role >= ProjectRole::Admin,
            ipn_secret,
        },
    ))
}

async fn settings(State(app): State<App>, p: Proj) -> DashResult {
    settings_view(&app, &p, String::new()).await
}

#[derive(Deserialize)]
struct SettingsForm {
    name: String,
    signature_header: String,
    payment_expiry: i32,
    invoice_expiry: i32,
}

fn valid_header_name(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 64
        && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && axum::http::HeaderName::from_bytes(h.as_bytes()).is_ok()
}

async fn save_settings(State(app): State<App>, p: Proj, Form(f): Form<SettingsForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/settings", p.base(&app));
    let name = f.name.trim();
    let header = f.signature_header.trim().to_ascii_lowercase();
    if name.is_empty() || name.chars().count() > 80 {
        return Ok(err_redirect(&app, &back, "Use a name of 1 to 80 characters."));
    }
    if !valid_header_name(&header) || header.starts_with("x-turmite-") && header != "x-turmite-sig" {
        return Ok(err_redirect(&app, &back, "Use a header name of letters, digits and dashes."));
    }
    if !(5..=10080).contains(&f.payment_expiry) || !(5..=43200).contains(&f.invoice_expiry) {
        return Ok(err_redirect(&app, &back, "Payment expiry is 5 to 10080 minutes; invoice expiry is 5 to 43200 minutes."));
    }
    let pid = p.project.project_id;
    app.store.rename_project(pid, name).await?;
    app.store.set_project_payment_settings(pid, &header, f.payment_expiry, f.invoice_expiry).await?;
    p.audit(&app, "project.settings", "project", Some(&pid.to_string()), serde_json::json!({ "name": name, "signature_header": header, "payment_expiry": f.payment_expiry, "invoice_expiry": f.invoice_expiry })).await;
    Ok(ok_redirect(&app, &back, "Settings saved."))
}

#[derive(Deserialize)]
struct TotpForm {
    #[serde(default)]
    totp: String,
}

async fn reveal_ipn_secret(State(app): State<App>, p: Proj, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    let back = format!("{}/settings", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let secret = app.sealer.open_str("ipn_secret", &p.project.ipn_secret)?.to_string();
    p.audit(&app, "project.ipn_secret_view", "project", Some(&p.project.project_id.to_string()), serde_json::json!({})).await;
    settings_view(&app, &p, secret).await
}

async fn rotate_ipn_secret(State(app): State<App>, p: Proj, Form(f): Form<TotpForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/settings", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let secret = random_hex(24);
    app.store.set_project_ipn_secret(p.project.project_id, &app.sealer.seal_str("ipn_secret", &secret)).await?;
    p.audit(&app, "project.ipn_secret_rotate", "project", Some(&p.project.project_id.to_string()), serde_json::json!({})).await;
    let p = Proj { project: app.store.project(p.project.project_id).await?, ..p };
    settings_view(&app, &p, secret).await
}

// ---- team ----------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/team.html")]
struct TeamPage {
    shell: Shell,
    pid: i64,
    members: Vec<MemberRow>,
    can_edit: bool,
}

async fn team(State(app): State<App>, p: Proj) -> DashResult {
    let members = app.store.list_members(p.project.project_id).await?;
    let shell = Shell::new(&app, &p.user, Some(&p.project), "team", "Team").await;
    Ok(page(&app, &TeamPage { shell, pid: p.project.project_id, members, can_edit: p.role >= ProjectRole::Admin }))
}

#[derive(Deserialize)]
struct MemberForm {
    email: String,
    role: String,
    #[serde(default)]
    totp: String,
}

async fn add_member(State(app): State<App>, p: Proj, Form(f): Form<MemberForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/team", p.base(&app));
    if !matches!(f.role.as_str(), "project_admin" | "developer" | "read_only") {
        return Ok(err_redirect(&app, &back, "Choose a role."));
    }
    // A project admin controls wallets, keys and payouts.
    if f.role == "project_admin"
        && let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await
    {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let Some(u) = app.store.user_by_email(&f.email).await? else {
        return Ok(err_redirect(&app, &back, "No user has this email. A server admin adds users under Users."));
    };
    app.store.set_member(p.project.project_id, u.user_id, &f.role).await?;
    p.audit(&app, "member.set", "user", Some(&u.user_id.to_string()), serde_json::json!({ "email": u.email, "role": f.role })).await;
    Ok(ok_redirect(&app, &back, "Member saved."))
}

async fn remove_member(State(app): State<App>, p: Proj, Path((_pid, uid)): Path<(i64, i64)>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    app.store.remove_member(p.project.project_id, uid).await?;
    p.audit(&app, "member.remove", "user", Some(&uid.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &format!("{}/team", p.base(&app)), "Member removed."))
}

// ---- API keys ------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/keys.html")]
struct KeysPage {
    shell: Shell,
    pid: i64,
    keys: Vec<ApiKeyRow>,
    new_key: String,
    can_create: bool,
    can_revoke: bool,
    api_base: String,
}

async fn keys_view(app: &App, p: &Proj, new_key: String) -> DashResult {
    let keys = app.store.list_api_keys(p.project.project_id).await?;
    let shell = Shell::new(app, &p.user, Some(&p.project), "keys", "API keys").await;
    Ok(page(
        app,
        &KeysPage {
            shell,
            pid: p.project.project_id,
            keys,
            new_key,
            can_create: p.role >= ProjectRole::Admin,
            can_revoke: p.role >= ProjectRole::Developer,
            api_base: app.config.site_url.clone(),
        },
    ))
}

async fn keys(State(app): State<App>, p: Proj) -> DashResult {
    keys_view(&app, &p, String::new()).await
}

#[derive(Deserialize)]
struct KeyForm {
    label: String,
    #[serde(default)]
    totp: String,
}

async fn create_key(State(app): State<App>, p: Proj, Form(f): Form<KeyForm>) -> DashResult {
    p.require(ProjectRole::Admin)?;
    let back = format!("{}/keys", p.base(&app));
    if let Err(e) = p.user.check_totp(&app, Some(&f.totp)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let label = f.label.trim();
    let label = if label.is_empty() { "API key" } else { label };
    let (token, id) =
        app.store.create_api_key(p.project.project_id, &label.chars().take(80).collect::<String>(), Some(p.user.user.user_id)).await?;
    p.audit(&app, "api_key.create", "api_key", Some(&id.to_string()), serde_json::json!({ "label": label })).await;
    keys_view(&app, &p, token).await
}

async fn revoke_key(State(app): State<App>, p: Proj, Path((_pid, kid)): Path<(i64, i64)>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    let back = format!("{}/keys", p.base(&app));
    if app.store.revoke_api_key(p.project.project_id, kid).await? {
        p.audit(&app, "api_key.revoke", "api_key", Some(&kid.to_string()), serde_json::json!({})).await;
        return Ok(ok_redirect(&app, &back, "Key revoked."));
    }
    Ok(Redirect::to(&back).into_response())
}
