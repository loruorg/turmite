use askama::Template;
use axum::{
    Form, Router,
    extract::State,
    response::Response,
    routing::{get, post},
};
use chrono::Utc;
use serde::Deserialize;

use super::{DashResult, Shell, User, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    auth::{self, totp},
    http::checkout::qr_svg,
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/account", get(account))
        .route("/account/password", post(change_password))
        .route("/account/2fa/start", post(start_totp))
        .route("/account/2fa/confirm", post(confirm_totp))
        .route("/account/2fa/disable", post(disable_totp))
}

#[derive(Template)]
#[template(path = "dashboard/account.html")]
struct AccountPage {
    shell: Shell,
    totp_enabled: bool,
    totp_pending: bool,
    totp_qr: String,
    totp_key: String,
    created: String,
    last_login: String,
}

async fn account(State(app): State<App>, user: User) -> DashResult {
    let shell = Shell::new(&app, &user, None, "account", "Account").await;
    let u = &user.user;
    let (pending, qr, key) = match (&u.totp_secret, u.totp_enabled) {
        (Some(sealed), false) => {
            let secret = app.sealer.open("totp", sealed)?;
            let uri = totp::otpauth_uri(&secret, &format!("Turmite {}", app.config.site_host()), &u.email);
            (true, qr_svg(&uri), totp::base32(&secret))
        }
        _ => (false, String::new(), String::new()),
    };
    Ok(page(
        &app,
        &AccountPage {
            shell,
            totp_enabled: u.totp_enabled,
            totp_pending: pending,
            totp_qr: qr,
            totp_key: key,
            created: u.created_at.format("%Y-%m-%d").to_string(),
            last_login: u.last_login_at.map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default(),
        },
    ))
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    password: String,
    password2: String,
}

async fn change_password(State(app): State<App>, user: User, Form(f): Form<PasswordForm>) -> DashResult {
    let back = app.path("/dashboard/account");
    if !auth::verify_password(&f.current, &user.user.password_hash).await {
        return Ok(err_redirect(&app, &back, "The current password is wrong."));
    }
    if f.password != f.password2 {
        return Ok(err_redirect(&app, &back, "The new passwords do not match."));
    }
    if let Err(e) = auth::check_password_policy(&f.password) {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let hash = auth::hash_password(&f.password).await?;
    app.store.set_user_password(user.user.user_id, &hash).await?;
    app.store.delete_other_sessions(user.user.user_id, &user.session_hash).await?;
    user.audit(&app, None, "user.password", "user", Some(&user.user.user_id.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &back, "Password changed. Your other sessions are signed out."))
}

async fn start_totp(State(app): State<App>, user: User) -> DashResult {
    let back = app.path("/dashboard/account");
    if user.user.totp_enabled {
        return Ok(err_redirect(&app, &back, "Two-factor is already on."));
    }
    let sealed = app.sealer.seal("totp", &totp::new_secret());
    app.store.set_pending_totp(user.user.user_id, &sealed).await?;
    Ok(ok_redirect(&app, &back, "Scan the code with your authenticator app, then enter a code to confirm."))
}

#[derive(Deserialize)]
struct CodeForm {
    code: String,
}

async fn confirm_totp(State(app): State<App>, user: User, Form(f): Form<CodeForm>) -> DashResult {
    let back = app.path("/dashboard/account");
    let Some(sealed) = &user.user.totp_secret else {
        return Ok(err_redirect(&app, &back, "Start the setup first."));
    };
    let secret = app.sealer.open("totp", sealed)?;
    let now = Utc::now().timestamp().max(0) as u64;
    match totp::verify(&secret, &f.code, now, 0) {
        Some(step) => {
            app.store.enable_totp(user.user.user_id, step as i64).await?;
            user.audit(&app, None, "user.2fa_on", "user", Some(&user.user.user_id.to_string()), serde_json::json!({})).await;
            Ok(ok_redirect(&app, &back, "Two-factor authentication is on."))
        }
        None => Ok(err_redirect(&app, &back, "The code is wrong. Check the time on your phone and try again.")),
    }
}

async fn disable_totp(State(app): State<App>, user: User, Form(f): Form<CodeForm>) -> DashResult<Response> {
    let back = app.path("/dashboard/account");
    if !user.user.totp_enabled {
        app.store.disable_totp(user.user.user_id).await?;
        return Ok(ok_redirect(&app, &back, "Setup cancelled."));
    }
    if let Err(e) = user.check_totp(&app, Some(&f.code)).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    app.store.disable_totp(user.user.user_id).await?;
    user.audit(&app, None, "user.2fa_off", "user", Some(&user.user.user_id.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &back, "Two-factor authentication is off."))
}
