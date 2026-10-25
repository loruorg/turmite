//! Project branding on the checkout: logo processing, accent colours, return URL rules.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, imageops::FilterType};

use crate::{
    app::App,
    db::BrandingRow,
    error::{Error, Result},
};

/// Largest logo file accepted.
pub const MAX_LOGO_BYTES: usize = 2 * 1024 * 1024;
/// Largest side of a stored logo.
pub const LOGO_SIDE: u32 = 512;
const MAX_SOURCE_SIDE: u32 = 8192;
const MAX_SOURCE_PIXELS: u64 = 40_000_000;

pub const THEMES: [&str; 3] = ["auto", "light", "dark"];
pub const MAX_MESSAGE: usize = 280;

/// Check, scale to fit 512x512 and re-encode a PNG, JPEG or WebP logo as PNG.
/// Returns the PNG and a short hash for its URL.
pub fn process_logo(bytes: &[u8]) -> Result<(Vec<u8>, String)> {
    if bytes.len() > MAX_LOGO_BYTES {
        return Err(Error::BadRequest("The logo is larger than 2 MB.".into()));
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    if head.trim_start().starts_with('<') || head.contains("<svg") {
        return Err(Error::BadRequest("SVG logos are not accepted. Use a PNG, JPEG or WebP image.".into()));
    }
    let format = image::guess_format(bytes).map_err(|_| Error::BadRequest("Use a PNG, JPEG or WebP image.".into()))?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) {
        return Err(Error::BadRequest("Use a PNG, JPEG or WebP image.".into()));
    }
    // Read the size from the header before decoding any pixels.
    let (w, h) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| Error::BadRequest("The image file is damaged.".into()))?;
    if w == 0 || h == 0 || w > MAX_SOURCE_SIDE || h > MAX_SOURCE_SIDE || u64::from(w) * u64::from(h) > MAX_SOURCE_PIXELS {
        return Err(Error::BadRequest(format!("The image is {w}x{h} pixels. Use at most {MAX_SOURCE_SIDE} pixels on each side.")));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|_| Error::BadRequest("The image file is damaged.".into()))?;
    let img = if w > LOGO_SIDE || h > LOGO_SIDE { img.resize(LOGO_SIDE, LOGO_SIDE, FilterType::Lanczos3) } else { img };
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| Error::Internal(format!("logo encode: {e}")))?;
    let hash = crate::crypto::sha256_hex(&out)[..16].to_string();
    Ok((out, hash))
}

/// Download a logo from an https URL (public hosts only on mainnet), at most 2 MB.
pub async fn fetch_logo(app: &App, url: &str) -> Result<Vec<u8>> {
    crate::netguard::check_target(app, url).await?;
    let client = crate::netguard::webhook_client(app)?;
    let mut resp = client.get(url).send().await.map_err(|_| Error::BadRequest("The logo URL cannot be reached.".into()))?;
    if !resp.status().is_success() {
        return Err(Error::BadRequest(format!("The logo URL returned HTTP {}.", resp.status().as_u16())));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| Error::BadRequest("The logo download failed.".into()))? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_LOGO_BYTES {
            return Err(Error::BadRequest("The logo is larger than 2 MB.".into()));
        }
    }
    Ok(body)
}

/// `#abc` or `#aabbcc` as lowercase `#aabbcc`.
pub fn parse_hex_color(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#').to_ascii_lowercase();
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => Some(format!("#{h}")),
        3 => Some(format!("#{}", h.chars().flat_map(|c| [c, c]).collect::<String>())),
        _ => None,
    }
}

fn rgb(hex: &str) -> (f64, f64, f64) {
    let v = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f64 / 255.0;
    (v(1), v(3), v(5))
}

fn to_hex((r, g, b): (f64, f64, f64)) -> String {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

/// WCAG relative luminance.
pub fn luminance(hex: &str) -> f64 {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let (r, g, b) = rgb(hex);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn mix(hex: &str, toward: (f64, f64, f64), t: f64) -> String {
    let (r, g, b) = rgb(hex);
    to_hex((r + (toward.0 - r) * t, g + (toward.1 - g) * t, b + (toward.2 - b) * t))
}

/// Text colour on an accent background.
fn on(hex: &str) -> &'static str {
    if luminance(hex) > 0.36 { "#0a0a0a" } else { "#ffffff" }
}

/// CSS custom properties for an accent colour: one variant for light and one for dark
/// surfaces, each readable on its background.
pub fn accent_style(accent: Option<&str>) -> String {
    let Some(a) = accent.and_then(parse_hex_color) else { return String::new() };
    let mut light = a.clone();
    let mut i = 0;
    while luminance(&light) > 0.45 && i < 20 {
        light = mix(&a, (0.0, 0.0, 0.0), 0.05 * f64::from(i + 1));
        i += 1;
    }
    let mut dark = a.clone();
    let mut i = 0;
    while luminance(&dark) < 0.2 && i < 20 {
        dark = mix(&a, (1.0, 1.0, 1.0), 0.05 * f64::from(i + 1));
        i += 1;
    }
    format!("--accent-l:{light};--accent-l-fg:{};--accent-d:{dark};--accent-d-fg:{}", on(&light), on(&dark))
}

/// What every customer page needs from the branding.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub store: String,
    /// `light`, `dark` or `auto`.
    pub theme: String,
    /// Inline custom properties for the `<html>` element.
    pub style: String,
    /// Path of the logo, or empty.
    pub logo: String,
    pub initial: String,
    pub message: String,
    pub support_email: String,
    pub support_url: String,
}

impl Look {
    pub fn new(app: &App, b: &BrandingRow) -> Self {
        let logo = match &b.logo_hash {
            Some(h) => app.path(&format!("/branding/{}/logo-{h}.png", b.project_id)),
            None => String::new(),
        };
        Look {
            store: b.name.clone(),
            theme: if THEMES.contains(&b.theme.as_str()) { b.theme.clone() } else { "auto".into() },
            style: accent_style(b.accent_color.as_deref()),
            logo,
            initial: b.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default(),
            message: b.message.clone().unwrap_or_default(),
            support_email: b.support_email.clone().unwrap_or_default(),
            support_url: b.support_url.clone().unwrap_or_default(),
        }
    }

    pub async fn load(app: &App, project_id: i64) -> Result<Self> {
        Ok(Self::new(app, &app.store.branding(project_id).await?))
    }

    pub fn has_support(&self) -> bool {
        !self.support_email.is_empty() || !self.support_url.is_empty()
    }
}

/// Parse a list of host names (comma, space or line separated).
pub fn parse_hosts(input: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for raw in input.split([',', ' ', '\n', '\r', '\t']).map(str::trim).filter(|s| !s.is_empty()) {
        let h = raw.trim_end_matches('.').to_ascii_lowercase();
        if !valid_host(&h) {
            return Err(Error::BadRequest(format!("`{raw}` is not a host name.")));
        }
        if !out.contains(&h) {
            out.push(h);
        }
    }
    if out.len() > 50 {
        return Err(Error::BadRequest("Use at most 50 return hosts.".into()));
    }
    Ok(out)
}

/// A DNS host name: letters, digits, dashes and dots, with a dot.
pub fn valid_host(h: &str) -> bool {
    h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && !h.rsplit('.').next().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
}

/// True when `url`'s host is in `hosts`, or `hosts` is empty.
pub fn host_allowed(hosts: &[String], url: &str) -> bool {
    if hosts.is_empty() {
        return true;
    }
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_end_matches('.').to_ascii_lowercase()))
        .is_some_and(|h| hosts.contains(&h))
}

/// Refuse redirect URLs whose host is not in the project's allowed return hosts.
pub async fn check_return_hosts(app: &App, project_id: i64, urls: &[(&str, &Option<String>)]) -> Result<()> {
    let hosts = app.store.branding(project_id).await?.return_hosts;
    for (name, url) in urls {
        if let Some(u) = url.as_deref().filter(|s| !s.is_empty())
            && !host_allowed(&hosts, u)
        {
            return Err(Error::BadRequest(format!("The host of `{name}` is not an allowed return host of this project.")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_hex_color("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(parse_hex_color("12ab34").as_deref(), Some("#12ab34"));
        assert_eq!(parse_hex_color("#12ab3"), None);
        assert_eq!(parse_hex_color("red"), None);
        let s = accent_style(Some("#000080"));
        assert!(s.starts_with("--accent-l:#000080;--accent-l-fg:#ffffff;--accent-d:#"), "{s}");
        assert!(accent_style(Some("#ffff00")).contains("--accent-l-fg:#0a0a0a") || luminance("#ffff00") > 0.45);
    }

    #[test]
    fn hosts() {
        assert_eq!(
            parse_hosts("Shop.example.com, pay.example.com\nshop.example.com").unwrap(),
            vec!["shop.example.com", "pay.example.com"]
        );
        assert!(parse_hosts("not a host!").is_err());
        assert!(parse_hosts("localhost").is_err());
        assert!(parse_hosts("1.2.3.4").is_err());
        let hosts = vec!["shop.example.com".to_string()];
        assert!(host_allowed(&hosts, "https://shop.example.com/thanks"));
        assert!(!host_allowed(&hosts, "https://evil.example.com/"));
        assert!(host_allowed(&[], "https://anything.example/"));
    }
}
