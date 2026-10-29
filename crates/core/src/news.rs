// SPDX-License-Identifier: GPL-3.0-or-later
//! News panel feed, ported from `news.service.ts` (Warcraft Tavern RSS).

use chrono::{DateTime, Utc};
use regex::Regex;

use crate::net::Net;

/// Upstream's feed (`warcraftTavernNewsFeedUrl`), then the site's standard RSS as a fallback.
pub const FEEDS: [&str; 2] = [
    "https://www.warcrafttavern.com/?call_custom_simple_rss=1&csrp_post_type=wow-classic-news,tbc-classic-news,retail-news&csrp_thumbnail_size=medium",
    "https://www.warcrafttavern.com/feed/",
];

#[derive(Debug, Clone, PartialEq)]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub link: String,
    pub description: String,
    pub published_at: Option<DateTime<Utc>>,
    pub published_by: String,
    pub thumbnail: String,
}

fn tag(item: &str, name: &str) -> String {
    let re = Regex::new(&format!(r"(?s)<{0}[^>]*>(.*?)</{0}>", regex::escape(name))).unwrap();
    let raw = re.captures(item).map(|c| c[1].trim().to_string()).unwrap_or_default();
    let raw = raw.strip_prefix("<![CDATA[").and_then(|s| s.strip_suffix("]]>")).unwrap_or(&raw).to_string();
    crate::providers::html_to_text(&raw)
}

/// Parse RSS 2.0 items.
pub fn parse_rss(xml: &str) -> Vec<NewsItem> {
    let item_re = Regex::new(r"(?s)<item>(.*?)</item>").unwrap();
    let media = Regex::new(r#"<media:content[^>]*url="([^"]+)""#).unwrap();
    item_re
        .captures_iter(xml)
        .map(|c| {
            let it = &c[1];
            NewsItem {
                id: tag(it, "dc:identifier"),
                title: tag(it, "title"),
                link: tag(it, "link"),
                description: tag(it, "description"),
                published_at: DateTime::parse_from_rfc2822(&tag(it, "pubDate")).ok().map(|d| d.with_timezone(&Utc)),
                published_by: tag(it, "dc:creator"),
                thumbnail: media.captures(it).map(|m| m[1].to_string()).unwrap_or_default(),
            }
        })
        .collect()
}

/// Newest items from the first feed that returns any.
pub async fn load(net: &Net) -> Vec<NewsItem> {
    for url in std::env::var("WOWUP_NEWS_URL").ok().into_iter().chain(FEEDS.iter().map(|s| s.to_string())) {
        let url = url.as_str();
        match net.get_text(url, &[]).await {
            Ok(xml) => {
                let mut items = parse_rss(&xml);
                if !items.is_empty() {
                    items.sort_by(|a, b| b.published_at.cmp(&a.published_at));
                    return items;
                }
            }
            Err(e) => tracing::warn!("[news] {url}: {e:#}"),
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_items() {
        let xml = r#"<rss><channel><item><title><![CDATA[Patch 12.1 notes]]></title><link>https://x/a</link>
            <dc:creator>Writer</dc:creator><pubDate>Thu, 29 Oct 2026 16:51:30 +0000</pubDate>
            <description>&lt;p&gt;Big &amp; new&lt;/p&gt;</description><media:content url="https://x/t.jpg" /></item></channel></rss>"#;
        let items = parse_rss(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Patch 12.1 notes");
        assert_eq!(items[0].published_by, "Writer");
        assert_eq!(items[0].thumbnail, "https://x/t.jpg");
        assert!(items[0].published_at.is_some());
    }
}
