//! Domain allow and block lists.

use crate::error::{Error, Result};

/// Normalized domain lists applied to one call.
pub(super) struct Filter {
    pub(super) allowed: Vec<String>,
    pub(super) blocked: Vec<String>,
}

impl Filter {
    /// Normalizes a tool's configured lists. Fails naming the first entry that is not a
    /// hostname, or when every allowed domain is also blocked.
    pub(super) fn try_new(allowed: &[String], blocked: &[String]) -> Result<Self> {
        let parse = |domains: &[String], param: &str| {
            normalize_domains(domains).map_err(|bad| {
                Error::validation(
                    format!("{param} entry {bad:?} is not a hostname"),
                    Some(param.to_string()),
                )
            })
        };
        let allowed = parse(allowed, "allowed_domains")?;
        let blocked = parse(blocked, "blocked_domains")?;
        if !allowed.is_empty() && allowed.iter().all(|a| is_blocked(a, &blocked)) {
            return Err(Error::validation(
                "every allowed domain is also blocked",
                Some("allowed_domains".to_string()),
            ));
        }
        Ok(Self { allowed, blocked })
    }

    pub(super) fn permits(&self, url: &str) -> bool {
        let Some(host) = url::Url::parse(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .and_then(|u| {
                u.host_str()
                    .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
            })
        else {
            return false;
        };
        let allowed =
            self.allowed.is_empty() || self.allowed.iter().any(|a| domain_matches(&host, a));
        allowed && !is_blocked(&host, &self.blocked)
    }
}

/// Normalizes every entry, or returns the first one that is not a plain hostname.
pub(super) fn normalize_domains(domains: &[String]) -> std::result::Result<Vec<String>, String> {
    domains
        .iter()
        .map(|entry| normalize_domain(entry).ok_or_else(|| entry.clone()))
        .collect()
}

/// Lowercase ASCII (punycode) form of a hostname entry, without wildcard prefix or root dot.
fn normalize_domain(entry: &str) -> Option<String> {
    let lower = entry.trim().to_lowercase();
    let bare = lower
        .strip_prefix("*.")
        .or_else(|| lower.strip_prefix('.'))
        .unwrap_or(&lower);
    let bare = bare.strip_suffix('.').unwrap_or(bare);
    let forbidden = |c: char| c.is_whitespace() || "/:?#@*[]\\".contains(c);
    // An empty label ("..evil.com", "example..com") would never match a host, so a blocklist
    // entry like that would silently block nothing.
    if bare.split('.').any(str::is_empty) || bare.contains(forbidden) {
        return None;
    }
    match url::Host::parse(bare).ok()? {
        url::Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            (!domain.is_empty()).then(|| domain.to_string())
        }
        url::Host::Ipv4(ip) => Some(ip.to_string()),
        url::Host::Ipv6(_) => None,
    }
}

pub(super) fn is_blocked(host: &str, blocked: &[String]) -> bool {
    blocked.iter().any(|b| domain_matches(host, b))
}

/// Whether `host` is `domain` or one of its subdomains.
pub(super) fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}
