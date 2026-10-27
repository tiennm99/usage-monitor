//! Pure builders for the JSON payloads the bubble and panel webviews render.
//!
//! Rust owns every displayed string and colour; the webview only lays out
//! pixels. Field names are camelCase on the wire.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::core::i18n::{self, LocaleStrings};
use crate::core::state::ProviderUiState;
use crate::core::usage_color::{accent_color_for, bar_fill_color};
use crate::usage::ProviderId;

const FIVE_HOURS_SECS: u64 = 5 * 60 * 60;
const SEVEN_DAYS_SECS: u64 = 7 * 24 * 60 * 60;

/// Placeholder for the head label and weekly countdown before the first poll.
const PENDING: &str = "…";

/// Length of a provider's `(primary, secondary)` usage windows in seconds.
/// Claude exposes 5h/7d directly; Codex's primary/secondary map to the same
/// short/long windows in the compact UI.
pub fn window_durations_secs(provider: ProviderId) -> (u64, u64) {
    match provider {
        ProviderId::Claude | ProviderId::ChatGpt => (FIVE_HOURS_SECS, SEVEN_DAYS_SECS),
    }
}

/// The bubble window's canvas, in physical px.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub dpi: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BubblePayload {
    pub provider: ProviderId,
    pub session_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
    /// `"73%"`, or `"—"` with no value.
    pub session_pct_text: String,
    /// `None` hides the weekly percent.
    pub weekly_pct_text: Option<String>,
    /// Session countdown; `"…"` before the first poll. The UI swaps an empty
    /// or overflowing label for `"5H"`.
    pub head_label: String,
    /// Weekly countdown; `"…"` before the first poll, `""` hides it.
    pub weekly_countdown: String,
    pub session_resets_at_ms: Option<u64>,
    pub weekly_resets_at_ms: Option<u64>,
    pub primary_dur_secs: u64,
    pub secondary_dur_secs: u64,
    pub session_fill: String,
    pub weekly_fill: String,
    pub accent: String,
    pub is_dark: bool,
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub dpi: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelPayload {
    pub provider: ProviderId,
    pub label: String,
    pub is_dark: bool,
    pub accent: String,
    pub session_pct: f64,
    pub weekly_pct: f64,
    /// `"73% · 2h"`, `"!"` after an auth failure, `"…"` after a transient
    /// error, `""` before the first poll.
    pub session_text: String,
    pub weekly_text: String,
    pub session_fill: String,
    pub weekly_fill: String,
}

fn unix_ms(t: Option<SystemTime>) -> Option<u64> {
    let ms = t?.duration_since(UNIX_EPOCH).ok()?.as_millis();
    u64::try_from(ms).ok()
}

/// Build the bubble payload. `snapshot` is `None` before the provider's
/// first poll. After an error the snapshot still holds the last good
/// windows, so the bubble keeps those values and countdowns; it never shows
/// the panel's `"!"` or `"…"` markers.
pub fn bubble_payload(
    provider: ProviderId,
    snapshot: Option<&ProviderUiState>,
    strings: &LocaleStrings,
    is_dark: bool,
    canvas: Canvas,
    now: SystemTime,
) -> BubblePayload {
    let windows = snapshot.map(|s| s.windows);
    let session_pct = windows.map(|w| w.primary.utilization);
    let weekly_pct = windows.map(|w| w.secondary.utilization);
    let (head_label, weekly_countdown) = match windows {
        Some(w) => (
            i18n::format_countdown_at(w.primary.resets_at, strings, now),
            i18n::format_countdown_at(w.secondary.resets_at, strings, now),
        ),
        None => (PENDING.to_string(), PENDING.to_string()),
    };
    let (primary_dur_secs, secondary_dur_secs) = window_durations_secs(provider);
    BubblePayload {
        provider,
        session_pct,
        weekly_pct,
        session_pct_text: i18n::format_pct(session_pct),
        weekly_pct_text: weekly_pct.map(|p| i18n::format_pct(Some(p))),
        head_label,
        weekly_countdown,
        session_resets_at_ms: unix_ms(windows.and_then(|w| w.primary.resets_at)),
        weekly_resets_at_ms: unix_ms(windows.and_then(|w| w.secondary.resets_at)),
        primary_dur_secs,
        secondary_dur_secs,
        session_fill: bar_fill_color(provider, is_dark, session_pct.unwrap_or(0.0)).to_hex(),
        weekly_fill: bar_fill_color(provider, is_dark, weekly_pct.unwrap_or(0.0)).to_hex(),
        accent: accent_color_for(provider, is_dark).to_hex(),
        is_dark,
        canvas_w: canvas.w,
        canvas_h: canvas.h,
        dpi: canvas.dpi,
    }
}

/// Build the details-panel payload. Before the first poll the bars are
/// empty and the texts blank.
pub fn panel_payload(
    provider: ProviderId,
    snapshot: Option<&ProviderUiState>,
    strings: &LocaleStrings,
    is_dark: bool,
) -> PanelPayload {
    let state = snapshot.cloned().unwrap_or_default();
    let session_pct = state.windows.primary.utilization;
    let weekly_pct = state.windows.secondary.utilization;
    PanelPayload {
        provider,
        label: strings.provider_label(provider).to_string(),
        is_dark,
        accent: accent_color_for(provider, is_dark).to_hex(),
        session_pct,
        weekly_pct,
        session_text: state.primary_text,
        weekly_text: state.secondary_text,
        session_fill: bar_fill_color(provider, is_dark, session_pct).to_hex(),
        weekly_fill: bar_fill_color(provider, is_dark, weekly_pct).to_hex(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::core::i18n::I18n;
    use crate::core::state::{PollOutcome, UsageState};
    use crate::usage::{UsageWindows, Window};

    const CANVAS: Canvas = Canvas {
        w: 300,
        h: 99,
        dpi: 144,
    };

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn strings() -> LocaleStrings {
        I18n::load().strings().clone()
    }

    fn polled(session: f64, weekly: f64) -> UsageState {
        let mut state = UsageState::default();
        let windows = UsageWindows {
            primary: Window {
                utilization: session,
                resets_at: Some(now() + Duration::from_secs(2 * 3_600 + 10)),
            },
            secondary: Window {
                utilization: weekly,
                resets_at: Some(now() + Duration::from_secs(3 * 86_400)),
            },
        };
        state.apply_results(
            vec![(ProviderId::Claude, PollOutcome::Ok(windows))],
            &strings(),
            Instant::now(),
            SystemTime::now(),
        );
        state
    }

    #[test]
    fn before_first_poll_shows_placeholders() {
        let p = bubble_payload(ProviderId::ChatGpt, None, &strings(), true, CANVAS, now());
        assert_eq!(p.session_pct, None);
        assert_eq!(p.session_pct_text, "—");
        assert_eq!(p.weekly_pct_text, None);
        assert_eq!(p.head_label, "…");
        assert_eq!(p.weekly_countdown, "…");
        assert_eq!(p.session_resets_at_ms, None);
        assert_eq!(p.session_fill, "#E5E5E5");
        assert_eq!(p.accent, "#E5E5E5");
        assert_eq!((p.canvas_w, p.canvas_h, p.dpi), (300, 99, 144));

        let panel = panel_payload(ProviderId::ChatGpt, None, &strings(), false);
        assert_eq!(panel.session_text, "");
        assert_eq!(panel.weekly_text, "");
        assert_eq!(panel.session_pct, 0.0);
        assert_eq!(panel.label, "Codex");
        assert_eq!(panel.accent, "#1A1A1A");
    }

    #[test]
    fn polled_values_format_in_rust() {
        let state = polled(72.5, 96.0);
        let snap = state.snapshots.get(&ProviderId::Claude);
        let p = bubble_payload(ProviderId::Claude, snap, &strings(), true, CANVAS, now());
        assert_eq!(p.session_pct, Some(72.5));
        assert_eq!(p.session_pct_text, "72%");
        assert_eq!(p.weekly_pct_text.as_deref(), Some("96%"));
        assert_eq!(p.head_label, "2h");
        assert_eq!(p.weekly_countdown, "3d");
        assert_eq!(
            p.session_resets_at_ms,
            Some((1_800_000_000 + 2 * 3_600 + 10) * 1_000)
        );
        assert_eq!(
            (p.primary_dur_secs, p.secondary_dur_secs),
            (18_000, 604_800)
        );
        assert_eq!(p.session_fill, "#E0A040");
        assert_eq!(p.weekly_fill, "#A01818");
        assert_eq!(p.accent, "#D97757");
    }

    #[test]
    fn errors_keep_stale_bubble_values_but_mark_the_panel() {
        let mut state = polled(40.0, 20.0);
        state.apply_results(
            vec![(ProviderId::Claude, PollOutcome::AuthFailed)],
            &strings(),
            Instant::now(),
            SystemTime::now(),
        );
        let snap = state.snapshots.get(&ProviderId::Claude);
        let p = bubble_payload(ProviderId::Claude, snap, &strings(), false, CANVAS, now());
        assert_eq!(p.session_pct_text, "40%");
        assert_eq!(p.head_label, "2h");
        assert_eq!(p.weekly_countdown, "3d");
        let json = serde_json::to_string(&p).expect("serialise");
        assert!(!json.contains('!'), "{json}");

        let panel = panel_payload(ProviderId::Claude, snap, &strings(), false);
        assert_eq!(panel.session_text, "!");
        assert_eq!(panel.weekly_text, "!");
        assert_eq!(panel.session_pct, 40.0);

        state.apply_results(
            vec![(ProviderId::Claude, PollOutcome::Failed)],
            &strings(),
            Instant::now(),
            SystemTime::now(),
        );
        let snap = state.snapshots.get(&ProviderId::Claude);
        let panel = panel_payload(ProviderId::Claude, snap, &strings(), false);
        assert_eq!(panel.session_text, "…");
        assert_eq!(panel.weekly_text, "…");
        let p = bubble_payload(ProviderId::Claude, snap, &strings(), false, CANVAS, now());
        assert_eq!(p.head_label, "2h");
    }

    #[test]
    fn wire_format_is_camel_case_with_nulls() {
        let p = bubble_payload(ProviderId::Claude, None, &strings(), true, CANVAS, now());
        let v = serde_json::to_value(&p).expect("serialise");
        assert_eq!(v["provider"], "claude");
        assert!(v["sessionPct"].is_null());
        assert!(v["weeklyPctText"].is_null());
        assert_eq!(v["sessionPctText"], "—");
        assert_eq!(v["canvasW"], 300);
        assert_eq!(v["isDark"], true);
        let mut keys: Vec<&str> = v
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "accent",
                "canvasH",
                "canvasW",
                "dpi",
                "headLabel",
                "isDark",
                "primaryDurSecs",
                "provider",
                "secondaryDurSecs",
                "sessionFill",
                "sessionPct",
                "sessionPctText",
                "sessionResetsAtMs",
                "weeklyCountdown",
                "weeklyFill",
                "weeklyPct",
                "weeklyPctText",
                "weeklyResetsAtMs",
            ]
        );

        let panel = serde_json::to_value(panel_payload(ProviderId::Claude, None, &strings(), true))
            .expect("serialise");
        for key in [
            "provider",
            "label",
            "isDark",
            "accent",
            "sessionPct",
            "weeklyPct",
            "sessionText",
            "weeklyText",
            "sessionFill",
            "weeklyFill",
        ] {
            assert!(panel.get(key).is_some(), "missing {key}");
        }
    }
}
