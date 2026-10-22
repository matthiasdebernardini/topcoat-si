//! HTTP plumbing shared by the Jev and Firecrawl clients: the bearer header, the client builder,
//! the base URL switch and reading one response.

use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client as ReqwestClient, ClientBuilder, RequestBuilder, redirect};

use crate::base_url;
use crate::error::{Error, RequestPhase, Result, Service, map_reqwest_error};
use crate::types::retry_after_from_headers;

/// Builds the sensitive `Authorization: Bearer` value for `api_key`.
///
/// Fails when the key is empty after trimming or is not printable ASCII without whitespace.
pub(crate) fn bearer(service: Service, api_key: &str) -> Result<HeaderValue> {
    let invalid = |message: String| Error::validation(message, Some("api_key".to_string()));
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(invalid(format!("{service} API key is empty")));
    }
    // Printable ASCII is always a valid header value, so `from_str` cannot fail past this check.
    let not_printable = || {
        invalid(format!(
            "{service} api_key must be printable ASCII without whitespace"
        ))
    };
    if !api_key.chars().all(|c| c.is_ascii_graphic()) {
        return Err(not_printable());
    }
    let mut authorization =
        HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|_| not_printable())?;
    authorization.set_sensitive(true);
    Ok(authorization)
}

/// Builds the HTTP client for `base_url` from `builder`, without redirects. The base URL decides
/// whether proxies are used (see [`base_url::with_proxy_rule`]).
pub(crate) fn client(base_url: &str, builder: ClientBuilder) -> Result<ReqwestClient> {
    base_url::with_proxy_rule(builder.redirect(redirect::Policy::none()), base_url)
        .build()
        .map_err(|e| Error::validation("Failed to build HTTP client", None).with_source(e))
}

/// Checks a new base URL with [`base_url::checked_base_url`] and builds the client for it, since
/// a loopback base URL bypasses proxies. `setting` names where the URL came from in the
/// validation error.
pub(crate) fn rebase(
    service: Service,
    setting: &str,
    base_url: &str,
    builder: ClientBuilder,
) -> Result<(String, ReqwestClient)> {
    let base_url = base_url::checked_base_url(&service.to_string(), setting, base_url)?;
    let http = client(&base_url, builder)?;
    Ok((base_url, http))
}

/// One response read to the end, whatever its status.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) request_id: Option<String>,
    pub(crate) body: Bytes,
}

impl Reply {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends `request` and reads the whole body. `request_id` picks the service's request id out of
/// the headers. An error status is returned as a [`Reply`] for the caller to parse.
pub(crate) async fn send(
    request: RequestBuilder,
    service: Service,
    request_id: fn(&HeaderMap) -> Option<String>,
) -> Result<Reply> {
    let response = request
        .send()
        .await
        .map_err(|e| map_reqwest_error(e, RequestPhase::Headers))?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let request_id = request_id(&headers);
    let success = (200..300).contains(&status);
    // Headers arrived, so a body failure keeps the request id. An error status whose body
    // failed is still that status, so it stays retryable as one.
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(e) if success => {
            return Err(map_reqwest_error(e, RequestPhase::Body).with_request_id(request_id));
        }
        Err(e) => {
            let error = Error::http_status(
                service,
                status,
                None,
                None,
                None,
                request_id,
                retry_after_from_headers(&headers),
            );
            return Err(error.with_source(e));
        }
    };
    Ok(Reply {
        status,
        headers,
        request_id,
        body,
    })
}

pub(crate) fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Keeps the first `max_chars` characters of `text`, appending `suffix` when any were cut.
pub(crate) fn truncate_chars(text: &str, max_chars: usize, suffix: &str) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}{suffix}", &text[..end]),
        None => text.to_string(),
    }
}
