// Usage data shapes shared across providers.

use std::time::SystemTime;

use serde::Serialize;

/// Serialises as its [`slug`](ProviderId::slug), which also names the
/// provider's bubble window and tray icon.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize)]
pub enum ProviderId {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "chatgpt")]
    ChatGpt,
}

impl ProviderId {
    pub const ALL: [Self; 2] = [Self::Claude, Self::ChatGpt];

    pub fn slug(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ChatGpt => "chatgpt",
        }
    }

    /// Inverse of [`slug`](ProviderId::slug).
    pub fn from_slug(slug: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.slug() == slug)
    }
}

/// One usage window: how much you've consumed (0–100), and when it resets.
#[derive(Clone, Copy, Debug, Default)]
pub struct Window {
    pub utilization: f64,
    pub resets_at: Option<SystemTime>,
}

/// The pair of windows a provider reports per poll.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsageWindows {
    pub primary: Window,
    pub secondary: Window,
}

/// One provider's most recent poll result, keyed by `id`.
#[derive(Clone, Debug)]
pub struct ProviderSnapshot {
    pub id: ProviderId,
    pub windows: UsageWindows,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_as_slug_and_parses_back() {
        for provider in ProviderId::ALL {
            let json = serde_json::to_string(&provider).expect("serialise");
            assert_eq!(json, format!("\"{}\"", provider.slug()));
            assert_eq!(ProviderId::from_slug(provider.slug()), Some(provider));
        }
        assert_eq!(ProviderId::from_slug("codex"), None);
    }
}
