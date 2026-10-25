use askama::Template;
use axum::{
    Form, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use serde::Deserialize;

use super::{DashResult, SESSION_IDLE_HOURS, User, read_cookie, render, session_cookie};
use crate::{
    app::App,
    auth::{self, totp},
    crypto::{random_hex, sha256_hex},
    db::UserRow,
    error::Error,
    http::middleware::ClientIp,
};

const PENDING_COOKIE: &str = "turmite_login";

pub fn routes() -> Router<App> {
    Router::new()
        .route("/setup", get(setup_page).post(setup))
        .route("/login", get(login_page).post(login))
        .route("/login/code", get(code_page).post(code))
        .route("/logout", post(logout))
}

#[derive(Template)]
#[template(path = "dashboard/auth.html")]
struct AuthPage {
    base: String,
    mode: &'static str,
    error: String,
    email: String,
    testnet: bool,
}

fn auth_page(app: &App, mode: &'static str, error: &str, email: &str) -> Response {
    render(&AuthPage {
        base: app.config.base_path().to_string(),
        mode,
        error: error.to_string(),
        email: email.to_string(),
        testnet: !app.config.mode.is_mainnet(),
    })
}

#[derive(Deserialize)]
struct SetupForm {
    email: String,
    password: String,
    password2: String,
}

async fn setup_page(State(app): State<App>) -> DashResult {
    if app.store.user_count().await? > 0 {
        return Ok(Redirect::to(&app.path("/dashboard/login")).into_response());
    }
    Ok(auth_page(&app, "setup", "", ""))
}

/// The first account becomes root and gets a first project.
async fn setup(State(app): State<App>, ip: ClientIp, Form(f): Form<SetupForm>) -> DashResult {
    if app.store.user_count().await? > 0 {
        return Ok(Redirect::to(&app.path("/dashboard/login")).into_response());
    }
    let email = f.email.trim().to_string();
    if !email.contains('@') || email.len() > 254 {
        return Ok(auth_page(&app, "setup", "Enter a valid email address.", &email));
    }
    if f.password != f.password2 {
        return Ok(auth_page(&app, "setup", "The passwords do not match.", &email));
    }
    if let Err(e) = auth::check_password_policy(&f.password) {
        return Ok(auth_page(&app, "setup", &e.public_message(), &email));
    }
    let hash = auth::hash_password(&f.password).await?;
    let user_id = match app.store.create_user(&email, &hash, "root").await {
        Ok(id) => id,
        Err(Error::Conflict(_)) => return Ok(Redirect::to(&app.path("/dashboard/login")).into_response()),
        Err(e) => return Err(e.into()),
    };
    if app.store.list_projects().await?.is_empty() {
        let secret = app.sealer.seal_str("ipn_secret", &random_hex(24));
        app.store.create_project("My shop", &secret).await?;
    }
    app.store
        .insert_audit(
            Some(user_id),
            None,
            "user",
            "user.setup",
            "user",
            Some(&user_id.to_string()),
            &serde_json::json!({}),
            Some(&ip.key()),
        )
        .await?;
    start_session(&app, user_id, &ip, &HeaderMap::new()).await
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

async fn login_page(State(app): State<App>, user: Option<User>) -> DashResult {
    if user.is_some() {
        return Ok(Redirect::to(&app.path("/dashboard")).into_response());
    }
    if app.store.user_count().await? == 0 {
        return Ok(Redirect::to(&app.path("/dashboard/setup")).into_response());
    }
    Ok(auth_page(&app, "login", "", ""))
}

async fn login(State(app): State<App>, ip: ClientIp, headers: HeaderMap, Form(f): Form<LoginForm>) -> DashResult {
    let email = f.email.trim().to_lowercase();
    if !app.limits.login.hit(&format!("ip:{}", ip.key())) || !app.limits.login.hit(&format!("email:{email}")) {
        return Ok(auth_page(&app, "login", "Too many attempts. Wait a minute.", &email));
    }
    let user = app.store.user_by_email(&email).await?;
    let hash = user.as_ref().map(|u| u.password_hash.clone()).unwrap_or_else(|| auth::DUMMY_HASH.clone());
    let valid = auth::verify_password(&f.password, &hash).await;
    let user = match (valid, user) {
        (true, Some(u)) if u.disabled_at.is_none() => u,
        _ => return Ok(auth_page(&app, "login", "Wrong email or password.", &email)),
    };
    if user.totp_enabled {
        let mut r = Redirect::to(&app.path("/dashboard/login/code")).into_response();
        r.headers_mut().append(header::SET_COOKIE, pending_cookie(&app, &user, 300));
        return Ok(r);
    }
    start_session(&app, user.user_id, &ip, &headers).await
}

fn pending_value(app: &App, user: &UserRow, exp: i64) -> String {
    let body = format!("{}.{exp}", user.user_id);
    let mac = app.sealer.mac("login", format!("{body}.{}", user.password_hash).as_bytes());
    format!("{body}.{mac}")
}

fn pending_cookie(app: &App, user: &UserRow, max_age: i64) -> HeaderValue {
    let exp = Utc::now().timestamp() + max_age;
    let value = format!(
        "{PENDING_COOKIE}={}; Path={}; Max-Age={max_age}; HttpOnly; SameSite=Strict{}",
        pending_value(app, user, exp),
        if app.config.base_path().is_empty() { "/" } else { app.config.base_path() },
        if app.config.secure_cookies() { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&value).expect("ascii")
}

async fn pending_user(app: &App, headers: &HeaderMap) -> Option<UserRow> {
    let raw = read_cookie(headers, PENDING_COOKIE)?;
    let mut parts = raw.splitn(3, '.');
    let user_id: i64 = parts.next()?.parse().ok()?;
    let exp: i64 = parts.next()?.parse().ok()?;
    if exp < Utc::now().timestamp() {
        return None;
    }
    let user = app.store.user(user_id).await.ok()?;
    let want = pending_value(app, &user, exp);
    crate::crypto::ct_eq(want.as_bytes(), raw.as_bytes()).then_some(user)
}

async fn code_page(State(app): State<App>, headers: HeaderMap) -> DashResult {
    if pending_user(&app, &headers).await.is_none() {
        return Ok(Redirect::to(&app.path("/dashboard/login")).into_response());
    }
    Ok(auth_page(&app, "code", "", ""))
}

#[derive(Deserialize)]
struct CodeForm {
    code: String,
}

async fn code(State(app): State<App>, ip: ClientIp, headers: HeaderMap, Form(f): Form<CodeForm>) -> DashResult {
    let Some(user) = pending_user(&app, &headers).await else {
        return Ok(Redirect::to(&app.path("/dashboard/login")).into_response());
    };
    let key = format!("totp:{}", user.user_id);
    if app.limits.totp.is_blocked(&key) {
        return Ok(auth_page(&app, "code", "Too many wrong codes. Wait 15 minutes.", ""));
    }
    let secret = match &user.totp_secret {
        Some(s) => app.sealer.open("totp", s)?,
        None => return Ok(auth_page(&app, "login", "Two-factor is not set up for this account.", "")),
    };
    let now = Utc::now().timestamp().max(0) as u64;
    match totp::verify(&secret, &f.code, now, user.totp_last_step.max(0) as u64) {
        Some(step) if app.store.consume_totp_step(user.user_id, step as i64).await? => {
            app.limits.totp.reset(&key);
            let mut r = start_session(&app, user.user_id, &ip, &headers).await?;
            r.headers_mut().append(header::SET_COOKIE, pending_cookie(&app, &user, 0));
            Ok(r)
        }
        _ => {
            app.limits.totp.hit(&key);
            Ok(auth_page(&app, "code", "The code is wrong or was already used.", ""))
        }
    }
}

async fn start_session(app: &App, user_id: i64, ip: &ClientIp, headers: &HeaderMap) -> DashResult {
    let token = random_hex(32);
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let expires = Utc::now() + Duration::hours(SESSION_IDLE_HOURS);
    app.store.create_session(&sha256_hex(token.as_bytes()), user_id, Some(&ip.key()), ua, expires).await?;
    app.store.touch_login(user_id).await?;
    app.store
        .insert_audit(
            Some(user_id),
            None,
            "user",
            "user.login",
            "user",
            Some(&user_id.to_string()),
            &serde_json::json!({}),
            Some(&ip.key()),
        )
        .await?;
    let mut r = Redirect::to(&app.path("/dashboard")).into_response();
    r.headers_mut().append(header::SET_COOKIE, session_cookie(app, &token, 7 * 24 * 3600));
    Ok(r)
}

async fn logout(State(app): State<App>, user: Option<User>) -> DashResult {
    if let Some(u) = user {
        app.store.delete_session(&u.session_hash).await?;
    }
    let mut r = Redirect::to(&app.path("/dashboard/login")).into_response();
    r.headers_mut().append(header::SET_COOKIE, session_cookie(&app, "", 0));
    Ok(r)
}
