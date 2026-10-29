// SPDX-License-Identifier: GPL-3.0-or-later
//! GitHub releases provider, ported from `github-addon-provider.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Deserialize;

use super::{AddonProvider, Capabilities, DownloadAuth, GetAllResult, ProviderError};
use crate::models::*;
use crate::net::{HttpError, Net};

const API_URL: &str = "https://api.github.com/repos";

#[derive(Debug, Clone, Deserialize)]
struct Owner {
    login: String,
    #[serde(default)]
    avatar_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Repository {
    name: String,
    html_url: String,
    #[serde(default)]
    description: Option<String>,
    owner: Owner,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    url: String,
    name: String,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Release {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct MetaFlavor {
    flavor: String,
}

#[derive(Debug, Clone, Deserialize)]
struct MetaRelease {
    filename: String,
    #[serde(default)]
    nolib: bool,
    #[serde(default)]
    metadata: Vec<MetaFlavor>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseMeta {
    releases: Vec<MetaRelease>,
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error("GitHub rate limit reached ({used}/{max}); add a personal access token in settings")]
    RateLimit { max: String, used: String },
    #[error("No release asset found for this client: {0}")]
    AssetMissing(String),
}

pub struct GitHubProvider {
    net: Arc<Net>,
    token: Option<String>,
}

fn re(s: &str) -> Regex {
    Regex::new(s).unwrap()
}

impl GitHubProvider {
    pub fn new(net: Arc<Net>, token: Option<String>) -> Self {
        Self { net, token }
    }

    fn headers(&self, binary: bool) -> Vec<(&'static str, String)> {
        let mut h = vec![("Accept", if binary { "application/octet-stream" } else { "application/vnd.github+json" }.to_string())];
        if let Some(t) = &self.token {
            h.push(("Authorization", format!("Bearer {t}")));
        }
        h
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str, binary: bool) -> anyhow::Result<T> {
        let resp = self.net.client().get(url).headers(to_map(&self.headers(binary))).send().await?;
        let status = resp.status().as_u16();
        if status == 403 && resp.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()) == Some("0") {
            let h = |k: &str| resp.headers().get(k).and_then(|v| v.to_str().ok()).unwrap_or("?").to_string();
            return Err(GitHubError::RateLimit { max: h("x-ratelimit-limit"), used: h("x-ratelimit-used") }.into());
        }
        let text = resp.text().await?;
        if status == 404 {
            return Err(ProviderNotFound.into());
        }
        if !(200..300).contains(&status) {
            return Err(HttpError { status, url: url.into(), body: text.chars().take(200).collect() }.into());
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// `/?(.*?)/(.*?)(/.*|\.git.*)?$` -> (owner, repo)
    pub fn parse_repo_path(path: &str) -> Option<(String, String)> {
        let c = re(r"^/?([^/]*?)/([^/]*?)(/.*|\.git.*)?$").captures(path)?;
        let (o, r) = (c[1].to_string(), c[2].to_string());
        (!o.is_empty() && !r.is_empty()).then_some((o, r))
    }

    fn meta_flavor(client: WowClientType) -> Option<&'static str> {
        use WowClientType::*;
        Some(match client {
            Classic | ClassicPtr => "mists",
            Anniversary => "bcc",
            ClassicEra | ClassicEraPtr => "classic",
            Beta | Retail | RetailPtr | RetailXPtr => "mainline",
            Forever | ClassicBeta => "forever",
            None => return Option::None,
        })
    }

    fn not_nolib(a: &Asset) -> bool {
        !a.name.to_lowercase().contains("-nolib")
    }

    fn valid_content_type(a: &Asset) -> bool {
        matches!(a.content_type.as_str(), "application/zip" | "application/x-zip-compressed")
            || (a.content_type == "application/octet-stream" && a.browser_download_url.ends_with(".zip"))
    }

    pub fn is_valid_client_asset_name(name: &str, client: WowClientType) -> bool {
        use WowClientType::*;
        let classic = re(r"(?i)[-_](classic|vanilla)\.zip$").is_match(name);
        let bc = re(r"(?i)[-_](bc|bcc|tbc)\.zip$").is_match(name);
        let wotlk = re(r"(?i)[-_](wrath|wotlkc)\.zip$").is_match(name);
        let cata = re(r"(?i)[-_](cata)\.zip$").is_match(name);
        let mists = re(r"(?i)[-_](mists)\.zip$").is_match(name);
        let forever = re(r"(?i)[-_](forever|camelot)\.zip$").is_match(name);
        match client {
            Retail | RetailPtr | RetailXPtr | Beta => !classic && !bc && !wotlk && !cata && !forever,
            ClassicEra | ClassicEraPtr => classic,
            Classic | ClassicPtr => mists,
            Forever | ClassicBeta => forever,
            Anniversary => bc,
            None => false,
        }
    }

    fn any_asset(r: &Release) -> Option<&Asset> {
        r.assets.iter().find(|a| Self::not_nolib(a) && Self::valid_content_type(a))
    }

    fn client_asset(r: &Release, client: WowClientType) -> Option<&Asset> {
        r.assets
            .iter()
            .find(|a| Self::not_nolib(a) && Self::valid_content_type(a) && Self::is_valid_client_asset_name(&a.name, client))
    }

    async fn release_json_asset<'a>(&self, r: &'a Release, client: WowClientType) -> Option<&'a Asset> {
        let meta_asset = r.assets.iter().find(|a| a.name == "release.json")?;
        let url = if meta_asset.browser_download_url.is_empty() { &meta_asset.url } else { &meta_asset.browser_download_url };
        let flavor = Self::meta_flavor(client)?;
        let meta: ReleaseMeta = match self.get(url, self.token.is_some()).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[github] release.json: {e:#}");
                return None;
            }
        };
        let entry = meta.releases.iter().find(|m| !m.nolib && m.metadata.iter().any(|f| f.flavor == flavor))?;
        r.assets.iter().find(|a| Self::valid_content_type(a) && a.name == entry.filename)
    }

    /// Returns (matched asset, release, any-client fallback asset).
    async fn latest_valid_asset<'a>(
        &self,
        releases: &'a [Release],
        client: WowClientType,
    ) -> (Option<&'a Asset>, Option<&'a Release>, Option<&'a Asset>) {
        let mut sorted: Vec<&Release> = releases.iter().filter(|r| !r.draft).collect();
        sorted.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        sorted.truncate(5);
        let mut latest_release = sorted.first().copied();
        let mut latest_asset = latest_release.and_then(Self::any_asset);
        for r in sorted {
            let mut asset = self.release_json_asset(r, client).await;
            if asset.is_none() {
                asset = Self::client_asset(r, client);
            }
            if let Some(a) = asset {
                latest_release = Some(r);
                latest_asset = Self::any_asset(r);
                return (Some(a), latest_release, latest_asset);
            }
        }
        (None, latest_release, latest_asset)
    }

    fn releases_to_check(releases: Vec<Release>, channel: AddonChannelType) -> Vec<Release> {
        if channel != AddonChannelType::Stable {
            return releases;
        }
        let (stable, pre): (Vec<Release>, Vec<Release>) = releases.into_iter().partition(|r| !r.prerelease);
        if stable.is_empty() {
            pre
        } else {
            stable
        }
    }

    async fn fetch(&self, owner: &str, repo: &str) -> anyhow::Result<(Repository, Vec<Release>)> {
        let repository: Repository = self.get(&format!("{API_URL}/{owner}/{repo}"), false).await?;
        let releases: Vec<Release> = self.get(&format!("{API_URL}/{owner}/{repo}/releases"), false).await?;
        Ok((repository, releases))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("repository not found")]
struct ProviderNotFound;

fn to_map(h: &[(&str, String)]) -> reqwest::header::HeaderMap {
    let mut m = reqwest::header::HeaderMap::new();
    for (k, v) in h {
        if let (Ok(k), Ok(v)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(v)) {
            m.insert(k, v);
        }
    }
    m
}

#[async_trait]
impl AddonProvider for GitHubProvider {
    fn name(&self) -> &'static str {
        PROVIDER_GITHUB
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { force_ignore: false, allow_reinstall: true, allow_rescan: false, ..Default::default() }
    }

    fn note(&self) -> &'static str {
        "Add a personal access token in Settings to avoid GitHub's anonymous rate limit."
    }

    async fn get_by_id(&self, id: &str, inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        let Some((owner, repo)) = id.split_once('/') else { return Ok(None) };
        let (repository, releases) = self.fetch(owner, repo).await?;
        if releases.is_empty() {
            return Ok(None);
        }
        let releases = Self::releases_to_check(releases, inst.default_addon_channel_type);
        let (matched, release, latest) = self.latest_valid_asset(&releases, inst.client_type).await;
        let (Some(asset), Some(release)) = (matched.or(latest), release) else { return Ok(None) };
        let name = id.split('/').filter(|s| !s.is_empty()).nth(1).unwrap_or(repo).to_string();
        Ok(Some(AddonSearchResult {
            author: repository.owner.login.clone(),
            external_id: id.to_string(),
            external_url: repository.html_url.clone(),
            name: name.clone(),
            provider_name: PROVIDER_GITHUB.into(),
            thumbnail_url: repository.owner.avatar_url.clone(),
            summary: repository.description.clone(),
            files: vec![AddonSearchResultFile {
                external_id: None,
                channel_type: AddonChannelType::Stable,
                download_url: asset.url.clone(),
                folders: vec![name],
                game_version: String::new(),
                version: asset.name.clone(),
                release_date: asset.created_at,
                dependencies: vec![],
                changelog: release.body.clone(),
                title: None,
                authors: None,
            }],
            ..Default::default()
        }))
    }

    async fn get_all(&self, inst: &WowInstallation, ids: &[String]) -> anyhow::Result<GetAllResult> {
        use futures::stream::{self, StreamExt};
        let results: Vec<(String, anyhow::Result<Option<AddonSearchResult>>)> = stream::iter(ids.iter().cloned())
            .map(|id| async move {
                let r = self.get_by_id(&id, inst).await;
                (id, r)
            })
            .buffer_unordered(3)
            .collect()
            .await;
        let mut out = GetAllResult::default();
        for (id, r) in results {
            match r {
                Ok(Some(sr)) => out.search_results.push(sr),
                Ok(None) => out.errors.push(ProviderError::msg(format!("no release found for {id}"))),
                Err(e) if e.is::<GitHubError>() => return Err(e),
                Err(e) if e.is::<ProviderNotFound>() => out.errors.push(ProviderError::removed(id)),
                Err(e) => out.errors.push(ProviderError { addon_id: Some(id), warning: None, message: format!("{e:#}") }),
            }
        }
        Ok(out)
    }

    async fn search_by_url(&self, url: &url::Url, inst: &WowInstallation) -> anyhow::Result<Option<AddonSearchResult>> {
        let (owner, repo) = Self::parse_repo_path(url.path()).ok_or_else(|| anyhow::anyhow!("Invalid URL"))?;
        let (repository, releases) = self.fetch(&owner, &repo).await?;
        let releases = Self::releases_to_check(releases, inst.default_addon_channel_type);
        let (matched, release, latest) = self.latest_valid_asset(&releases, inst.client_type).await;
        let (Some(asset), Some(release)) = (matched.or(latest), release) else {
            return Err(GitHubError::AssetMissing(url.to_string()).into());
        };
        if matched.is_none() {
            tracing::warn!("[github] no asset for {:?}, falling back to {}", inst.client_type, asset.name);
        }
        Ok(Some(AddonSearchResult {
            author: repository.owner.login.clone(),
            external_id: format!("{owner}/{repo}"),
            external_url: repository.html_url.clone(),
            name: repository.name.clone(),
            provider_name: PROVIDER_GITHUB.into(),
            thumbnail_url: repository.owner.avatar_url.clone(),
            download_count: Some(asset.download_count),
            summary: repository.description.clone(),
            files: vec![AddonSearchResultFile {
                external_id: None,
                channel_type: if release.prerelease { AddonChannelType::Beta } else { AddonChannelType::Stable },
                download_url: asset.url.clone(),
                folders: vec![],
                game_version: String::new(),
                version: asset.name.clone(),
                release_date: release.published_at,
                dependencies: vec![],
                changelog: release.body.clone(),
                title: None,
                authors: None,
            }],
            ..Default::default()
        }))
    }

    fn is_valid_addon_uri(&self, url: &url::Url) -> bool {
        url.host_str().is_some_and(|h| h.ends_with("github.com"))
    }

    fn is_valid_addon_id(&self, id: &str) -> bool {
        id.contains('/')
    }

    async fn download_auth(&self) -> Option<DownloadAuth> {
        let mut headers = vec![("Accept".to_string(), "application/octet-stream".to_string())];
        if let Some(t) = &self.token {
            headers.push(("Authorization".into(), format!("Bearer {t}")));
        }
        Some(DownloadAuth { headers, query_params: vec![] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_paths() {
        assert_eq!(GitHubProvider::parse_repo_path("/WeakAuras/WeakAuras2"), Some(("WeakAuras".into(), "WeakAuras2".into())));
        assert_eq!(
            GitHubProvider::parse_repo_path("/owner/repo/releases/tag/v1"),
            Some(("owner".into(), "repo".into()))
        );
    }

    #[test]
    fn asset_names() {
        use WowClientType::*;
        assert!(GitHubProvider::is_valid_client_asset_name("Foo-1.0.zip", Retail));
        assert!(!GitHubProvider::is_valid_client_asset_name("Foo-1.0-classic.zip", Retail));
        assert!(GitHubProvider::is_valid_client_asset_name("Foo-1.0-classic.zip", ClassicEra));
        assert!(GitHubProvider::is_valid_client_asset_name("Foo-1.0-mists.zip", Classic));
        assert!(GitHubProvider::is_valid_client_asset_name("Foo-1.0-bcc.zip", Anniversary));
    }
}
