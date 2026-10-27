// Persisted settings. The schema, key names and physical-px positions stay
// byte-compatible with the Win32 builds.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::geom::{
    position_on_any_monitor, Rect, DEFAULT_BUBBLE_SIZE, MAX_BUBBLE_SIZE, MIN_BUBBLE_SIZE,
};
use crate::update::channel::Channel;
use crate::usage::ProviderId;

pub const APP_DIR_NAME: &str = "UsageMonitor";
const SETTINGS_FILE: &str = "settings.json";

pub const POLL_1_MIN: u32 = 60_000;
pub const POLL_5_MIN: u32 = 5 * 60_000;
pub const POLL_15_MIN: u32 = 15 * 60_000;
pub const POLL_1_HOUR: u32 = 60 * 60_000;
/// The poll intervals the menu offers, in menu order.
pub const POLL_INTERVALS: [u32; 4] = [POLL_1_MIN, POLL_5_MIN, POLL_15_MIN, POLL_1_HOUR];

// Update-check intervals (seconds). `None` means auto-check is disabled.
pub const UPDATE_CHECK_HOURLY_SECS: u64 = 60 * 60;
pub const UPDATE_CHECK_DAILY_SECS: u64 = 24 * 60 * 60;
pub const UPDATE_CHECK_WEEKLY_SECS: u64 = 7 * 24 * 60 * 60;
/// The auto-update check choices the menu offers, in menu order.
pub const UPDATE_CHECK_INTERVALS: [Option<u64>; 4] = [
    None,
    Some(UPDATE_CHECK_HOURLY_SECS),
    Some(UPDATE_CHECK_DAILY_SECS),
    Some(UPDATE_CHECK_WEEKLY_SECS),
];

fn default_show_claude() -> bool {
    true
}
fn default_show_codex() -> bool {
    false
}
fn default_widget_visible() -> bool {
    true
}
fn default_bubble_size() -> i32 {
    DEFAULT_BUBBLE_SIZE
}
fn default_poll_interval_ms() -> u32 {
    POLL_5_MIN
}
fn default_update_check_interval_secs() -> Option<u64> {
    Some(UPDATE_CHECK_HOURLY_SECS)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BubblePositions {
    pub claude: Option<(i32, i32)>,
    pub codex: Option<(i32, i32)>,
}

impl BubblePositions {
    pub fn get(&self, provider: ProviderId) -> Option<(i32, i32)> {
        match provider {
            ProviderId::Claude => self.claude,
            ProviderId::ChatGpt => self.codex,
        }
    }
    pub fn set(&mut self, provider: ProviderId, pos: (i32, i32)) {
        match provider {
            ProviderId::Claude => self.claude = Some(pos),
            ProviderId::ChatGpt => self.codex = Some(pos),
        }
    }
    pub fn reset(&mut self, provider: ProviderId) {
        match provider {
            ProviderId::Claude => self.claude = None,
            ProviderId::ChatGpt => self.codex = None,
        }
    }
    pub fn reset_all(&mut self) {
        self.claude = None;
        self.codex = None;
    }

    /// Drop any saved position whose top-left no longer falls on one of
    /// `monitors`. Guards against creating a bubble on a disconnected
    /// secondary monitor, where the user can't see or recover it.
    pub fn validate(&mut self, monitors: &[Rect]) {
        for provider in ProviderId::ALL {
            if let Some((x, y)) = self.get(provider) {
                if !position_on_any_monitor(x, y, monitors) {
                    log::warn!(
                        "bubble position {} ({x},{y}) outside all monitors; resetting to default",
                        provider.slug()
                    );
                    self.reset(provider);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_show_claude")]
    pub show_claude_code: bool,
    #[serde(default = "default_show_codex")]
    pub show_codex: bool,
    #[serde(default)]
    pub bubble_positions: BubblePositions,
    #[serde(default = "default_bubble_size")]
    pub bubble_size_logical: i32,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u32,
    #[serde(default)]
    pub last_update_check_unix: Option<u64>,
    #[serde(default = "default_update_check_interval_secs")]
    pub update_check_interval_secs: Option<u64>,
    #[serde(default = "default_widget_visible")]
    pub widget_visible: bool,
    /// The bubbles ignore the mouse, so clicks reach the windows below.
    #[serde(default)]
    pub click_through: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_claude_code: default_show_claude(),
            show_codex: default_show_codex(),
            bubble_positions: BubblePositions::default(),
            bubble_size_logical: default_bubble_size(),
            poll_interval_ms: default_poll_interval_ms(),
            last_update_check_unix: None,
            update_check_interval_secs: default_update_check_interval_secs(),
            widget_visible: default_widget_visible(),
            click_through: false,
        }
    }
}

impl Settings {
    pub fn is_provider_enabled(&self, provider: ProviderId) -> bool {
        match provider {
            ProviderId::Claude => self.show_claude_code,
            ProviderId::ChatGpt => self.show_codex,
        }
    }

    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        match provider {
            ProviderId::Claude => self.show_claude_code = enabled,
            ProviderId::ChatGpt => self.show_codex = enabled,
        }
    }

    /// Menu toggle for a provider. Unchecking the last enabled provider is
    /// undone so the app always shows something. Returns the provider's new
    /// state.
    pub fn toggle_provider(&mut self, provider: ProviderId) -> bool {
        let enabled = !self.is_provider_enabled(provider);
        self.set_provider_enabled(provider, enabled);
        if !self.has_enabled_provider() {
            self.set_provider_enabled(provider, true);
        }
        self.is_provider_enabled(provider)
    }

    pub fn has_enabled_provider(&self) -> bool {
        ProviderId::ALL
            .iter()
            .any(|provider| self.is_provider_enabled(*provider))
    }
}

/// The folder holding `settings.json`. The Windows exe and the AppImage are
/// portable, so their settings sit next to the file and move with it. A
/// `.deb` install lives in a read-only system folder and keeps them in the
/// user's config folder.
pub fn settings_dir() -> Option<PathBuf> {
    portable_dir(
        crate::update::channel::current(),
        std::env::var_os("APPIMAGE"),
        std::env::current_exe().ok(),
    )
    .or_else(|| dirs::config_dir().map(|d| d.join(APP_DIR_NAME)))
}

/// The folder of the portable file for `channel`: the exe on Windows, the
/// AppImage file (not its temporary mount) on Linux. `None` for installs
/// that are not portable.
fn portable_dir(
    channel: Channel,
    appimage: Option<OsString>,
    exe: Option<PathBuf>,
) -> Option<PathBuf> {
    let file = match channel {
        Channel::Portable => exe?,
        Channel::AppImage => PathBuf::from(appimage?),
        Channel::PackageManager | Channel::Unsupported => return None,
    };
    file.parent().map(Path::to_path_buf)
}

pub fn settings_path() -> PathBuf {
    settings_dir()
        .unwrap_or_else(|| std::env::temp_dir().join(APP_DIR_NAME))
        .join(SETTINGS_FILE)
}

/// Read and normalise the settings file. A missing, unreadable or corrupt
/// file yields the defaults. Saved positions are not checked against the
/// monitors here; call [`drop_offscreen_positions`] once they are known.
pub fn load() -> Settings {
    load_from(&settings_path())
}

/// [`load`] from an explicit path. A file that is not a JSON object at all
/// is copied to `settings.json.bak` first, since the next save replaces it.
pub fn load_from(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed = parse(&content);
            if parsed.is_none() {
                back_up_unreadable(path);
            }
            normalise(parsed.unwrap_or_default())
        }
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                log::warn!("settings read failed ({}): {err}", path.display());
            }
            Settings::default()
        }
    }
}

/// Keep a copy of a settings file that could not be read at all, so the
/// user can recover it by hand.
fn back_up_unreadable(path: &Path) {
    let backup = path.with_extension("json.bak");
    match std::fs::copy(path, &backup) {
        Ok(_) => log::warn!(
            "settings file is not valid JSON; kept a copy at {}",
            backup.display()
        ),
        Err(err) => log::warn!(
            "settings file is not valid JSON and could not be copied to {}: {err}",
            backup.display()
        ),
    }
}

/// Parse settings JSON, keeping every readable key, then enforce the
/// invariants a hand-edited file may break. A file that is not a JSON
/// object yields the defaults.
pub fn parse_and_normalise(content: &str) -> Settings {
    normalise(parse(content).unwrap_or_default())
}

/// Parse settings JSON. A value of the wrong type for one key costs only
/// that key, which falls back to its default; `None` when the content is
/// not a JSON object.
fn parse(content: &str) -> Option<Settings> {
    let err = match serde_json::from_str::<Settings>(content) {
        Ok(settings) => return Some(settings),
        Err(err) => err,
    };
    let Ok(serde_json::Value::Object(file)) = serde_json::from_str(content) else {
        log::warn!("settings file is not valid; using defaults: {err}");
        return None;
    };
    log::warn!("settings file has invalid values; keeping the valid keys: {err}");
    let serde_json::Value::Object(mut merged) = serde_json::to_value(Settings::default()).ok()?
    else {
        return None;
    };
    for (key, value) in file {
        // Unknown keys are ignored, as the strict parse does.
        if !merged.contains_key(&key) {
            continue;
        }
        let mut trial = merged.clone();
        trial.insert(key.clone(), value.clone());
        match serde_json::from_value::<Settings>(serde_json::Value::Object(trial)) {
            Ok(_) => {
                merged.insert(key, value);
            }
            Err(err) => log::warn!("settings key {key:?} ignored: {err}"),
        }
    }
    serde_json::from_value(serde_json::Value::Object(merged)).ok()
}

/// Enforce the invariants a hand-edited file may break.
fn normalise(mut settings: Settings) -> Settings {
    // At least one provider must be visible. Otherwise the app has nothing to show.
    if !settings.has_enabled_provider() {
        settings.show_claude_code = true;
    }
    // Clamp bubble size to safe range in case settings.json was hand-edited.
    settings.bubble_size_logical = settings
        .bubble_size_logical
        .clamp(MIN_BUBBLE_SIZE, MAX_BUBBLE_SIZE);
    settings
}

/// Drop positions on monitors that have since been disconnected.
/// `monitors` are the full monitor rects in physical px. An empty list means
/// the monitors could not be enumerated, so every position is kept rather
/// than all of them being thrown away.
pub fn drop_offscreen_positions(settings: &mut Settings, monitors: &[Rect]) {
    if monitors.is_empty() {
        log::warn!("no monitors reported; keeping saved bubble positions");
        return;
    }
    settings.bubble_positions.validate(monitors);
}

/// Persist `settings` with an atomic write (tmp file, then rename). Falls
/// back to a direct write when the rename fails.
pub fn save(settings: &Settings) -> std::io::Result<()> {
    save_to(&settings_path(), settings)
}

/// [`save`] to an explicit path.
pub fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    let tmp_path = path.with_extension("json.tmp");
    let atomic = std::fs::write(&tmp_path, &json).and_then(|()| std::fs::rename(&tmp_path, path));
    match atomic {
        Ok(()) => Ok(()),
        Err(err) => {
            log::warn!("atomic settings save failed, writing directly: {err}");
            let _ = std::fs::remove_file(&tmp_path);
            std::fs::write(path, json)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_installs_keep_settings_next_to_the_file() {
        let folder = Path::new("Tools").join("Usage");
        assert_eq!(
            portable_dir(
                Channel::Portable,
                None,
                Some(folder.join("usage-monitor.exe"))
            ),
            Some(folder)
        );
        // The AppImage file's folder, not the mount the exe runs from.
        assert_eq!(
            portable_dir(
                Channel::AppImage,
                Some(OsString::from("/home/u/Apps/usage-monitor.AppImage")),
                Some(PathBuf::from("/tmp/.mount_usage/usr/bin/usage-monitor")),
            ),
            Some(PathBuf::from("/home/u/Apps"))
        );
        assert_eq!(
            portable_dir(
                Channel::PackageManager,
                None,
                Some(PathBuf::from("/usr/bin/usage-monitor"))
            ),
            None
        );
        assert_eq!(portable_dir(Channel::Unsupported, None, None), None);
    }

    #[test]
    fn old_two_provider_settings_json_still_loads() {
        let json = r#"{
            "show_claude_code": false,
            "show_codex": true,
            "bubble_positions": {
                "claude": [10, 20],
                "codex": [30, 40]
            }
        }"#;

        let settings: Settings =
            serde_json::from_str(json).expect("old settings json should parse");

        assert!(!settings.show_claude_code);
        assert!(settings.show_codex);
        assert_eq!(
            settings.bubble_positions.get(ProviderId::Claude),
            Some((10, 20))
        );
        assert_eq!(
            settings.bubble_positions.get(ProviderId::ChatGpt),
            Some((30, 40))
        );
    }

    #[test]
    fn retired_opencode_go_keys_are_ignored() {
        let json = r#"{
            "show_claude_code": false,
            "show_codex": true,
            "show_opencode_go": true,
            "bubble_positions": {
                "codex": [30, 40],
                "opencode_go": [50, 60]
            }
        }"#;

        let settings: Settings =
            serde_json::from_str(json).expect("settings with retired keys should parse");

        assert!(!settings.show_claude_code);
        assert!(settings.show_codex);
        assert_eq!(
            settings.bubble_positions.get(ProviderId::ChatGpt),
            Some((30, 40))
        );
        let keys = serde_json::to_value(&settings).expect("serialise");
        assert!(keys.get("show_opencode_go").is_none());
        assert!(keys["bubble_positions"].get("opencode_go").is_none());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let settings = parse_and_normalise("{ not json");
        assert!(settings.show_claude_code);
        assert!(!settings.show_codex);
        assert_eq!(settings.bubble_size_logical, DEFAULT_BUBBLE_SIZE);
        assert_eq!(settings.poll_interval_ms, POLL_5_MIN);
        assert_eq!(
            settings.update_check_interval_secs,
            Some(UPDATE_CHECK_HOURLY_SECS)
        );
        assert!(settings.widget_visible);
    }

    #[test]
    fn one_wrongly_typed_key_keeps_the_others() {
        let settings = parse_and_normalise(
            r#"{"bubble_size_logical":250.5,"show_codex":true,"poll_interval_ms":60000}"#,
        );
        assert_eq!(settings.bubble_size_logical, DEFAULT_BUBBLE_SIZE);
        assert!(settings.show_codex);
        assert_eq!(settings.poll_interval_ms, POLL_1_MIN);
        // Keys missing from the file keep their defaults.
        assert!(settings.show_claude_code);
        assert_eq!(
            settings.update_check_interval_secs,
            Some(UPDATE_CHECK_HOURLY_SECS)
        );

        let settings = parse_and_normalise(
            r#"{"show_codex":"yes","widget_visible":false,"last_update_check_unix":5,
                "bubble_positions":{"claude":[1,2]},"unknown":1}"#,
        );
        assert!(!settings.show_codex);
        assert!(!settings.widget_visible);
        assert_eq!(settings.last_update_check_unix, Some(5));
        assert_eq!(
            settings.bubble_positions.get(ProviderId::Claude),
            Some((1, 2))
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccub-settings-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ))
    }

    #[test]
    fn an_unreadable_file_is_backed_up_before_it_can_be_replaced() {
        let dir = temp_dir("backup");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(SETTINGS_FILE);
        let backup = path.with_extension("json.bak");
        std::fs::write(&path, "{ not json").expect("write");
        assert!(load_from(&path).show_claude_code);
        assert_eq!(
            std::fs::read_to_string(&backup).expect("backup"),
            "{ not json"
        );

        // A file with only a bad value is salvaged, not backed up.
        std::fs::remove_file(&backup).expect("remove backup");
        std::fs::write(&path, r#"{"show_codex":true,"poll_interval_ms":"x"}"#).expect("write");
        assert!(load_from(&path).show_codex);
        assert!(!backup.exists());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn normalise_restores_a_provider_and_clamps_size() {
        let settings = parse_and_normalise(
            r#"{"show_claude_code": false, "show_codex": false, "bubble_size_logical": 9000}"#,
        );
        assert!(settings.show_claude_code);
        assert_eq!(settings.bubble_size_logical, MAX_BUBBLE_SIZE);
        let small = parse_and_normalise(r#"{"bubble_size_logical": 10}"#);
        assert_eq!(small.bubble_size_logical, MIN_BUBBLE_SIZE);
    }

    #[test]
    fn serialised_keys_stay_compatible() {
        let value = serde_json::to_value(Settings::default()).expect("serialise");
        let mut keys: Vec<_> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "bubble_positions",
                "bubble_size_logical",
                "click_through",
                "last_update_check_unix",
                "poll_interval_ms",
                "show_claude_code",
                "show_codex",
                "update_check_interval_secs",
                "widget_visible",
            ]
        );
        let mut s = Settings::default();
        s.bubble_positions.set(ProviderId::ChatGpt, (-1900, 40));
        let json = serde_json::to_string(&s).expect("serialise");
        assert!(json.contains(r#""codex":[-1900,40]"#), "{json}");
    }

    #[test]
    fn offscreen_positions_are_dropped_against_monitors() {
        let mut settings = parse_and_normalise(
            r#"{"bubble_positions": {"claude": [100, 100], "codex": [-3000, 100]}}"#,
        );
        let monitors = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 3840, 1080)];
        drop_offscreen_positions(&mut settings, &monitors);
        let p = &settings.bubble_positions;
        assert_eq!(p.get(ProviderId::Claude), Some((100, 100)));
        assert_eq!(p.get(ProviderId::ChatGpt), None);
    }

    #[test]
    fn no_monitors_keeps_saved_positions() {
        let mut settings = Settings::default();
        settings
            .bubble_positions
            .set(ProviderId::Claude, (-3000, 100));
        drop_offscreen_positions(&mut settings, &[]);
        assert_eq!(
            settings.bubble_positions.get(ProviderId::Claude),
            Some((-3000, 100))
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "ccub-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let path = dir.join(SETTINGS_FILE);
        let mut settings = Settings {
            show_codex: true,
            poll_interval_ms: POLL_15_MIN,
            update_check_interval_secs: None,
            last_update_check_unix: Some(1_700_000_000),
            ..Settings::default()
        };
        settings.bubble_positions.set(ProviderId::Claude, (12, 34));
        save_to(&path, &settings).expect("save");
        assert!(!path.with_extension("json.tmp").exists());
        let loaded = load_from(&path);
        assert!(loaded.show_codex);
        assert_eq!(loaded.poll_interval_ms, POLL_15_MIN);
        assert_eq!(loaded.update_check_interval_secs, None);
        assert_eq!(loaded.last_update_check_unix, Some(1_700_000_000));
        assert_eq!(
            loaded.bubble_positions.get(ProviderId::Claude),
            Some((12, 34))
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
        // A missing file loads the defaults.
        assert!(load_from(&path).show_claude_code);
    }

    #[test]
    fn toggle_never_disables_the_last_provider() {
        let mut settings = Settings::default();
        assert!(settings.toggle_provider(ProviderId::ChatGpt));
        assert!(!settings.toggle_provider(ProviderId::Claude));
        assert!(settings.has_enabled_provider());
        // ChatGPT is now the only enabled provider, so it stays on.
        assert!(settings.toggle_provider(ProviderId::ChatGpt));
        assert!(settings.show_codex);
        assert!(settings.toggle_provider(ProviderId::Claude));
        assert!(!settings.toggle_provider(ProviderId::ChatGpt));
    }
}
