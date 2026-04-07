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
const MAX_VERIFY_KEYWORDS: usize = 6;
const GENERIC_FALLBACK_PATTERNS: &[&str] = &[
    "implement {feature}",
    "summarize recent commits",
    "explain this codebase",
];
const VERIFY_STOPWORDS: &[&str] = &[
    "a", "an", "and", "the", "this", "that", "these", "those", "for", "from", "with", "into",
    "about", "your", "you", "please", "task", "tasks", "issue", "issues", "review", "reviews",
    "session", "sessions", "native", "clawhip", "omx", "omc", "fix", "now",
];

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

    let pane_content = capture_recent_pane(&config.session).await?;
    let post_hash = content_hash(&pane_content);
    let content_changed = post_hash != pre_hash;
    let verified = content_changed
        && verify_prompt_against_content(&config.prompt, &pane_content, &config.verify_keywords);

    if !content_changed {
        return Err(
            format!("attempt {attempt}: pane content unchanged after sending prompt").into(),
        );
    }

    if !verified {
        return Err(format!(
            "attempt {attempt}: pane content changed but did not reflect the requested task"
        )
        .into());
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
    Ok(content_hash(&capture_recent_pane(session).await?))
}

async fn capture_recent_pane(session: &str) -> Result<String> {
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
    Ok(String::from_utf8(output.stdout)?)
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

pub fn derive_verify_keywords(prompt: &str) -> Vec<String> {
    let mut keywords = Vec::new();

    for token in prompt
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
    {
        if VERIFY_STOPWORDS.contains(&token.as_str()) {
            continue;
        }
        let is_numeric = token.chars().all(|ch| ch.is_ascii_digit());
        if !(token.len() >= 4 || (is_numeric && token.len() >= 3)) {
            continue;
        }
        if !keywords.contains(&token) {
            keywords.push(token);
        }
        if keywords.len() >= MAX_VERIFY_KEYWORDS {
            break;
        }
    }

    keywords
}

fn verify_prompt_against_content(prompt: &str, content: &str, verify_keywords: &[String]) -> bool {
    if looks_like_generic_fallback(content) {
        return false;
    }

    let effective_keywords = if verify_keywords.is_empty() {
        derive_verify_keywords(prompt)
    } else {
        verify_keywords.to_vec()
    };

    if effective_keywords.is_empty() {
        return true;
    }

    content_matches_keywords(content, &effective_keywords)
}

fn looks_like_generic_fallback(content: &str) -> bool {
    let normalized = content.to_ascii_lowercase();
    GENERIC_FALLBACK_PATTERNS
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

fn content_matches_keywords(content: &str, keywords: &[String]) -> bool {
    let normalized = content.to_ascii_lowercase();
    let required_matches = keywords.len().min(2);
    keywords
        .iter()
        .filter(|keyword| normalized.contains(keyword.as_str()))
        .take(required_matches)
        .count()
        >= required_matches.max(1)
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

    #[test]
    fn derive_verify_keywords_prefers_specific_task_terms() {
        let keywords = derive_verify_keywords(
            "Fix issue #166 now with minimal mergeable scope focused on live operational recovery",
        );

        assert_eq!(
            keywords,
            vec![
                "166".to_string(),
                "minimal".to_string(),
                "mergeable".to_string(),
                "scope".to_string(),
                "focused".to_string(),
                "live".to_string(),
            ]
        );
    }

    #[test]
    fn verify_prompt_against_content_rejects_generic_fallbacks() {
        let prompt =
            "Fix issue #166 now with minimal mergeable scope focused on live operational recovery";
        let content = "Implement {feature}\nSummarize recent commits\nExplain this codebase";

        assert!(!verify_prompt_against_content(prompt, content, &[]));
    }

    #[test]
    fn verify_prompt_against_content_requires_multiple_keyword_matches_when_available() {
        let keywords = derive_verify_keywords(
            "Fix issue #166 now with minimal mergeable scope focused on live operational recovery",
        );

        assert!(verify_prompt_against_content(
            "",
            "Working task: issue 166 live operational recovery is in progress",
            &keywords,
        ));
        assert!(!verify_prompt_against_content(
            "",
            "Working task: issue 166",
            &keywords,
        ));
    }
}
