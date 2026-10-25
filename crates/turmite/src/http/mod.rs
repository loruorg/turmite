//! HTTP server: merchant API, checkout, dashboard and static assets.

pub mod assets;
pub mod checkout;
pub mod dashboard;
pub mod glyphs;
pub mod merchant;
pub mod middleware;
pub mod payouts_api;
pub mod views;

use std::net::SocketAddr;

use axum::{
    Router,
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use tower::ServiceExt;
use tower_http::trace::TraceLayer;

use crate::{app::App, error::Result};

/// Site URL path, for pages rendered without the app at hand.
pub static BASE_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn router(app: App) -> Router {
    let _ = BASE_PATH.set(app.config.base_path().to_string());
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/", get(root))
        .merge(merchant::routes().merge(payouts_api::routes()).method_not_allowed_fallback(api_method_not_allowed))
        .merge(checkout::routes())
        .merge(assets::routes())
        .nest("/dashboard", dashboard::routes())
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(app.clone(), middleware::guard))
        .layer(TraceLayer::new_for_http())
        .with_state(app)
}

async fn root(axum::extract::State(app): axum::extract::State<App>) -> Redirect {
    Redirect::to(&app.path("/dashboard"))
}

async fn not_found(uri: axum::http::Uri) -> Response {
    if middleware::area(uri.path()) == middleware::Area::Api {
        return crate::error::Error::NotFound.into_response();
    }
    (StatusCode::NOT_FOUND, "Not found.").into_response()
}

async fn api_method_not_allowed() -> Response {
    crate::error::Error::MethodNotAllowed.into_response()
}

/// Base URL of a project's customer pages: `https://<checkout domain>` when the project has one,
/// else the site URL. The site URL path applies on both.
pub fn checkout_base(app: &App, checkout_domain: Option<&str>) -> String {
    match checkout_domain.filter(|d| !d.is_empty()) {
        Some(d) => format!("https://{d}{}", app.config.base_path()),
        None => app.config.site_url.clone(),
    }
}

/// Public link to a payment page.
pub fn payment_url(app: &App, checkout_domain: Option<&str>, token: &str) -> String {
    format!("{}/pay/{token}", checkout_base(app, checkout_domain))
}

/// Public link to an invoice page.
pub fn invoice_url(app: &App, checkout_domain: Option<&str>, token: &str) -> String {
    format!("{}/i/{token}", checkout_base(app, checkout_domain))
}

/// Host of a request, lowercase, without the port.
pub fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::HOST)?.to_str().ok()?;
    let host = match raw.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => raw.rsplit_once(':').map_or(raw, |(h, _)| h),
    };
    Some(host.trim_end_matches('.').to_ascii_lowercase()).filter(|h| !h.is_empty())
}

/// The project whose checkout domain served this request, if any.
pub async fn checkout_domain_project(app: &App, headers: &HeaderMap) -> Result<Option<i64>> {
    match request_host(headers) {
        Some(host) if host != app.config.site_host() => app.store.project_for_checkout_domain(&host).await,
        _ => Ok(None),
    }
}

/// Paths a checkout domain serves.
pub fn served_on_checkout_domain(path: &str) -> bool {
    ["/pay/", "/i/", "/x/", "/v1/", "/assets/", "/branding/"].iter().any(|p| path.starts_with(p))
        || matches!(path, "/v1" | "/healthz" | "/favicon.svg")
}

/// On a project's checkout domain only the checkout, the merchant API, assets and the health
/// check are served. Everything else, the dashboard included, is not found.
pub async fn checkout_domain_gate(app: &App, headers: &HeaderMap, path: &str) -> Option<Response> {
    if served_on_checkout_domain(path) {
        return None;
    }
    match checkout_domain_project(app, headers).await {
        Ok(None) => None,
        Ok(Some(_)) => Some((StatusCode::NOT_FOUND, "Not found.").into_response()),
        Err(e) => {
            e.log();
            Some((StatusCode::NOT_FOUND, "Not found.").into_response())
        }
    }
}

/// Serve until the process stops.
pub async fn serve(app: App) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(app.config.listen)
        .await
        .map_err(|e| crate::error::Error::Internal(format!("bind {}: {e}", app.config.listen)))?;
    tracing::info!(listen = %app.config.listen, site_url = %app.config.site_url, "turmite listening");
    let prefix = app.config.base_path().to_string();
    let service = router(app).map_request(move |req: Request| middleware::strip_prefix(&prefix, req));
    axum::serve(listener, axum::ServiceExt::into_make_service_with_connect_info::<SocketAddr>(service))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| crate::error::Error::Internal(e.to_string()))
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
