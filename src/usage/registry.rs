// Provider registry: holds every enabled provider and dispatches polls.
//
// The app owns one `Registry` and calls `poll_enabled(http, settings)` on
// every cycle. The result is a flat list of `(id, Result<UsageWindows>)`
// pairs the app can apply to its state.

use crate::core::settings::Settings;
use crate::creds::Locator;
use crate::net::Client;
use crate::usage::{
    anthropic::ClaudeProvider, chatgpt::ChatGptProvider, refresh, Error, ProviderId, UsageProvider,
    UsageWindows,
};

pub struct Registry {
    claude: ClaudeProvider,
    chatgpt: ChatGptProvider,
}

impl Registry {
    pub fn with_defaults() -> Self {
        Self {
            claude: ClaudeProvider::new(Locator::for_claude()),
            chatgpt: ChatGptProvider::new(Locator::for_chatgpt()),
        }
    }

    pub fn poll_enabled(
        &mut self,
        http: &Client,
        settings: &Settings,
    ) -> Vec<(ProviderId, Result<UsageWindows, Error>)> {
        let mut out = Vec::new();
        for provider in ProviderId::ALL {
            if !settings.is_provider_enabled(provider) {
                continue;
            }
            let result = match provider {
                ProviderId::Claude => self.claude.poll(http),
                ProviderId::ChatGpt => self.chatgpt.poll(http),
            };
            out.push((provider, result));
        }
        out
    }

    /// Attempt to refresh the active source for one provider.
    pub fn try_refresh(
        &self,
        id: ProviderId,
        orchestrator: &refresh::Orchestrator,
    ) -> refresh::Outcome {
        let locator = match id {
            ProviderId::Claude => self.claude.locator(),
            ProviderId::ChatGpt => self.chatgpt.locator(),
        };
        match locator.first_available() {
            Some(src) => orchestrator.refresh(src),
            None => refresh::Outcome::CliMissing,
        }
    }

    /// Snapshot of credential-file fingerprints across both providers —
    /// used to detect external re-authentication between poll cycles.
    pub fn credential_signatures(&self) -> Vec<String> {
        let mut sigs = self.claude.locator().signatures();
        sigs.extend(self.chatgpt.locator().signatures());
        sigs.sort();
        sigs.dedup();
        sigs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::USER_AGENT;

    /// Polls the real providers with the credentials on this machine. Run by
    /// hand: `cargo test -- --ignored live_poll --nocapture`.
    #[test]
    #[ignore = "talks to the real provider endpoints"]
    fn live_poll() {
        let http = Client::new(USER_AGENT).expect("client");
        let mut settings = Settings::default();
        for provider in ProviderId::ALL {
            settings.set_provider_enabled(provider, true);
        }
        let mut registry = Registry::with_defaults();
        let results = registry.poll_enabled(&http, &settings);
        assert_eq!(results.len(), ProviderId::ALL.len());
        for (provider, result) in results {
            match result {
                Ok(w) => println!(
                    "{}: 5h {:.1}% resets {:?}, weekly {:.1}% resets {:?}",
                    provider.slug(),
                    w.primary.utilization,
                    w.primary.resets_at,
                    w.secondary.utilization,
                    w.secondary.resets_at
                ),
                Err(err) => println!("{}: {err}", provider.slug()),
            }
        }
    }
}
