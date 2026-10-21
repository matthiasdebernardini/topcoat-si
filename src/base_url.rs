//! The base URL rule shared by every client that sends a key: https to any host, plain http
//! only for loopback, and no proxy for loopback.

use reqwest::ClientBuilder;
use url::{Host, Url};

use crate::error::{Error, Result};

/// Checks a base URL that a key will be sent to and returns it parsed.
///
/// It must use https, or plain http only for a loopback host (`localhost`, `127.x`, `[::1]`),
/// so the key never travels in clear over a network. Credentials, a query or a fragment in the
/// URL are rejected. `service` names the client and `setting` the configuration the URL came
/// from, which the validation error carries as its parameter.
pub(crate) fn checked_url(service: &str, setting: &str, base_url: &str) -> Result<Url> {
    let invalid = |message: String| Error::validation(message, Some(setting.to_string()));
    let parsed = Url::parse(base_url.trim())
        .map_err(|e| invalid(format!("{service} base URL is not a valid URL")).with_source(e))?;
    match parsed.scheme() {
        "https" if parsed.host().is_some() => {}
        "http" if is_loopback(&parsed) => {}
        _ => {
            return Err(invalid(format!(
                "{service} base URL must use https, or http only for a loopback host"
            )));
        }
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid(format!(
            "{service} base URL must not carry credentials, a query or a fragment"
        )));
    }
    Ok(parsed)
}

/// Checks a base URL with [`checked_url`] and returns it as given, trimmed and without trailing
/// slashes.
pub(crate) fn checked_base_url(service: &str, setting: &str, base_url: &str) -> Result<String> {
    checked_url(service, setting, base_url)?;
    Ok(base_url.trim().trim_end_matches('/').to_string())
}

/// Turns off every proxy, configured or from the system, when `base_url` is a loopback host.
///
/// A loopback base URL may use plain http, and a proxy would then see the key in clear. Any
/// other base URL keeps the builder's proxy settings, so system proxies still apply.
pub(crate) fn with_proxy_rule(builder: ClientBuilder, base_url: &str) -> ClientBuilder {
    if Url::parse(base_url).is_ok_and(|url| is_loopback(&url)) {
        builder.no_proxy()
    } else {
        builder
    }
}

/// Whether the URL's host is `localhost` or a loopback IP address.
fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::{Client, Proxy};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{checked_base_url, with_proxy_rule};

    #[test]
    fn base_url_rule_rejects_attacks_and_accepts_https_and_loopback() {
        let cases = [
            ("http://localhost.evil.com", false),
            ("http://localhost@evil.com", false),
            ("https://evil.com@localhost", false),
            ("http://127.0.0.1.evil.com", false),
            ("http://0.0.0.0", false),
            ("http://[::ffff:127.0.0.1]", false),
            ("http://localhost.", false),
            ("ftp://localhost", false),
            ("https://api.example.com?key=1", false),
            ("https://api.example.com#top", false),
            ("http://localhost:8080/?debug", false),
            ("http://localhost:8080/#top", false),
            ("https://any.host", true),
            ("http://localhost:8080", true),
            ("HTTP://LOCALHOST", true),
            ("http://127.0.0.1", true),
            ("http://127.1", true),
            ("http://[::1]:9000", true),
        ];
        for (base_url, accepted) in cases {
            let result = checked_base_url("Test", "base_url", base_url);
            assert_eq!(result.is_ok(), accepted, "{base_url}: {result:?}");
        }
    }

    #[test]
    fn unparseable_base_url_keeps_the_parse_error() {
        let error = checked_base_url("Test", "base_url", "not a url").unwrap_err();
        assert_eq!(error.to_string(), "Test base URL is not a valid URL");
        let source = std::error::Error::source(&error).expect("the url::ParseError is the source");
        assert!(source.is::<url::ParseError>(), "{source:?}");
    }

    /// Accepts one connection, then answers its request with `name` as the body.
    async fn answer_once(listener: TcpListener, name: &'static str) {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(n) => request.extend_from_slice(&buffer[..n]),
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{name}",
            name.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }

    /// Sends one request to a local server through a client whose builder has a local proxy and
    /// the proxy rule for `base_url` applied, and returns which of the two answered. The client
    /// times out after 5 seconds, so a regression fails instead of hanging.
    async fn answered_by(base_url: &str) -> String {
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_url = format!("http://{}/", target.local_addr().unwrap());
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        tokio::spawn(answer_once(target, "target"));
        tokio::spawn(answer_once(proxy, "proxy"));
        let builder = Client::builder()
            .timeout(Duration::from_secs(5))
            .proxy(Proxy::all(proxy_url).unwrap());
        let client = with_proxy_rule(builder, base_url).build().unwrap();
        client
            .get(target_url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn loopback_base_url_bypasses_proxies() {
        for base_url in ["http://localhost:8080", "http://127.0.0.1", "http://[::1]"] {
            assert_eq!(answered_by(base_url).await, "target", "{base_url}");
        }
    }

    #[tokio::test]
    async fn other_base_url_keeps_proxies() {
        assert_eq!(answered_by("https://api.anthropic.com").await, "proxy");
    }
}
