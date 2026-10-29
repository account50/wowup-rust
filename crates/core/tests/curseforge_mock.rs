// SPDX-License-Identifier: GPL-3.0-or-later
//! CurseForge provider against a mock CF Core API: fingerprint matching of an installed addon,
//! sync, update, install by id with a required dependency, search, and removal.

mod common;

use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wowup_core::addons::no_progress;
use wowup_core::fingerprint::{FolderScanner, HashesNeeded};
use wowup_core::models::*;

const KEY: &str = "test-curseforge-key";

fn file(id: i64, name: &str, version: &str, url: String, fp: u32, deps: serde_json::Value) -> serde_json::Value {
    json!({
        "id": id, "displayName": version, "downloadUrl": url, "fileDate": "2026-10-01T00:00:00Z",
        "releaseType": 1, "gameVersions": ["11.0.2"], "sortableGameVersions": [{"gameVersionTypeId": 517}],
        "modules": [{"name": name, "fingerprint": fp}], "dependencies": deps
    })
}

fn modr(id: i64, name: &str, latest: serde_json::Value) -> serde_json::Value {
    json!({
        "id": id, "name": name, "slug": name.to_lowercase(), "summary": format!("{name} summary"),
        "downloadCount": 1000.0, "links": {"websiteUrl": format!("https://www.curseforge.com/wow/addons/{}", name.to_lowercase())},
        "logo": {"thumbnailUrl": "https://example.invalid/t.png"}, "screenshots": [], "authors": [{"name": "Author"}],
        "latestFiles": [latest]
    })
}

#[tokio::test]
async fn curseforge_scan_update_install_remove() {
    let _env = common::env_lock();
    let server = MockServer::start().await;
    std::env::set_var("WOWUP_CURSEFORGE_URL", server.uri());
    let sb = common::sandbox(&["Curse"], |p| p.curseforge_api_key = Some(KEY.into()));
    let addons_dir = sb.addons_dir();

    // An Auctionator folder already on disk, as on the real install.
    common::write_addon(&addons_dir, "Auctionator", "333");
    let fp = FolderScanner::new(&addons_dir.join("Auctionator"))
        .scan(HashesNeeded { wowup: false, curseforge: true })
        .curseforge
        .unwrap()
        .fingerprint_num;

    let dl = |f: &str| format!("{}/dl/{f}", server.uri());
    let installed = file(5001, "Auctionator", "333", dl("auc-333.zip"), fp, json!([]));
    let latest = file(5002, "Auctionator", "334", dl("auc-334.zip"), 1, json!([]));

    Mock::given(method("POST"))
        .and(path("/fingerprints"))
        .and(header("x-api-key", KEY))
        .and(body_partial_json(json!({ "fingerprints": [fp] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"exactMatches": [{"id": 1001, "file": installed}], "partialMatches": []}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mods"))
        .and(body_partial_json(json!({ "modIds": [1001] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [modr(1001, "Auctionator", latest.clone())]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/dl/auc-334.zip"))
        .and(header("X-Api-Key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(common::addon_zip(&["Auctionator"], "334")))
        .mount(&server)
        .await;

    // Scan: the unknown folder is matched to CurseForge by fingerprint.
    let addons = sb.svc.rescan(&sb.inst).await.unwrap();
    assert_eq!(addons.len(), 1);
    let auc = &addons[0];
    assert_eq!(auc.provider(), PROVIDER_CURSEFORGE);
    assert_eq!(auc.external_id.as_deref(), Some("1001"));
    assert_eq!(auc.installed_version.as_deref(), Some("333"));
    assert!(auc.needs_update(), "334 is available");

    // Sync keeps it outdated, then update installs 334.
    let issues = sb.svc.sync_installation(&sb.inst).await;
    assert!(issues.is_empty(), "{issues:?}");
    let auc = sb.svc.get_addons(&sb.inst, false).await.unwrap().remove(0);
    assert_eq!(auc.latest_version.as_deref(), Some("334"));
    let updated = sb.svc.update_addon(auc, no_progress()).await.unwrap();
    assert_eq!(updated.installed_version.as_deref(), Some("334"));
    assert!(!updated.needs_update());
    let toc = std::fs::read_to_string(addons_dir.join("Auctionator/Auctionator.toc")).unwrap();
    assert!(toc.contains("## Version: 334"));
    assert!(!addons_dir.join("Auctionator-bak").exists(), "backup cleaned up");

    // Install by id with a required dependency, which is installed too.
    let details_file = file(6001, "Details", "1.0", dl("details.zip"), 2, json!([{"modId": 3003, "relationType": 3}]));
    let lib_file = file(7001, "LibDep", "2.0", dl("libdep.zip"), 3, json!([]));
    Mock::given(method("GET"))
        .and(path("/mods/2002"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": modr(2002, "Details", details_file)})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mods/3003"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": modr(3003, "LibDep", lib_file)})))
        .mount(&server)
        .await;
    for (f, folder, v) in [("details.zip", "Details", "1.0"), ("libdep.zip", "LibDep", "2.0")] {
        Mock::given(method("GET"))
            .and(path(format!("/dl/{f}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(common::addon_zip(&[folder], v)))
            .mount(&server)
            .await;
    }
    let details = sb.svc.install_by_id(PROVIDER_CURSEFORGE, "2002", &sb.inst, no_progress()).await.unwrap();
    assert_eq!(details.installed_folder_list, vec!["Details"]);
    let all = sb.svc.get_addons(&sb.inst, false).await.unwrap();
    assert!(all.iter().any(|a| a.name == "LibDep"), "required dependency installed: {:?}", all.iter().map(|a| &a.name).collect::<Vec<_>>());
    assert!(addons_dir.join("LibDep/LibDep.toc").exists());
    assert!(sb.svc.install_by_id(PROVIDER_CURSEFORGE, "2002", &sb.inst, no_progress()).await.is_err(), "no double install");

    // Search.
    Mock::given(method("GET"))
        .and(path("/mods/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [modr(1001, "Auctionator", latest)]})))
        .mount(&server)
        .await;
    let (results, errs) = sb.svc.search("auctionator", &sb.inst).await;
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(results[0].provider_name, PROVIDER_CURSEFORGE);

    // Remove Details with its dependencies: both folders and records go.
    let details = sb.svc.get_addons(&sb.inst, false).await.unwrap().into_iter().find(|a| a.name == "Details").unwrap();
    sb.svc.remove_addon(&details, true, true).unwrap();
    assert!(!addons_dir.join("Details").exists());
    assert!(!addons_dir.join("LibDep").exists());
    let left: Vec<String> = sb.svc.get_addons(&sb.inst, false).await.unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(left, vec!["Auctionator"]);

    // Remove the scanned addon too.
    let auc = sb.svc.get_addons(&sb.inst, false).await.unwrap().remove(0);
    sb.svc.remove_addon(&auc, false, true).unwrap();
    assert!(!addons_dir.join("Auctionator").exists());
}

#[tokio::test]
async fn curseforge_protocol_link() {
    let _env = common::env_lock();
    let server = MockServer::start().await;
    std::env::set_var("WOWUP_CURSEFORGE_URL", server.uri());
    let sb = common::sandbox(&["Curse"], |p| p.curseforge_api_key = Some(KEY.into()));
    let f = file(9002, "Clique", "5.0", format!("{}/dl/clique.zip", server.uri()), 4, json!([]));
    Mock::given(method("GET"))
        .and(path("/mods/9001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": modr(9001, "Clique", f.clone())})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mods/9001/files/9002"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": f})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/dl/clique.zip"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(common::addon_zip(&["Clique"], "5.0")))
        .mount(&server)
        .await;
    let p = sb.svc.search_protocol("curseforge://install?addonId=9001&fileId=9002").await.unwrap();
    assert!(p.valid_client_types.contains(&WowClientType::Retail));
    let file = p.search_result.files.first().cloned();
    let a = sb.svc.install_search_result(&p.search_result, &sb.inst, file.as_ref(), no_progress()).await.unwrap();
    assert_eq!(a.installed_version.as_deref(), Some("5.0"));
    assert!(sb.addons_dir().join("Clique/Clique.toc").exists());
}

#[tokio::test]
async fn curseforge_disabled_without_key() {
    let sb = common::sandbox(&["Curse"], |_| {});
    assert!(sb.svc.enabled_providers().iter().all(|p| p.name() != PROVIDER_CURSEFORGE));
}
