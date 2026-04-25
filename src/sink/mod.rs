pub mod discord;
pub mod slack;

use async_trait::async_trait;

use crate::Result;
use crate::events::MessageFormat;
use serde_json::Value;

pub use discord::DiscordSink;
pub use slack::SlackSink;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SinkTarget {
    DiscordChannel(String),
    DiscordWebhook(String),
    SlackWebhook(String),
}

impl SinkTarget {
    /// Human-readable label safe for logs, provenance, status output, and DLQ metadata.
    ///
    /// Webhook URLs contain credentials, so this method never returns the raw URL.
    #[must_use]
    pub fn safe_label(&self) -> String {
        match self {
            Self::DiscordChannel(channel) => format!("DiscordChannel({channel:?})"),
            Self::DiscordWebhook(_) => {
                format!("DiscordWebhook(redacted:{})", self.stable_fingerprint())
            }
            Self::SlackWebhook(_) => format!("SlackWebhook(redacted:{})", self.stable_fingerprint()),
        }
    }

    /// Stable non-secret target fingerprint for batching, logs, status, and DLQ keys.
    #[must_use]
    pub fn stable_fingerprint(&self) -> String {
        match self {
            Self::DiscordChannel(channel) => format!("discord-channel:{}", fnv1a_64_hex(channel)),
            Self::DiscordWebhook(webhook) => format!("discord-webhook:{}", fnv1a_64_hex(webhook)),
            Self::SlackWebhook(webhook) => format!("slack-webhook:{}", fnv1a_64_hex(webhook)),
        }
    }
}

fn fnv1a_64_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkMessage {
    pub event_kind: String,
    pub format: MessageFormat,
    pub content: String,
    pub payload: Value,
}

#[async_trait]
pub trait Sink: Send + Sync {
    async fn send(&self, target: &SinkTarget, message: &SinkMessage) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_label_redacts_discord_webhook_url() {
        let target = SinkTarget::DiscordWebhook(
            "https://discord.com/api/webhooks/123456789/super-secret".into(),
        );
        let label = target.safe_label();

        assert!(label.starts_with("DiscordWebhook(redacted:discord-webhook:"));
        assert!(!label.contains("discord.com/api/webhooks"));
        assert!(!label.contains("super-secret"));
    }

    #[test]
    fn safe_label_redacts_slack_webhook_url() {
        let target = SinkTarget::SlackWebhook(
            "https://hooks.slack.com/services/T000/B000/super-secret".into(),
        );
        let label = target.safe_label();

        assert!(label.starts_with("SlackWebhook(redacted:slack-webhook:"));
        assert!(!label.contains("hooks.slack.com/services"));
        assert!(!label.contains("super-secret"));
    }

    #[test]
    fn stable_fingerprint_is_stable_and_distinguishes_target_kinds() {
        let discord = SinkTarget::DiscordWebhook("https://example.test/webhook".into());
        let slack = SinkTarget::SlackWebhook("https://example.test/webhook".into());

        assert_eq!(discord.stable_fingerprint(), discord.stable_fingerprint());
        assert_ne!(discord.stable_fingerprint(), slack.stable_fingerprint());
    }
}
