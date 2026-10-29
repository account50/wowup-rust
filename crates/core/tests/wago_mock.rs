// SPDX-License-Identifier: GPL-3.0-or-later
//! Wago provider against a mock Wago API: fingerprint matching, sync, update with the user's key
//! (Bearer header + `?token=` download), search, install and removal. No ad token is involved.

mod common;

use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wowup_core::addons::no_progress;
use wowup_core::fingerprint::{FolderScanner, HashesNeeded};
use wowup_core::models::*;

const KEY: &str = "user-supplied-wago-key-0123456789";

#[tokio::test]
async fn wago_scan_update_search_install_remove() {
    let _env = common::env_lock();
    let server = MockServer::start().await;
    std::env::set_var("WOWUP_WAGO_URL", server.uri());
    let sb = common::sandbox(&["Wago"], |p| p.wago_api_key = Some(KEY.into()));
    let addons_dir = sb.addons_dir();
    common::write_addon(&addons_dir, "Auctionator", "333");
    let md5 = FolderScanner::new(&addons_dir.join("Auctionator"))
        .scan(HashesNeeded { wowup: true, curseforge: false })
        .wowup
        .unwrap()
        .fingerprint;
    let link = format!("{}/files/auc-334.zip", server.uri());
    let scan_addon = json!({
        "id": "aucwago123", "name": "Auctionator", "thumbnail": "https://example.invalid/t.png",
        "website_url": "https://addons.wago.io/addons/auctionator", "authors": ["plusmouse"],
        "matched_release": {"id": "r333", "created_at": "2026-09-01T00:00:00Z", "label": "333", "patch": "11.0.2", "supported_patches": ["11.0.2"]},
        "modules": {"Auctionator": {"hash": md5}},
        "recent_releases": {"stable": {"id": "r334", "created_at": "2026-10-01T00:00:00Z", "label": "334", "patch": "11.0.2", "supported_patches": ["11.0.2"], "link": link}}
    });
    let bearer = format!("Bearer {KEY}");
    Mock::given(method("POST"))
        .and(path("/addons/_match"))
        .and(header("Authorization", bearer.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"addons": [scan_addon.clone(), null]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/addons/_recents"))
        .and(header("Authorization", bearer.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"addons": {"aucwago123": scan_addon}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files/auc-334.zip"))
        .and(query_param("token", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(common::addon_zip(&["Auctionator"], "334")))
        .mount(&server)
        .await;

    let addons = sb.svc.rescan(&sb.inst).await.unwrap();
    assert_eq!(addons[0].provider(), PROVIDER_WAGO);
    assert_eq!(addons[0].installed_version.as_deref(), Some("333"));
    assert!(addons[0].needs_update());

    assert!(sb.svc.sync_installation(&sb.inst).await.is_empty());
    let auc = sb.svc.get_addons(&sb.inst, false).await.unwrap().remove(0);
    let updated = sb.svc.update_addon(auc, no_progress()).await.unwrap();
    assert_eq!(updated.installed_version.as_deref(), Some("334"));
    assert!(!updated.needs_update());
    assert!(std::fs::read_to_string(addons_dir.join("Auctionator/Auctionator.toc")).unwrap().contains("334"));

    // Channel change is allowed for Wago.
    let mut auc = sb.svc.get_addons(&sb.inst, false).await.unwrap().remove(0);
    sb.svc.set_channel(&mut auc, AddonChannelType::Beta).unwrap();

    // Search and install another addon.
    let bags = json!({"data": [{
        "id": "bagswago12", "display_name": "Baganator", "summary": "Bags", "thumbnail_image": "", "website_url": "https://addons.wago.io/addons/baganator",
        "authors": ["plusmouse"], "download_count": 5,
        "releases": {"stable": {"download_link": format!("{}/files/bags.zip", server.uri()), "label": "1.0", "created_at": "2026-10-02T00:00:00Z"}}
    }]});
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .and(query_param("query", "baganator"))
        .and(query_param("game_version", "retail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(bags))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files/bags.zip"))
        .and(query_param("token", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(common::addon_zip(&["Baganator"], "1.0")))
        .mount(&server)
        .await;
    let (results, errs) = sb.svc.search("baganator", &sb.inst).await;
    assert!(errs.is_empty(), "{errs:?}");
    let bag = sb.svc.install_search_result(&results[0], &sb.inst, None, no_progress()).await.unwrap();
    assert_eq!(bag.installed_folder_list, vec!["Baganator"]);

    // Remove both.
    for a in sb.svc.get_addons(&sb.inst, false).await.unwrap() {
        sb.svc.remove_addon(&a, false, true).unwrap();
    }
    assert!(!addons_dir.join("Auctionator").exists());
    assert!(!addons_dir.join("Baganator").exists());
    assert!(sb.svc.get_addons(&sb.inst, false).await.unwrap().is_empty());
}

#[tokio::test]
async fn wago_requires_user_key() {
    // A short or missing key leaves Wago unconfigured; nothing ever asks an ad page for a token.
    let sb = common::sandbox(&["Wago"], |p| p.wago_api_key = Some("short".into()));
    assert!(sb.svc.enabled_providers().iter().all(|p| p.name() != PROVIDER_WAGO));
}
