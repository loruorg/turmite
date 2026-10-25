//! Server-rendered dashboard. Every action is a plain form post, so it works without JavaScript.

mod account;
mod admin;
mod auth;
mod checkout_settings;
mod hot_wallets;
mod payments;
mod payouts;
mod projects;
mod wallets;
mod webhooks;

use askama::Template;
use axum::{
    Router,
    extract::{FromRequestParts, OptionalFromRequestParts, Path},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use chrono::{Duration, Utc};

use crate::{
    app::App,
    auth::totp,
    crypto::sha256_hex,
    db::{ProjectRow, UserRow},
    error::Error,
    http::middleware::ClientIp,
};

pub const SESSION_COOKIE: &str = "turmite_session";
const FLASH_COOKIE: &str = "turmite_flash";
pub const SESSION_IDLE_HOURS: i64 = 12;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/", get(home))
        .merge(auth::routes())
        .merge(account::routes())
        .merge(projects::routes())
        .merge(checkout_settings::routes())
        .merge(payments::routes())
        .merge(payouts::routes())
        .merge(hot_wallets::routes())
        .merge(wallets::routes())
        .merge(webhooks::routes())
        .merge(admin::routes())
}

// ---- rendering ---------------------------------------------------------------

/// Render a template, or a plain error page when rendering fails.
pub fn render<T: Template>(t: &T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal error.").into_response()
        }
    }
}

/// A dashboard error: a page for GET failures, a flash and redirect back for form posts.
pub struct DashError(pub Error);

impl From<Error> for DashError {
    fn from(e: Error) -> Self {
        DashError(e)
    }
}

impl From<sqlx::Error> for DashError {
    fn from(e: sqlx::Error) -> Self {
        DashError(e.into())
    }
}

impl IntoResponse for DashError {
    fn into_response(self) -> Response {
        self.0.log();
        let status = self.0.status();
        let base = crate::http::BASE_PATH.get().cloned().unwrap_or_default();
        let page = ErrorPage { base, status: status.as_u16(), message: self.0.public_message() };
        let mut r = render(&page);
        *r.status_mut() = status;
        r
    }
}

pub type DashResult<T = Response> = std::result::Result<T, DashError>;

#[derive(Template)]
#[template(path = "dashboard/error.html")]
struct ErrorPage {
    base: String,
    status: u16,
    message: String,
}

// ---- flash messages ----------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Flash {
    pub kind: String,
    pub text: String,
}

fn cookie_path(app: &App) -> String {
    let p = app.config.base_path();
    if p.is_empty() { "/".into() } else { p.to_string() }
}

/// Redirect with a message shown once on the next page.
pub fn redirect_with(app: &App, to: &str, kind: &str, text: &str) -> Response {
    let mut r = Redirect::to(to).into_response();
    let value = format!(
        "{FLASH_COOKIE}={}; Path={}; Max-Age=60; HttpOnly; SameSite=Strict{}",
        url::form_urlencoded::byte_serialize(format!("{kind}|{text}").as_bytes()).collect::<String>(),
        cookie_path(app),
        if app.config.secure_cookies() { "; Secure" } else { "" }
    );
    if let Ok(v) = HeaderValue::from_str(&value) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

pub fn ok_redirect(app: &App, to: &str, text: &str) -> Response {
    redirect_with(app, to, "ok", text)
}

pub fn err_redirect(app: &App, to: &str, text: &str) -> Response {
    redirect_with(app, to, "error", text)
}

fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn read_flash(headers: &HeaderMap) -> Option<Flash> {
    let raw = read_cookie(headers, FLASH_COOKIE)?;
    let decoded: String = url::form_urlencoded::parse(format!("x={raw}").as_bytes()).next()?.1.into_owned();
    let (kind, text) = decoded.split_once('|')?;
    Some(Flash { kind: if kind == "error" { "error".into() } else { "ok".into() }, text: text.chars().take(500).collect() })
}

/// Clear the flash cookie once it was shown.
pub fn clear_flash(app: &App, mut r: Response) -> Response {
    let value = format!("{FLASH_COOKIE}=; Path={}; Max-Age=0; HttpOnly; SameSite=Strict", cookie_path(app));
    if let Ok(v) = HeaderValue::from_str(&value) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

// ---- session -----------------------------------------------------------------

pub fn session_cookie(app: &App, token: &str, max_age: i64) -> HeaderValue {
    let value = format!(
        "{SESSION_COOKIE}={token}; Path={}; Max-Age={max_age}; HttpOnly; SameSite=Strict{}",
        cookie_path(app),
        if app.config.secure_cookies() { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&value).expect("cookie value is ASCII")
}

/// The signed-in user. Pages that need a login redirect to it when there is none.
pub struct User {
    pub user: UserRow,
    pub session_hash: String,
    pub flash: Option<Flash>,
    pub ip: String,
}

pub struct LoginRedirect(Response);

impl IntoResponse for LoginRedirect {
    fn into_response(self) -> Response {
        self.0
    }
}

impl OptionalFromRequestParts<App> for User {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Option<Self>, Self::Rejection> {
        let Some(token) = read_cookie(&parts.headers, SESSION_COOKIE) else { return Ok(None) };
        let hash = sha256_hex(token.as_bytes());
        let extend = Utc::now() + Duration::hours(SESSION_IDLE_HOURS);
        let Ok(Some(su)) = app.store.session_user(&hash, extend).await else { return Ok(None) };
        let ip = <ClientIp as FromRequestParts<App>>::from_request_parts(parts, app).await.map(|c| c.key()).unwrap_or_default();
        Ok(Some(User { user: su.user, session_hash: su.session_hash, flash: read_flash(&parts.headers), ip }))
    }
}

impl FromRequestParts<App> for User {
    type Rejection = LoginRedirect;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        match <User as OptionalFromRequestParts<App>>::from_request_parts(parts, app).await {
            Ok(Some(u)) => Ok(u),
            _ => {
                let to = if app.store.user_count().await.unwrap_or(1) == 0 { "/dashboard/setup" } else { "/dashboard/login" };
                Err(LoginRedirect(Redirect::to(&app.path(to)).into_response()))
            }
        }
    }
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.user.is_admin()
    }

    pub fn require_admin(&self) -> Result<(), Error> {
        if self.is_admin() { Ok(()) } else { Err(Error::Forbidden("Only server admins can do this.".into())) }
    }

    pub fn require_root(&self) -> Result<(), Error> {
        if self.user.is_root() { Ok(()) } else { Err(Error::Forbidden("Only the root account can do this.".into())) }
    }

    /// Check a fresh TOTP code for a sensitive action. On mainnet two-factor must be on.
    pub async fn check_totp(&self, app: &App, code: Option<&str>) -> Result<(), Error> {
        let u = &self.user;
        if !u.totp_enabled {
            if app.config.mode.is_mainnet() {
                return Err(Error::Forbidden("Turn on two-factor authentication on your account page first.".into()));
            }
            return Ok(());
        }
        let key = format!("totp:{}", u.user_id);
        if app.limits.totp.is_blocked(&key) {
            return Err(Error::Forbidden("Too many wrong codes. Wait 15 minutes.".into()));
        }
        let code = code.unwrap_or("").trim();
        let secret = match &u.totp_secret {
            Some(s) => app.sealer.open("totp", s)?,
            None => return Err(Error::Internal("two-factor secret missing".into())),
        };
        let now = Utc::now().timestamp().max(0) as u64;
        match totp::verify(&secret, code, now, u.totp_last_step.max(0) as u64) {
            Some(step) if app.store.consume_totp_step(u.user_id, step as i64).await? => {
                app.limits.totp.reset(&key);
                Ok(())
            }
            _ => {
                app.limits.totp.hit(&key);
                Err(Error::BadRequest("The two-factor code is wrong or was already used.".into()))
            }
        }
    }

    pub async fn audit(
        &self,
        app: &App,
        project_id: Option<i64>,
        action: &str,
        entity: &str,
        entity_id: Option<&str>,
        data: serde_json::Value,
    ) {
        if let Err(e) =
            app.store.insert_audit(Some(self.user.user_id), project_id, "user", action, entity, entity_id, &data, Some(&self.ip)).await
        {
            tracing::error!(error = %e, action, "audit log write failed");
        }
    }
}

// ---- project access ----------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum ProjectRole {
    ReadOnly,
    Developer,
    Admin,
}

impl ProjectRole {
    fn from_str(s: &str) -> Self {
        match s {
            "project_admin" => ProjectRole::Admin,
            "developer" => ProjectRole::Developer,
            _ => ProjectRole::ReadOnly,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ProjectRole::Admin => "Project admin",
            ProjectRole::Developer => "Developer",
            ProjectRole::ReadOnly => "Read only",
        }
    }
}

/// A signed-in user inside one project (`/dashboard/p/{project_id}/...`).
pub struct Proj {
    pub user: User,
    pub project: ProjectRow,
    pub role: ProjectRole,
}

#[derive(serde::Deserialize)]
struct ProjectPath {
    pid: i64,
}

impl FromRequestParts<App> for Proj {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let user = <User as FromRequestParts<App>>::from_request_parts(parts, app).await.map_err(IntoResponse::into_response)?;
        let Path(pp) = <Path<ProjectPath> as FromRequestParts<App>>::from_request_parts(parts, app)
            .await
            .map_err(|_| DashError(Error::NotFound).into_response())?;
        let project = app.store.project(pp.pid).await.map_err(|e| DashError(e).into_response())?;
        let role = if user.is_admin() {
            ProjectRole::Admin
        } else {
            match app.store.member_role(pp.pid, user.user.user_id).await {
                Ok(Some(r)) => ProjectRole::from_str(&r),
                _ => return Err(DashError(Error::NotFound).into_response()),
            }
        };
        Ok(Proj { user, project, role })
    }
}

impl Proj {
    pub fn require(&self, min: ProjectRole) -> Result<(), Error> {
        if self.role >= min { Ok(()) } else { Err(Error::Forbidden(format!("This needs the {} role.", min.label().to_lowercase()))) }
    }

    pub fn base(&self, app: &App) -> String {
        app.path(&format!("/dashboard/p/{}", self.project.project_id))
    }

    pub async fn audit(&self, app: &App, action: &str, entity: &str, entity_id: Option<&str>, data: serde_json::Value) {
        self.user.audit(app, Some(self.project.project_id), action, entity, entity_id, data).await
    }
}

// ---- page shell --------------------------------------------------------------

pub struct NavItem {
    pub key: &'static str,
    pub label: &'static str,
    pub href: String,
}

/// Everything the layout needs: user, projects, navigation, notices.
pub struct Shell {
    pub title: String,
    pub base: String,
    pub email: String,
    pub initial: String,
    pub is_admin: bool,
    pub projects: Vec<(i64, String)>,
    pub project_id: Option<i64>,
    pub project_name: String,
    pub role_label: String,
    pub nav: Vec<NavItem>,
    pub admin_nav: Vec<NavItem>,
    pub active: &'static str,
    pub flash: Option<Flash>,
    pub notices: Vec<String>,
    /// New release and security advisory lines (admins only).
    pub updates: Vec<crate::updates::Banner>,
    pub demo: bool,
    pub testnet: bool,
}

impl Shell {
    pub async fn new(app: &App, user: &User, project: Option<&ProjectRow>, active: &'static str, title: &str) -> Shell {
        let base = app.config.base_path().to_string();
        let projects = app
            .store
            .projects_for_user(user.user.user_id, user.is_admin())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.project_id, p.name))
            .collect();
        let nav = match project {
            Some(p) => {
                let pb = format!("{base}/dashboard/p/{}", p.project_id);
                [
                    ("overview", "Overview", ""),
                    ("payments", "Payments", "/payments"),
                    ("invoices", "Invoices", "/invoices"),
                    ("wallets", "Wallets", "/wallets"),
                    ("payouts", "Payouts", "/payouts"),
                    ("keys", "API keys", "/keys"),
                    ("webhooks", "Webhooks", "/webhooks"),
                    ("settings", "Settings", "/settings"),
                    ("team", "Team", "/team"),
                ]
                .into_iter()
                .map(|(key, label, path)| NavItem { key, label, href: format!("{pb}{path}") })
                .collect()
            }
            None => vec![],
        };
        let admin_nav = if user.is_admin() {
            [
                ("networks", "Networks", "/dashboard/admin/networks"),
                ("hot-wallets", "Hot wallets", "/dashboard/admin/hot-wallets"),
                ("users", "Users", "/dashboard/admin/users"),
                ("audit", "Audit log", "/dashboard/admin/audit"),
                ("updates", "Updates", "/dashboard/admin/updates"),
            ]
            .into_iter()
            .map(|(key, label, path)| NavItem { key, label, href: format!("{base}{path}") })
            .collect()
        } else {
            vec![]
        };
        let mut notices = Vec::new();
        if !user.user.totp_enabled {
            notices.push("Two-factor authentication is off. Turn it on in your account.".to_string());
        }
        let role_label = match project {
            Some(p) if !user.is_admin() => app
                .store
                .member_role(p.project_id, user.user.user_id)
                .await
                .ok()
                .flatten()
                .map(|r| ProjectRole::from_str(&r).label().to_string())
                .unwrap_or_default(),
            _ => match user.user.role.as_str() {
                "root" => "Root".into(),
                "admin" => "Admin".into(),
                _ => "Member".into(),
            },
        };
        Shell {
            title: title.to_string(),
            base,
            email: user.user.email.clone(),
            initial: user.user.email.chars().next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default(),
            is_admin: user.is_admin(),
            projects,
            project_id: project.map(|p| p.project_id),
            project_name: project.map(|p| p.name.clone()).unwrap_or_default(),
            role_label,
            nav,
            admin_nav,
            active,
            flash: user.flash.clone(),
            notices,
            updates: crate::updates::banners(app, user.is_admin()).await,
            demo: app.demo,
            testnet: !app.config.mode.is_mainnet(),
        }
    }

    pub fn has_flash(&self) -> bool {
        self.flash.is_some()
    }
}

/// Render a page and clear the flash it showed.
pub fn page<T: Template>(app: &App, t: &T) -> Response {
    clear_flash(app, render(t))
}

async fn home(app: axum::extract::State<App>, user: User) -> Response {
    let projects = app.store.projects_for_user(user.user.user_id, user.is_admin()).await.unwrap_or_default();
    match projects.first() {
        Some(p) => Redirect::to(&app.path(&format!("/dashboard/p/{}", p.project_id))).into_response(),
        None if user.is_admin() => Redirect::to(&app.path("/dashboard/projects/new")).into_response(),
        None => Redirect::to(&app.path("/dashboard/account")).into_response(),
    }
}
