//! Commands the webviews invoke. Every argument comes from a webview, so
//! each one is validated here before it reaches the runtime.

use std::sync::Arc;

use tauri::{AppHandle, State, WebviewWindow};

use crate::bubble;
use crate::core::geom::RESIZE_STEP_LOGICAL;
use crate::core::i18n::LocaleStrings;
use crate::core::payload::{BubblePayload, PanelPayload};
use crate::panel;
use crate::runtime::Runtime;
use crate::usage::ProviderId;

/// Longest JS log message kept, in characters.
const MAX_JS_LOG_CHARS: usize = 2_000;

/// The current payload for `provider`'s bubble, sized for the calling
/// window's scale factor.
#[tauri::command]
pub fn get_bubble_state(
    window: WebviewWindow,
    runtime: State<'_, Arc<Runtime>>,
    provider: String,
) -> Result<BubblePayload, String> {
    let provider = parse_provider(&provider)?;
    let scale = window.scale_factor().map_err(|err| err.to_string())?;
    Ok(runtime.bubble_payload(provider, scale))
}

/// The details-panel payload for the provider the panel last showed;
/// `None` before the first bubble click.
#[tauri::command]
pub fn get_panel_state(runtime: State<'_, Arc<Runtime>>) -> Option<PanelPayload> {
    panel::current_provider().map(|provider| runtime.panel_payload(provider))
}

/// Every UI string, camelCase keys.
#[tauri::command]
pub fn get_strings(runtime: State<'_, Arc<Runtime>>) -> LocaleStrings {
    runtime.with_state(|s| s.strings.clone())
}

/// Forward a webview error or message to the diagnostic log.
#[tauri::command]
pub fn log_js(window: WebviewWindow, level: String, message: String) {
    let message = sanitize_log_message(&message);
    let label = window.label();
    match level.as_str() {
        "error" => log::error!(target: "js", "[{label}] {message}"),
        "warn" => log::warn!(target: "js", "[{label}] {message}"),
        "debug" => log::debug!(target: "js", "[{label}] {message}"),
        _ => log::info!(target: "js", "[{label}] {message}"),
    }
}

/// The bubble rendered its first state and can be shown (unless hidden).
#[tauri::command]
pub fn bubble_ready(app: AppHandle, label: String) -> Result<(), String> {
    bubble::mark_ready(&app, &label)
}

/// A left click (a press released before the drag threshold) on a bubble.
/// Async, so the panel toggle is queued to the main thread instead of
/// running inside the webview's IPC callback.
#[tauri::command]
pub async fn bubble_click(runtime: State<'_, Arc<Runtime>>, slug: String) -> Result<(), String> {
    let provider = parse_provider(&slug)?;
    runtime.bubble_clicked(provider);
    Ok(())
}

/// The page is about to start an OS drag of a bubble.
#[tauri::command]
pub fn bubble_drag_start(app: AppHandle, slug: String) -> Result<(), String> {
    let provider = parse_provider(&slug)?;
    bubble::drag_start(&app, provider);
    Ok(())
}

/// Ctrl+wheel on a bubble: `dir` 1 grows every bubble one step, -1 shrinks.
#[tauri::command]
pub fn bubble_resize_step(runtime: State<'_, Arc<Runtime>>, dir: i32) -> Result<(), String> {
    let delta = resize_delta(dir)?;
    runtime.resize_bubbles(delta);
    Ok(())
}

/// The size change for a resize direction; only 1 and -1 are accepted.
fn resize_delta(dir: i32) -> Result<i32, String> {
    match dir {
        1 => Ok(RESIZE_STEP_LOGICAL),
        -1 => Ok(-RESIZE_STEP_LOGICAL),
        _ => Err("resize direction must be 1 or -1".into()),
    }
}

fn parse_provider(slug: &str) -> Result<ProviderId, String> {
    ProviderId::from_slug(slug).ok_or_else(|| "unknown provider".to_string())
}

/// One log line: control characters become spaces, so a message cannot
/// forge extra log lines, and the length is capped.
fn sanitize_log_message(message: &str) -> String {
    message
        .chars()
        .take(MAX_JS_LOG_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_providers_are_accepted() {
        assert_eq!(parse_provider("claude"), Ok(ProviderId::Claude));
        assert_eq!(parse_provider("chatgpt"), Ok(ProviderId::ChatGpt));
        assert!(parse_provider("codex").is_err());
        assert!(parse_provider("").is_err());
        assert!(parse_provider("CLAUDE").is_err());
    }

    #[test]
    fn resize_steps_are_one_step_either_way() {
        assert_eq!(resize_delta(1), Ok(20));
        assert_eq!(resize_delta(-1), Ok(-20));
        assert!(resize_delta(0).is_err());
        assert!(resize_delta(2).is_err());
        assert!(resize_delta(i32::MIN).is_err());
    }

    #[test]
    fn log_messages_stay_on_one_capped_line() {
        assert_eq!(sanitize_log_message("a\nb\r\tc"), "a b  c");
        let long = "x".repeat(MAX_JS_LOG_CHARS + 50);
        assert_eq!(
            sanitize_log_message(&long).chars().count(),
            MAX_JS_LOG_CHARS
        );
    }
}
