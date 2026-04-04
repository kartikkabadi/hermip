use std::time::Duration;

use tokio::process::Command;
use tokio::time::sleep;

use crate::Result;
use crate::source::tmux::{content_hash, tmux_bin};

const DEFAULT_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_TUI_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_VERIFY_DELAY: Duration = Duration::from_millis(250);
const PROMPT_CHARS: &[char] = &['$', '%', '>', '#', '❯', '›'];

/// Configuration for prompt delivery to a tmux session.
#[derive(Debug, Clone)]
pub struct PromptDeliverConfig {
    pub session: String,
    pub prompt: String,
    pub max_attempts: u32,
    pub tui_timeout: Duration,
    pub poll_interval: Duration,
    pub verify_delay: Duration,
    pub verify_keywords: Vec<String>,
}

impl PromptDeliverConfig {
    pub fn new(session: String, prompt: String) -> Self {
        Self {
            session,
            prompt,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            tui_timeout: DEFAULT_TUI_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            verify_delay: DEFAULT_VERIFY_DELAY,
            verify_keywords: Vec::new(),
        }
    }
}

/// Result of a prompt delivery attempt.
#[derive(Debug, Clone)]
pub struct DeliveryResult {
    pub delivered: bool,
    pub attempts: u32,
    pub verified: bool,
}

/// Deliver a prompt to a tmux session with TUI readiness detection, retry, and verification.
///
/// 1. Wait for the TUI to become ready (prompt character visible or timeout).
/// 2. Send the prompt text via `tmux send-keys -l`.
/// 3. Send Enter.
/// 4. Verify delivery by checking pane content changed (and optionally for keywords).
/// 5. Retry up to `max_attempts` on failure.
pub async fn deliver(config: &PromptDeliverConfig) -> Result<DeliveryResult> {
    let mut last_error: Option<String> = None;

    for attempt in 1..=config.max_attempts {
        match try_deliver(config, attempt).await {
            Ok(result) => return Ok(result),
            Err(error) => {
                last_error = Some(error.to_string());
                if attempt < config.max_attempts {
                    let backoff = Duration::from_millis(500 * u64::from(attempt));
                    sleep(backoff).await;
                }
            }
        }
    }

    Err(format!(
        "prompt delivery to '{}' failed after {} attempts: {}",
        config.session,
        config.max_attempts,
        last_error.unwrap_or_else(|| "unknown".into()),
    )
    .into())
}

async fn try_deliver(config: &PromptDeliverConfig, attempt: u32) -> Result<DeliveryResult> {
    wait_for_tui_ready(&config.session, config.tui_timeout, config.poll_interval).await?;

    let pre_hash = capture_pane_hash(&config.session).await?;

    send_literal_keys(&config.session, &config.prompt).await?;
    send_key(&config.session, "Enter").await?;

    sleep(config.verify_delay).await;

    let post_hash = capture_pane_hash(&config.session).await?;
    let content_changed = post_hash != pre_hash;

    let keyword_verified = if config.verify_keywords.is_empty() {
        true
    } else {
        verify_keywords(&config.session, &config.verify_keywords).await?
    };

    let verified = content_changed && keyword_verified;

    if !content_changed {
        return Err(
            format!("attempt {attempt}: pane content unchanged after sending prompt").into(),
        );
    }

    Ok(DeliveryResult {
        delivered: true,
        attempts: attempt,
        verified,
    })
}

/// Poll the tmux pane until a prompt character appears or the timeout expires.
async fn wait_for_tui_ready(
    session: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        if tokio::time::Instant::now() >= deadline {
            // Timeout — proceed anyway (the TUI may still accept input).
            return Ok(());
        }

        match capture_last_line(session).await {
            Ok(line) if has_prompt_char(&line) => return Ok(()),
            Ok(_) => {}
            Err(_) => {}
        }

        sleep(poll_interval).await;
    }
}

fn has_prompt_char(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    PROMPT_CHARS
        .iter()
        .any(|ch| trimmed.ends_with(*ch) || trimmed.ends_with(&format!("{ch} ")))
}

async fn capture_last_line(session: &str) -> Result<String> {
    let output = Command::new(tmux_bin())
        .arg("capture-pane")
        .arg("-p")
        .arg("-t")
        .arg(session)
        .arg("-S")
        .arg("-1")
        .output()
        .await?;
    if !output.status.success() {
        return Err(tmux_stderr(&output.stderr).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn capture_pane_hash(session: &str) -> Result<u64> {
    let output = Command::new(tmux_bin())
        .arg("capture-pane")
        .arg("-p")
        .arg("-t")
        .arg(session)
        .arg("-S")
        .arg("-200")
        .output()
        .await?;
    if !output.status.success() {
        return Err(tmux_stderr(&output.stderr).into());
    }
    Ok(content_hash(&String::from_utf8(output.stdout)?))
}

async fn send_literal_keys(session: &str, text: &str) -> Result<()> {
    let output = Command::new(tmux_bin())
        .arg("send-keys")
        .arg("-t")
        .arg(session)
        .arg("-l")
        .arg(text)
        .output()
        .await?;
    if !output.status.success() {
        return Err(tmux_stderr(&output.stderr).into());
    }
    Ok(())
}

async fn send_key(session: &str, key: &str) -> Result<()> {
    let output = Command::new(tmux_bin())
        .arg("send-keys")
        .arg("-t")
        .arg(session)
        .arg(key)
        .output()
        .await?;
    if !output.status.success() {
        return Err(tmux_stderr(&output.stderr).into());
    }
    Ok(())
}

/// Check whether any of the verify keywords appear in the current pane content.
async fn verify_keywords(session: &str, keywords: &[String]) -> Result<bool> {
    let output = Command::new(tmux_bin())
        .arg("capture-pane")
        .arg("-p")
        .arg("-t")
        .arg(session)
        .arg("-S")
        .arg("-50")
        .output()
        .await?;
    if !output.status.success() {
        return Err(tmux_stderr(&output.stderr).into());
    }
    let content = String::from_utf8_lossy(&output.stdout);
    Ok(keywords.iter().any(|kw| content.contains(kw.as_str())))
}

fn tmux_stderr(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_prompt_char_detects_common_shells() {
        assert!(has_prompt_char("user@host:~$ "));
        assert!(has_prompt_char("~ %"));
        assert!(has_prompt_char(">>> "));
        assert!(has_prompt_char("root@host:/# "));
        assert!(has_prompt_char("❯"));
        assert!(has_prompt_char("›"));
    }

    #[test]
    fn has_prompt_char_rejects_empty_and_output_lines() {
        assert!(!has_prompt_char(""));
        assert!(!has_prompt_char("   "));
        assert!(!has_prompt_char("compiling clawhip v0.5.0"));
        assert!(!has_prompt_char("error[E0308]: mismatched types"));
    }

    #[test]
    fn config_defaults_are_sensible() {
        let config = PromptDeliverConfig::new("test".into(), "hello".into());
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.tui_timeout, Duration::from_secs(30));
        assert_eq!(config.poll_interval, Duration::from_millis(500));
        assert_eq!(config.verify_delay, Duration::from_millis(250));
        assert!(config.verify_keywords.is_empty());
    }

    #[test]
    fn prompt_chars_include_expected_set() {
        assert!(PROMPT_CHARS.contains(&'$'));
        assert!(PROMPT_CHARS.contains(&'%'));
        assert!(PROMPT_CHARS.contains(&'>'));
        assert!(PROMPT_CHARS.contains(&'#'));
        assert!(PROMPT_CHARS.contains(&'❯'));
        assert!(PROMPT_CHARS.contains(&'›'));
    }
}
