//! Outbound HTTP to merchant URLs. On mainnet only public `https` targets: private and
//! reserved addresses are refused when the name resolves, so DNS tricks do not help.
//! Redirects are never followed.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::OnceLock,
    time::Duration,
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::{
    app::App,
    error::{Error, Result},
};

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let seg = v6.segments();
            // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 address.
            if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
                let o = v6.octets();
                return is_public_v4(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)
                || v6 == Ipv6Addr::LOCALHOST)
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || o[0] >= 240)
}

struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            let public: Vec<SocketAddr> = addrs.into_iter().filter(|a| is_public_ip(a.ip())).collect();
            if public.is_empty() {
                return Err("refused: the name resolves only to private or reserved addresses".into());
            }
            Ok(Box::new(public.into_iter()) as Addrs)
        })
    }
}

/// HTTP client for webhooks: no redirects, 30-second timeout, public targets only on mainnet.
pub fn webhook_client(app: &App) -> Result<&'static reqwest::Client> {
    static PUBLIC: OnceLock<reqwest::Client> = OnceLock::new();
    static ANY: OnceLock<reqwest::Client> = OnceLock::new();
    let base = || {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
    };
    if app.config.mode.is_mainnet() {
        Ok(PUBLIC.get_or_init(|| base().dns_resolver(std::sync::Arc::new(PublicOnly)).build().expect("http client")))
    } else {
        Ok(ANY.get_or_init(|| base().build().expect("http client")))
    }
}

/// Reject targets that are never allowed. Host names are checked again at connect time.
pub async fn check_target(app: &App, raw: &str) -> Result<()> {
    let url = url::Url::parse(raw).map_err(|_| Error::BadRequest("invalid URL".into()))?;
    if !app.config.mode.is_mainnet() {
        return Ok(());
    }
    if url.scheme() != "https" {
        return Err(Error::BadRequest("only https:// URLs are allowed on mainnet".into()));
    }
    let literal = match url.host() {
        Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
        _ => None,
    };
    if let Some(ip) = literal
        && !is_public_ip(ip)
    {
        return Err(Error::BadRequest("private and reserved addresses are not allowed".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!is_public_ip(bad.parse().unwrap()), "{bad}");
        }
        for good in ["1.1.1.1", "8.8.8.8", "2606:4700::1111"] {
            assert!(is_public_ip(good.parse().unwrap()), "{good}");
        }
    }
}
