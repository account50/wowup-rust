// SPDX-License-Identifier: GPL-3.0-or-later
//! Window state and zoom, the terminal counterparts of upstream's `window-state.ts` and
//! `zoom.service.ts`. In Terminal.app the window that hosts the TUI is scripted directly:
//! its bounds are saved on quit and restored on launch, and zoom changes its font size.
//! Other terminals keep their own size and font, so these become no-ops there.

use std::process::{Command, Stdio};

/// Upstream's zoom steps (`ZOOM_FACTORS`).
pub const ZOOM_FACTORS: [f64; 14] = [0.5, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0];

pub struct TerminalWindow {
    id: String,
}

fn osascript(script: &str) -> Option<String> {
    let out = Command::new("osascript").arg("-e").arg(script).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn own_tty() -> Option<String> {
    let out = Command::new("tty").stdin(Stdio::inherit()).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    s.starts_with("/dev/").then_some(s)
}

impl TerminalWindow {
    /// The Terminal.app window whose tab owns our tty, if we run in Terminal.app.
    pub fn find() -> Option<Self> {
        if std::env::var("TERM_PROGRAM").ok().as_deref() != Some("Apple_Terminal") {
            return None;
        }
        let tty = own_tty()?;
        let id = osascript(&format!(
            "tell application \"Terminal\"\n repeat with w in windows\n  repeat with t in tabs of w\n   if tty of t is \"{tty}\" then return id of w\n  end repeat\n end repeat\nend tell"
        ))?;
        (!id.is_empty()).then_some(Self { id })
    }

    pub fn bounds(&self) -> Option<[i32; 4]> {
        let s = osascript(&format!("tell application \"Terminal\" to get bounds of window id {}", self.id))?;
        let v: Vec<i32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
    }

    pub fn set_bounds(&self, b: [i32; 4]) {
        let _ = osascript(&format!(
            "tell application \"Terminal\" to set bounds of window id {} to {{{}, {}, {}, {}}}",
            self.id, b[0], b[1], b[2], b[3]
        ));
    }

    pub fn font_size(&self) -> Option<f64> {
        osascript(&format!("tell application \"Terminal\" to get font size of selected tab of window id {}", self.id))?
            .parse()
            .ok()
    }

    pub fn set_font_size(&self, size: f64) {
        let _ = osascript(&format!(
            "tell application \"Terminal\" to set font size of selected tab of window id {} to {}",
            self.id,
            size.round().max(6.0)
        ));
    }
}

/// The next zoom step up (+1) or down (-1) from `current`.
pub fn step_zoom(current: f64, dir: i32) -> f64 {
    let idx = ZOOM_FACTORS.iter().position(|z| (*z - current).abs() < 1e-6).unwrap_or(4) as i32;
    ZOOM_FACTORS[(idx + dir).clamp(0, ZOOM_FACTORS.len() as i32 - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps() {
        assert_eq!(step_zoom(1.0, 1), 1.1);
        assert_eq!(step_zoom(1.0, -1), 0.9);
        assert_eq!(step_zoom(3.0, 1), 3.0);
        assert_eq!(step_zoom(0.5, -1), 0.5);
    }
}
