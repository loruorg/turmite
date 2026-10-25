//! Static files compiled into the binary.

use std::sync::LazyLock;

use axum::{
    Router,
    extract::Path,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::app::App;

struct Asset {
    name: &'static str,
    mime: &'static str,
    body: &'static [u8],
}

const ASSETS: &[Asset] = &[
    Asset { name: "dashboard.css", mime: "text/css; charset=utf-8", body: include_bytes!("../../assets/dashboard.css") },
    Asset { name: "dashboard.js", mime: "text/javascript; charset=utf-8", body: include_bytes!("../../assets/dashboard.js") },
    Asset { name: "checkout.css", mime: "text/css; charset=utf-8", body: include_bytes!("../../assets/checkout.css") },
    Asset { name: "fonts/inter.woff2", mime: "font/woff2", body: include_bytes!("../../assets/fonts/inter.woff2") },
    Asset { name: "fonts/jetbrains-mono.woff2", mime: "font/woff2", body: include_bytes!("../../assets/fonts/jetbrains-mono.woff2") },
    Asset { name: "favicon.svg", mime: "image/svg+xml", body: include_bytes!("../../assets/favicon.svg") },
];

static ETAGS: LazyLock<Vec<String>> =
    LazyLock::new(|| ASSETS.iter().map(|a| format!("\"{}\"", &crate::crypto::sha256_hex(a.body)[..16])).collect());

pub fn routes() -> Router<App> {
    Router::new()
        .route("/assets/{*path}", get(asset))
        .route("/favicon.svg", get(|headers: HeaderMap| async move { serve("favicon.svg", &headers) }))
}

async fn asset(Path(path): Path<String>, headers: HeaderMap) -> Response {
    serve(&path, &headers)
}

fn serve(path: &str, headers: &HeaderMap) -> Response {
    let Some(i) = ASSETS.iter().position(|a| a.name == path) else {
        return (StatusCode::NOT_FOUND, "Not found.").into_response();
    };
    let etag = &ETAGS[i];
    if headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let a = &ASSETS[i];
    let mut r = a.body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(a.mime));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    if let Ok(v) = HeaderValue::from_str(etag) {
        h.insert(header::ETAG, v);
    }
    r
}
