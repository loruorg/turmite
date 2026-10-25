//! Request plumbing shared by every route.

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{app::App, error::Error};

/// Strip the site URL path (`/pay` in `https://example.com/pay`) from the request path.
/// Requests without the prefix are served at the root.
pub fn strip_prefix(prefix: &str, mut req: Request) -> Request {
    if prefix.is_empty() {
        return req;
    }
    let path = req.uri().path();
    let rest = if path == prefix { Some("/") } else { path.strip_prefix(prefix).filter(|r| r.starts_with('/')) };
    if let Some(rest) = rest {
        let pq = match req.uri().query() {
            Some(q) => format!("{rest}?{q}"),
            None => rest.to_string(),
        };
        let mut parts = req.uri().clone().into_parts();
        if let Ok(p) = pq.parse() {
            parts.path_and_query = Some(p);
            if let Ok(uri) = axum::http::Uri::from_parts(parts) {
                *req.uri_mut() = uri;
            }
        }
    }
    req
}

/// The client address: the socket peer, or the rightmost untrusted `X-Forwarded-For` hop
/// when the peer is a trusted proxy.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp(pub IpAddr);

impl ClientIp {
    pub fn key(&self) -> String {
        self.0.to_string()
    }
}

pub fn client_ip(app: &App, peer: Option<IpAddr>, headers: &HeaderMap) -> IpAddr {
    let unspecified = IpAddr::from([0u8, 0, 0, 0]);
    let Some(peer) = peer else { return unspecified };
    let trusted = |ip: &IpAddr| app.config.trusted_proxies.iter().any(|n| n.contains(ip));
    if !trusted(&peer) {
        return peer;
    }
    let hops: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    hops.into_iter().rev().find(|ip| !trusted(ip)).unwrap_or(peer)
}

impl FromRequestParts<App> for ClientIp {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
        Ok(ClientIp(client_ip(app, peer, &parts.headers)))
    }
}

/// Which part of the server a path belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Area {
    Api,
    Checkout,
    CheckoutFrame,
    Dashboard,
    Assets,
    Other,
}

pub fn area(path: &str) -> Area {
    if path.starts_with("/v1/") || path == "/v1" {
        Area::Api
    } else if (path.starts_with("/pay/") || path.starts_with("/x/")) && path.ends_with("/status") {
        Area::CheckoutFrame
    } else if path.starts_with("/pay/") || path.starts_with("/i/") || path.starts_with("/x/") {
        Area::Checkout
    } else if path.starts_with("/dashboard") || path == "/" {
        Area::Dashboard
    } else if path.starts_with("/assets/") || path.starts_with("/branding/") || path == "/favicon.svg" {
        Area::Assets
    } else {
        Area::Other
    }
}

const CSP_CHECKOUT: &str = "default-src 'none'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; \
    font-src 'self'; frame-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";
const CSP_FRAME: &str = "default-src 'none'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data:; \
    font-src 'self'; form-action 'self'; frame-ancestors 'self'; base-uri 'none'";
const CSP_DASHBOARD: &str = "default-src 'self'; script-src 'self'; style-src 'self'; style-src-attr 'unsafe-inline'; \
    img-src 'self' data:; font-src 'self'; connect-src 'self'; form-action 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'";
const CSP_API: &str = "default-src 'none'; frame-ancestors 'none'";

/// Security headers, rate limits and the cross-site check for dashboard forms.
pub async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let area = area(&path);
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    let ip = client_ip(&app, peer, req.headers());

    let limiter = match area {
        Area::Api => Some(&app.limits.api),
        Area::Checkout | Area::CheckoutFrame => Some(&app.limits.checkout),
        _ => None,
    };
    if let Some(l) = limiter
        && !l.hit(&ip.to_string())
    {
        return Error::TooManyRequests.into_response();
    }

    if area == Area::Dashboard && !matches!(*req.method(), Method::GET | Method::HEAD) && !same_origin(&app, req.headers()) {
        return (StatusCode::FORBIDDEN, "Cross-site request refused.").into_response();
    }

    if let Some(resp) = crate::http::checkout_domain_gate(&app, req.headers(), &path).await {
        return resp;
    }

    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    let (csp, frame) = match area {
        Area::Checkout => (CSP_CHECKOUT, "DENY"),
        Area::CheckoutFrame => (CSP_FRAME, "SAMEORIGIN"),
        Area::Dashboard => (CSP_DASHBOARD, "DENY"),
        Area::Api => (CSP_API, "DENY"),
        Area::Assets | Area::Other => ("", "DENY"),
    };
    if !csp.is_empty() && !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(csp));
    }
    if !h.contains_key(header::X_FRAME_OPTIONS) {
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static(frame));
    }
    if app.config.secure_cookies() {
        h.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=31536000"));
    }
    if matches!(area, Area::Dashboard | Area::Checkout | Area::CheckoutFrame) && !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

/// True when the request's `Origin` (or `Referer`) is the site URL's origin.
pub fn same_origin(app: &App, headers: &HeaderMap) -> bool {
    let site = match url::Url::parse(&app.config.site_url) {
        Ok(u) => u.origin(),
        Err(_) => return false,
    };
    let from = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| url::Url::parse(v).ok());
    match from {
        Some(u) => u.origin() == site || is_local_dev(app, &u),
        None => false,
    }
}

/// On testnet, a browser on localhost may use another port than the site URL (dev servers).
fn is_local_dev(app: &App, u: &url::Url) -> bool {
    !app.config.mode.is_mainnet()
        && matches!(u.host_str(), Some("localhost") | Some("127.0.0.1"))
        && matches!(
            url::Url::parse(&app.config.site_url).ok().and_then(|s| s.host_str().map(str::to_string)).as_deref(),
            Some("localhost") | Some("127.0.0.1")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_prefix() {
        let req = Request::builder().uri("/pay/v1/status?x=1").body(axum::body::Body::empty()).unwrap();
        assert_eq!(strip_prefix("/pay", req).uri().to_string(), "/v1/status?x=1");
        let req = Request::builder().uri("/payx").body(axum::body::Body::empty()).unwrap();
        assert_eq!(strip_prefix("/pay", req).uri().to_string(), "/payx");
        let req = Request::builder().uri("/pay").body(axum::body::Body::empty()).unwrap();
        assert_eq!(strip_prefix("/pay", req).uri().to_string(), "/");
    }

    #[test]
    fn areas() {
        assert_eq!(area("/v1/payment"), Area::Api);
        assert_eq!(area("/pay/abc/status"), Area::CheckoutFrame);
        assert_eq!(area("/pay/abc"), Area::Checkout);
        assert_eq!(area("/x/abc/status"), Area::CheckoutFrame);
        assert_eq!(area("/x/abc"), Area::Checkout);
        assert_eq!(area("/dashboard/login"), Area::Dashboard);
    }
}
