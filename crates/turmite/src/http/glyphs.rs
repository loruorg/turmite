//! Monochrome coin and network marks as inline SVG. They take the text colour.

const OPEN: &str = r#"<svg class="glyph" viewBox="0 0 24 24" aria-hidden="true" focusable="false">"#;

fn mark(symbol: &str) -> Option<&'static str> {
    Some(match symbol {
        "BTC" => {
            r#"<g transform="rotate(12 12 12)" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M8.5 6.5h5a2.75 2.75 0 0 1 0 5.5h-5zM8.5 12h6a2.75 2.75 0 0 1 0 5.5h-6zM10.5 4.5v2M13 4.5v2M10.5 17.5v2M13 17.5v2"/></g>"#
        }
        "ETH" => {
            r#"<g fill="currentColor"><path d="M12 2.5 6.2 12.2 12 15.6z"/><path opacity=".62" d="M12 2.5v13.1l5.8-3.4z"/><path opacity=".82" d="M12 16.9 6.2 13.4 12 21.5z"/><path opacity=".5" d="M12 16.9v4.6l5.8-8.1z"/></g>"#
        }
        "XMR" => {
            r#"<path fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" d="M3.5 15.5h3.25V8.75L12 14l5.25-5.25v6.75h3.25"/>"#
        }
        "USDT" => {
            r#"<g fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M6.5 6.5h11M12 6.5v11.5"/><ellipse cx="12" cy="11.2" rx="6.2" ry="2"/></g>"#
        }
        "USDC" => {
            r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M14.6 9.4c-.4-1-1.4-1.6-2.6-1.6-1.5 0-2.6.8-2.6 2s1 1.6 2.6 2c1.7.4 2.8 1 2.8 2.3s-1.2 2.2-2.8 2.2c-1.3 0-2.4-.6-2.9-1.6M12 6.1v1.7M12 16.3V18"/><path d="M8.6 4.8a8 8 0 0 0 0 14.4M15.4 4.8a8 8 0 0 1 0 14.4"/></g>"#
        }
        "SOL" => {
            r#"<g fill="currentColor"><path d="M7.2 6h12.3l-2.7 2.8H4.5z"/><path d="M4.5 10.6h12.3l2.7 2.8H7.2z"/><path d="M7.2 15.2h12.3l-2.7 2.8H4.5z"/></g>"#
        }
        "TRX" => {
            r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 4.8 19.6 8.2 11.2 20z"/><path d="M4 4.8 12.6 8.6 11.2 20M12.6 8.6l7-0.4"/></g>"#
        }
        "TON" => {
            r#"<path fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" d="M6.3 6.3h11.4c.9 0 1.4.9 1 1.7L12 19.6 5.3 8c-.4-.8.1-1.7 1-1.7zM12 6.3v13.3"/>"#
        }
        "BNB" => {
            r#"<g fill="currentColor"><path d="M12 9.3 14.7 12 12 14.7 9.3 12z"/><path d="M12 3.6l4.6 4.6-1.6 1.6-3-3-3 3-1.6-1.6z"/><path d="M12 20.4l-4.6-4.6 1.6-1.6 3 3 3-3 1.6 1.6z"/><path d="M3.6 12l1.6-1.6 1.6 1.6-1.6 1.6zM17.2 12l1.6-1.6 1.6 1.6-1.6 1.6z"/></g>"#
        }
        "POL" => {
            r#"<path fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" d="M15.6 9.6V8.2l-3.3-1.9L9 8.2v7.6l-3.3 1.9-3.2-1.9v-3.6l3.2-1.9 1.6.9M8.4 14.4v1.4l3.3 1.9 3.3-1.9V8.2l3.3-1.9 3.2 1.9v3.6l-3.2 1.9-1.6-.9"/>"#
        }
        "BASE" => r#"<path fill="currentColor" d="M12 3.5a8.5 8.5 0 1 1-8.45 9.4h11.2v-1.8H3.55A8.5 8.5 0 0 1 12 3.5z"/>"#,
        _ => return None,
    })
}

/// The mark of an asset (`BTC`, `USDT`, ...), or `None` for coins without one.
pub fn coin(symbol: &str) -> Option<String> {
    mark(&symbol.to_ascii_uppercase()).map(|m| format!("{OPEN}{m}</svg>"))
}

/// The mark of a network: its native coin, or its own mark.
pub fn network(code: &str, native_symbol: &str) -> Option<String> {
    match code {
        "BASE" => coin("BASE"),
        _ => coin(native_symbol),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn marks() {
        for s in ["btc", "ETH", "XMR", "USDT", "USDC", "SOL", "TRX", "TON", "BNB", "POL"] {
            let svg = super::coin(s).unwrap();
            assert!(svg.starts_with("<svg") && svg.ends_with("</svg>") && !svg.contains("<script"));
        }
        assert!(super::coin("LTC").is_none());
        assert!(super::network("BASE", "ETH").unwrap().contains("M12 3.5"));
    }
}
