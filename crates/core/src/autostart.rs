// SPDX-License-Identifier: GPL-3.0-or-later
//! Launch at login, the counterpart of upstream's `start_with_system` / `start_minimized`
//! (Electron login items + tray). On macOS this is a user LaunchAgent:
//! - start with system: open WowUp.rs at login;
//! - start minimized: don't open a window, run `wowup auto-update-pass --notify` at login and then
//!   on the auto-update interval, which is what the hidden tray app did upstream.

use std::path::{Path, PathBuf};

use anyhow::Result;

pub const LABEL: &str = "io.wowup.rs";

pub fn agent_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The LaunchAgent plist for the given mode.
pub fn plist(cli: &Path, minimized: bool, interval_minutes: u64, log: &Path) -> String {
    let args: Vec<String> = if minimized {
        vec![cli.display().to_string(), "auto-update-pass".into(), "--notify".into()]
    } else {
        vec!["/usr/bin/open".into(), "-a".into(), "WowUp.rs".into()]
    };
    let args_xml: String = args.iter().map(|a| format!("\t\t<string>{}</string>\n", xml_escape(a))).collect();
    let interval = if minimized {
        format!("\t<key>StartInterval</key>\n\t<integer>{}</integer>\n", interval_minutes.max(5) * 60)
    } else {
        String::new()
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Label</key>\n\t<string>{LABEL}</string>\n\t<key>ProgramArguments</key>\n\t<array>\n{args_xml}\t</array>\n\t<key>RunAtLoad</key>\n\t<true/>\n{interval}\t<key>StandardOutPath</key>\n\t<string>{log}</string>\n\t<key>StandardErrorPath</key>\n\t<string>{log}</string>\n</dict>\n</plist>\n",
        log = xml_escape(&log.display().to_string())
    )
}

fn launchctl(args: &[&str]) {
    let _ = std::process::Command::new("launchctl").args(args).output();
}

/// Apply the login setting. `cli` is the `wowup` binary used for background passes.
pub fn apply(enabled: bool, minimized: bool, interval_minutes: u64, cli: &Path, log: &Path) -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("launch at login is only implemented for macOS");
    }
    let Some(path) = agent_path() else { anyhow::bail!("no home directory") };
    if path.exists() {
        launchctl(&["unload", &path.to_string_lossy()]);
        std::fs::remove_file(&path)?;
    }
    if enabled {
        std::fs::create_dir_all(path.parent().unwrap())?;
        // Agents in ~/Library/LaunchAgents load at the next login; it is not started now, since
        // RunAtLoad would immediately open a second window or run a pass.
        std::fs::write(&path, plist(cli, minimized, interval_minutes, log))?;
    }
    Ok(())
}

pub fn is_installed() -> bool {
    agent_path().is_some_and(|p| p.exists())
}

/// Desktop notification (macOS Notification Center via osascript, `notify-send` on Linux).
pub fn notify(title: &str, body: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!("display notification \"{}\" with title \"{}\"", esc(body), esc(title)))
            .spawn();
    } else if cfg!(target_os = "linux") {
        let _ = std::process::Command::new("notify-send").arg(title).arg(body).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plists() {
        let p = plist(Path::new("/x/wowup"), true, 60, Path::new("/tmp/l.log"));
        assert!(p.contains("<string>auto-update-pass</string>"));
        assert!(p.contains("<integer>3600</integer>"));
        let p = plist(Path::new("/x/wowup"), false, 60, Path::new("/tmp/l.log"));
        assert!(p.contains("<string>/usr/bin/open</string>"));
        assert!(!p.contains("StartInterval"));
    }
}
