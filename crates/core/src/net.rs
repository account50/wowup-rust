// SPDX-License-Identifier: GPL-3.0-or-later
//! HTTP helper, the Rust side of wowup-lib's `NetworkInterface` plus a small response cache
//! (`caching-service.ts`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

pub const USER_AGENT: &str = concat!("WowUp.rs/", env!("CARGO_PKG_VERSION"), " (+https://github.com/WowUp/WowUp)");
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Net {
    client: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, String)>>,
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    pub fn new() -> Self {
        Self::with_options(false)
    }

    /// `force_ipv4` binds outgoing connections to IPv4 (upstream `--disable-ipv6`).
    pub fn with_options(force_ipv4: bool) -> Self {
        let mut builder = reqwest::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(60));
        if force_ipv4 {
            builder = builder.local_address(Some(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)));
        }
        let client = builder
            .build()
            .expect("http client");
        Self { client, cache: Mutex::new(HashMap::new()) }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    fn headers(headers: &[(&str, String)]) -> Result<HeaderMap> {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            map.insert(HeaderName::from_bytes(k.as_bytes())?, HeaderValue::from_str(v)?);
        }
        Ok(map)
    }

    pub async fn get_text(&self, url: &str, headers: &[(&str, String)]) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .headers(Self::headers(headers)?)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!(HttpError { status: status.as_u16(), url: url.to_string(), body: truncate(&text) });
        }
        Ok(text)
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str, headers: &[(&str, String)]) -> Result<T> {
        let text = self.get_text(url, headers).await?;
        serde_json::from_str(&text).with_context(|| format!("decoding {url}"))
    }

    /// GET with an in-memory cache for `ttl` (used for large lists such as the TukUI catalog).
    pub async fn get_json_cached<T: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, String)],
        ttl: Duration,
    ) -> Result<T> {
        if let Some((at, body)) = self.cache.lock().unwrap().get(url) {
            if at.elapsed() < ttl {
                return serde_json::from_str(body).with_context(|| format!("decoding {url}"));
            }
        }
        let text = self.get_text(url, headers).await?;
        let value = serde_json::from_str(&text).with_context(|| format!("decoding {url}"))?;
        self.cache.lock().unwrap().insert(url.to_string(), (Instant::now(), text));
        Ok(value)
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        url: &str,
        body: &B,
        headers: &[(&str, String)],
    ) -> Result<T> {
        let resp = self
            .client
            .post(url)
            .headers(Self::headers(headers)?)
            .json(body)
            .timeout(DEFAULT_TIMEOUT)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!(HttpError { status: status.as_u16(), url: url.to_string(), body: truncate(&text) });
        }
        serde_json::from_str(&text).with_context(|| format!("decoding {url}"))
    }

    pub fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(300).collect()
}

#[derive(Debug, thiserror::Error)]
#[error("HTTP {status} from {url}: {body}")]
pub struct HttpError {
    pub status: u16,
    pub url: String,
    pub body: String,
}

/// Percent-encode a query component.
pub fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
