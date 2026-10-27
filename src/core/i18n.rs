// English UI strings and formatting helpers.
//
// Every string the UI displays is formatted here; the webview never formats
// a number for display.

use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::usage::{ProviderId, Window};

/// The strings every UI module needs. Serialised camelCase for the
/// webviews.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleStrings {
    pub window_title: String,
    pub refresh: String,
    pub update_frequency: String,
    pub one_minute: String,
    pub five_minutes: String,
    pub fifteen_minutes: String,
    pub one_hour: String,
    pub providers: String,
    pub claude_label: String,
    pub chatgpt_label: String,
    pub settings: String,
    pub start_with_windows: String,
    pub start_at_login: String,
    pub reset_position: String,
    pub size_smaller: String,
    pub size_larger: String,
    pub reset_size: String,
    pub controls: String,
    pub control_left_click: String,
    pub control_right_click: String,
    pub control_drag: String,
    pub control_ctrl_wheel: String,
    pub control_tray_click: String,
    /// Linux variant of `control_tray_click`: AppIndicator delivers no tray
    /// clicks, so every click opens the menu.
    pub control_tray_click_linux: String,
    pub tray_left_click: String,
    pub check_for_updates: String,
    pub checking_for_updates: String,
    pub up_to_date: String,
    pub update_failed: String,
    pub applying_update: String,
    pub update_available: String,
    pub update_via_winget: String,
    pub auto_update_check: String,
    pub auto_check_disabled: String,
    pub auto_check_hourly: String,
    pub auto_check_daily: String,
    pub auto_check_weekly: String,
    pub exit: String,
    pub restart: String,
    pub show_widget: String,
    pub click_through: String,
    pub session_window: String,
    pub weekly_window: String,
    pub now: String,
    pub day_suffix: String,
    pub hour_suffix: String,
    pub minute_suffix: String,
    pub second_suffix: String,
    pub token_expired_title: String,
    pub token_expired_body: String,
    pub chatgpt_token_expired_title: String,
    pub chatgpt_token_expired_body: String,
    /// Body text for "your usage just crossed 80% of the 5h limit". The
    /// title is composed from the provider label + percent.
    pub threshold_80_body: String,
    /// Body text for the 95% threshold balloon.
    pub threshold_95_body: String,
    /// Title for the tray balloon shown on first launch after an auto-update.
    pub update_applied_title: String,
    /// Prefix for the tray balloon body. Call site appends the version.
    pub update_applied_body: String,
    /// Prefix for the rollback-failed MessageBox body. Call site appends
    /// the backup path and a separator with the expected target filename.
    pub update_rollback_failed_body: String,
}

pub struct I18n {
    strings: LocaleStrings,
}

impl I18n {
    pub fn load() -> Self {
        Self {
            strings: english_strings(),
        }
    }

    pub fn strings(&self) -> &LocaleStrings {
        &self.strings
    }
}

impl LocaleStrings {
    /// Label of the autostart menu item for the platform this build targets.
    pub fn autostart_label(&self) -> &str {
        if cfg!(windows) {
            &self.start_with_windows
        } else {
            &self.start_at_login
        }
    }

    /// Controls hint for the tray icon on the platform this build targets.
    /// Linux tray icons (AppIndicator) only open the menu, so the
    /// show/hide click is not offered there.
    pub fn tray_click_hint(&self) -> &str {
        if cfg!(target_os = "linux") {
            &self.control_tray_click_linux
        } else {
            &self.control_tray_click
        }
    }

    /// Display name of a provider.
    pub fn provider_label(&self, provider: ProviderId) -> &str {
        match provider {
            ProviderId::Claude => &self.claude_label,
            ProviderId::ChatGpt => &self.chatgpt_label,
        }
    }
}

fn english_strings() -> LocaleStrings {
    LocaleStrings {
        window_title: "Usage Monitor".into(),
        refresh: "Refresh".into(),
        update_frequency: "Update frequency".into(),
        one_minute: "1 minute".into(),
        five_minutes: "5 minutes".into(),
        fifteen_minutes: "15 minutes".into(),
        one_hour: "1 hour".into(),
        providers: "Providers".into(),
        claude_label: "Claude Code".into(),
        chatgpt_label: "Codex".into(),
        settings: "Settings".into(),
        start_with_windows: "Start with Windows".into(),
        start_at_login: "Start at login".into(),
        reset_position: "Reset position".into(),
        size_smaller: "Make smaller".into(),
        size_larger: "Make larger".into(),
        reset_size: "Reset size".into(),
        controls: "Controls".into(),
        control_left_click: "Left-click: details".into(),
        control_right_click: "Right-click: menu".into(),
        control_drag: "Drag: move/snap".into(),
        control_ctrl_wheel: "Ctrl+Wheel: resize".into(),
        control_tray_click: "Tray click: show/hide".into(),
        control_tray_click_linux: "Tray click: menu".into(),
        tray_left_click: "Left-click: show/hide".into(),
        check_for_updates: "Check for updates".into(),
        checking_for_updates: "Checking for updates...".into(),
        up_to_date: "Up to date".into(),
        update_failed: "Update failed".into(),
        applying_update: "Applying update...".into(),
        update_available: "Update available".into(),
        update_via_winget: "via WinGet".into(),
        auto_update_check: "Auto-update check".into(),
        auto_check_disabled: "Disabled".into(),
        auto_check_hourly: "Hourly".into(),
        auto_check_daily: "Daily".into(),
        auto_check_weekly: "Weekly".into(),
        exit: "Exit".into(),
        restart: "Restart".into(),
        show_widget: "Show widget".into(),
        click_through: "Click-through".into(),
        session_window: "5h".into(),
        weekly_window: "7d".into(),
        now: "now".into(),
        day_suffix: "d".into(),
        hour_suffix: "h".into(),
        minute_suffix: "m".into(),
        second_suffix: "s".into(),
        token_expired_title: "Claude Code session expired".into(),
        token_expired_body: "Sign in again to keep tracking your usage.".into(),
        chatgpt_token_expired_title: "Codex session expired".into(),
        chatgpt_token_expired_body: "Sign in again to keep tracking your usage.".into(),
        threshold_80_body: "Approaching the 5-hour limit.".into(),
        threshold_95_body: "Limit is close - consider easing up.".into(),
        update_applied_title: "Update applied".into(),
        update_applied_body: "Updated to v".into(),
        update_rollback_failed_body: "Update failed. Your original binary is saved at: ".into(),
    }
}

// ---------- Free-function helpers ----------

/// A percentage rounded half-to-even (`"72%"`), or `"—"` when there is none.
pub fn format_pct(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{p:.0}%"),
        None => String::from("\u{2014}"),
    }
}

/// Format a `usage::Window` percentage + countdown as `"73% · 2h"`-style text.
/// Returns just the percentage when no reset time is available.
pub fn format_window(window: &Window, strings: &LocaleStrings) -> String {
    format_window_at(window, strings, SystemTime::now())
}

/// [`format_window`] evaluated at `now`.
pub fn format_window_at(window: &Window, strings: &LocaleStrings, now: SystemTime) -> String {
    let pct = format!("{:.0}%", window.utilization);
    let cd = format_countdown_at(window.resets_at, strings, now);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} \u{00b7} {cd}")
    }
}

/// Countdown only — used by the bubble, which renders the percent inside the
/// bar fill and only needs the time-to-reset on the right.
pub fn format_countdown(resets_at: Option<SystemTime>, strings: &LocaleStrings) -> String {
    format_countdown_at(resets_at, strings, SystemTime::now())
}

/// [`format_countdown`] evaluated at `now`.
pub fn format_countdown_at(
    resets_at: Option<SystemTime>,
    strings: &LocaleStrings,
    now: SystemTime,
) -> String {
    let Some(reset) = resets_at else {
        return String::new();
    };
    let remaining = match reset.duration_since(now) {
        Ok(d) => d,
        Err(_) => return strings.now.clone(),
    };
    format_countdown_secs(remaining.as_secs(), strings)
}

fn format_countdown_secs(total_secs: u64, strings: &LocaleStrings) -> String {
    let days = total_secs / 86_400;
    let hours = total_secs / 3_600;
    let mins = total_secs / 60;
    if days >= 1 {
        format!("{days}{}", strings.day_suffix)
    } else if hours >= 1 {
        format!("{hours}{}", strings.hour_suffix)
    } else if mins >= 1 {
        format!("{mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    }
}

/// How long before `format_window`'s string would change.
/// Used by the countdown timer to refresh exactly when needed.
pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    time_until_display_change_at(resets_at, SystemTime::now())
}

/// [`time_until_display_change`] evaluated at `now`.
pub fn time_until_display_change_at(
    resets_at: Option<SystemTime>,
    now: SystemTime,
) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(now).ok()?;
    let secs = remaining.as_secs();
    let bucket_start = if secs / 86_400 >= 1 {
        (secs / 86_400) * 86_400
    } else if secs / 3_600 >= 1 {
        (secs / 3_600) * 3_600
    } else if secs / 60 >= 1 {
        (secs / 60) * 60
    } else {
        secs
    };
    Some(Duration::from_secs(secs.saturating_sub(bucket_start) + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_strings_include_required_menu_labels() {
        let strings = english_strings();
        for (name, value) in [
            ("refresh", strings.refresh.as_str()),
            ("providers", strings.providers.as_str()),
            ("settings", strings.settings.as_str()),
            ("size_smaller", strings.size_smaller.as_str()),
            ("size_larger", strings.size_larger.as_str()),
            ("reset_size", strings.reset_size.as_str()),
            ("controls", strings.controls.as_str()),
            ("control_left_click", strings.control_left_click.as_str()),
            ("control_right_click", strings.control_right_click.as_str()),
            ("control_drag", strings.control_drag.as_str()),
            ("control_ctrl_wheel", strings.control_ctrl_wheel.as_str()),
            ("control_tray_click", strings.control_tray_click.as_str()),
            (
                "control_tray_click_linux",
                strings.control_tray_click_linux.as_str(),
            ),
            ("tray_left_click", strings.tray_left_click.as_str()),
        ] {
            assert!(!value.trim().is_empty(), "empty string: {name}");
        }
    }

    fn at(now: SystemTime, secs: u64) -> Option<SystemTime> {
        Some(now + Duration::from_secs(secs))
    }

    #[test]
    fn countdown_buckets() {
        let s = english_strings();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let cd = |secs| format_countdown_at(at(now, secs), &s, now);
        assert_eq!(format_countdown_at(None, &s, now), "");
        assert_eq!(
            format_countdown_at(Some(now - Duration::from_secs(1)), &s, now),
            "now"
        );
        assert_eq!(cd(0), "0s");
        assert_eq!(cd(59), "59s");
        assert_eq!(cd(60), "1m");
        assert_eq!(cd(3_599), "59m");
        assert_eq!(cd(3_600), "1h");
        assert_eq!(cd(86_399), "23h");
        assert_eq!(cd(86_400), "1d");
        assert_eq!(cd(86_400 + 23 * 3_600), "1d");
        assert_eq!(cd(7 * 86_400), "7d");
    }

    #[test]
    fn window_text_joins_percent_and_countdown() {
        let s = english_strings();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let w = Window {
            utilization: 73.2,
            resets_at: at(now, 2 * 3_600 + 5),
        };
        assert_eq!(format_window_at(&w, &s, now), "73% \u{00b7} 2h");
        let bare = Window {
            utilization: 73.2,
            resets_at: None,
        };
        assert_eq!(format_window_at(&bare, &s, now), "73%");
    }

    #[test]
    fn display_change_fires_just_past_the_bucket_edge() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let ttl = |secs| time_until_display_change_at(at(now, secs), now);
        assert_eq!(time_until_display_change_at(None, now), None);
        assert_eq!(
            time_until_display_change_at(Some(now - Duration::from_secs(5)), now),
            None
        );
        // 1d 23h: "1d" stays until 23h + 1s from now.
        assert_eq!(
            ttl(86_400 + 23 * 3_600),
            Some(Duration::from_secs(23 * 3_600 + 1))
        );
        assert_eq!(ttl(2 * 3_600 + 30), Some(Duration::from_secs(31)));
        assert_eq!(ttl(5 * 60 + 7), Some(Duration::from_secs(8)));
        // Under a minute the text changes every second.
        assert_eq!(ttl(42), Some(Duration::from_secs(1)));
    }

    #[test]
    fn percent_rounds_half_to_even() {
        assert_eq!(format_pct(Some(72.5)), "72%");
        assert_eq!(format_pct(Some(73.5)), "74%");
        assert_eq!(format_pct(Some(0.0)), "0%");
        assert_eq!(format_pct(None), "\u{2014}");
    }

    #[test]
    fn autostart_label_matches_the_platform() {
        let s = english_strings();
        let expected = if cfg!(windows) {
            "Start with Windows"
        } else {
            "Start at login"
        };
        assert_eq!(s.autostart_label(), expected);
        assert_eq!(s.provider_label(ProviderId::ChatGpt), "Codex");
    }

    #[test]
    fn tray_click_hint_matches_the_platform() {
        let s = english_strings();
        let expected = if cfg!(target_os = "linux") {
            "Tray click: menu"
        } else {
            "Tray click: show/hide"
        };
        assert_eq!(s.tray_click_hint(), expected);
    }
}
