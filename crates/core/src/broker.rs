// SPDX-License-Identifier: GPL-3.0-or-later
//! Addon list import/export, ported from `addon-broker.service.ts`. The format is WowUp's, so
//! export strings can move between this app and upstream WowUp.

use anyhow::{bail, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::addons::AddonService;
use crate::models::{Addon, WowClientType, WowInstallation};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportAddon {
    pub id: String,
    pub name: String,
    pub provider_name: String,
    #[serde(default)]
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPayload {
    #[serde(default)]
    pub collection_name: Option<String>,
    pub client_type: String,
    pub addons: Vec<ExportAddon>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImportState {
    Conflict,
    Added,
    NoChange,
}

#[derive(Debug, Clone)]
pub struct ImportEntry {
    pub state: ImportState,
    pub reason: Option<&'static str>,
    pub addon: ExportAddon,
}

pub fn export(installation: &WowInstallation, addons: &[Addon]) -> Result<String> {
    let payload = ExportPayload {
        collection_name: Some(format!("WowUp_export_{}", chrono::Utc::now().timestamp_millis())),
        client_type: format!("{:?}", installation.client_type),
        addons: addons
            .iter()
            .filter(|a| !a.is_ignored && a.external_id.as_deref().is_some_and(|i| !i.is_empty()))
            .map(|a| ExportAddon {
                id: a.external_id.clone().unwrap_or_default(),
                name: a.name.clone(),
                provider_name: a.provider().to_string(),
                version_id: a.installed_external_release_id.clone(),
            })
            .collect(),
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&payload)?))
}

pub fn parse(input: &str) -> Result<ExportPayload> {
    let input = input.trim();
    let json = if input.starts_with('{') {
        input.as_bytes().to_vec()
    } else {
        base64::engine::general_purpose::STANDARD.decode(input)?
    };
    Ok(serde_json::from_slice(&json)?)
}

/// Compare an import against what is installed (`getImportSummary`).
pub fn summarize(svc: &AddonService, inst: &WowInstallation, payload: &ExportPayload, installed: &[Addon]) -> Result<Vec<ImportEntry>> {
    if payload.addons.is_empty() {
        bail!("Nothing to import");
    }
    if let Some(ct) = WowClientType::ALL.into_iter().find(|c| format!("{c:?}") == payload.client_type) {
        if ct.group() != inst.client_type.group() {
            bail!("This export is for {} and cannot be imported into {}", ct.display_name(), inst.client_type.display_name());
        }
    }
    let enabled: Vec<&str> = svc.enabled_providers().iter().map(|p| p.name()).collect();
    let missing: Vec<String> =
        payload.addons.iter().map(|a| a.provider_name.clone()).filter(|p| !enabled.contains(&p.as_str())).collect();
    if !missing.is_empty() {
        let mut m = missing;
        m.dedup();
        bail!("Enable these providers first: {}", m.join(", "));
    }
    let mut out: Vec<ImportEntry> = payload
        .addons
        .iter()
        .map(|imp| {
            let by_id = installed
                .iter()
                .find(|a| a.external_id.as_deref() == Some(&imp.id) && a.provider() == imp.provider_name);
            let by_name = || installed.iter().find(|a| a.name.eq_ignore_ascii_case(&imp.name));
            match by_id.or_else(by_name) {
                Some(a) if a.provider() != imp.provider_name => {
                    ImportEntry { state: ImportState::Conflict, reason: Some("provider mismatch"), addon: imp.clone() }
                }
                Some(a) if imp.version_id.is_some() && a.installed_external_release_id != imp.version_id => {
                    ImportEntry { state: ImportState::Conflict, reason: Some("version mismatch"), addon: imp.clone() }
                }
                Some(_) => ImportEntry { state: ImportState::NoChange, reason: None, addon: imp.clone() },
                None => ImportEntry { state: ImportState::Added, reason: None, addon: imp.clone() },
            }
        })
        .collect();
    out.sort_by_key(|e| e.state);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let inst = WowInstallation {
            id: "i".into(),
            client_type: WowClientType::Retail,
            location: "/x".into(),
            label: String::new(),
            display_name: "Retail".into(),
            selected: true,
            default_addon_channel_type: Default::default(),
            default_auto_update: false,
            available_update_count: 0,
        };
        let a = Addon {
            id: "1".into(),
            name: "Foo".into(),
            external_id: Some("123".into()),
            provider_name: Some("WowInterface".into()),
            ..Default::default()
        };
        let s = export(&inst, &[a]).unwrap();
        let p = parse(&s).unwrap();
        assert_eq!(p.addons[0].id, "123");
        assert_eq!(p.client_type, "Retail");
    }
}
