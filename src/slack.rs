use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde_json::{Value, json};

use crate::Result;
use crate::core::circuit_breaker::CircuitBreaker;
use crate::core::dlq::{Dlq, DlqEntry};
use crate::core::rate_limit::RateLimiter;
use crate::events::MessageFormat;
use crate::sink::{SinkMessage, SinkTarget};

const MAX_ATTEMPTS: u32 = 3;
const JITTER_MS: u64 = 50;
const CIRCUIT_FAILURE_THRESHOLD: u32 = 3;
const CIRCUIT_COOLDOWN_SECS: u64 = 5;
const RATE_LIMIT_CAPACITY: u32 = 5;
const RATE_LIMIT_REFILL_PER_SEC: f64 = 5.0;

#[derive(Clone)]
pub struct SlackClient {
    webhook_client: reqwest::Client,
    state: Arc<Mutex<SlackState>>,
}

#[derive(Debug)]
struct SlackState {
    limiter: RateLimiter,
    circuits: HashMap<String, CircuitBreaker>,
    dlq: Dlq,
}

#[derive(Debug)]
pub(crate) struct SlackSendError {
    pub(crate) message: String,
    pub(crate) retry_after: Option<Duration>,
}

impl SlackClient {
    pub fn new() -> Self {
        Self {
            webhook_client: reqwest::Client::new(),
            state: Arc::new(Mutex::new(SlackState {
                limiter: RateLimiter::new(RATE_LIMIT_CAPACITY, RATE_LIMIT_REFILL_PER_SEC),
                circuits: HashMap::new(),
                dlq: Dlq::default(),
            })),
        }
    }

    pub async fn send(&self, target: &SinkTarget, message: &SinkMessage) -> Result<()> {
        match target {
            SinkTarget::SlackWebhook(webhook_url) => {
                self.send_with_resilience(webhook_url, target, message)
                    .await
            }
            SinkTarget::DiscordChannel(_) | SinkTarget::DiscordWebhook(_) => {
                Err("cannot send Discord target via Slack client".into())
            }
        }
    }

    async fn send_with_resilience(
        &self,
        webhook_url: &str,
        target: &SinkTarget,
        message: &SinkMessage,
    ) -> Result<()> {
        let key = slack_rate_limit_key(webhook_url);

        if !self.allow_request(&key) {
            let error = format!("Slack circuit open for {key}");
            self.record_dlq(target, message, MAX_ATTEMPTS, error.clone());
            return Err(error.into());
        }

        for attempt in 1..=MAX_ATTEMPTS {
            let delay = self.rate_limit_delay(&key);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }

            let result = self.send_webhook(webhook_url, message).await;

            match result {
                Ok(()) => {
                    self.record_success(&key);
                    return Ok(());
                }
                Err(error) => {
                    self.record_failure(&key);
                    if let Some(retry_after) = error.retry_after
                        && attempt < MAX_ATTEMPTS
                    {
                        tokio::time::sleep(retry_after + jitter_for_attempt(attempt)).await;
                        continue;
                    }

                    // For non-429 errors, still retry with exponential backoff + jitter
                    if error.retry_after.is_none() && attempt < MAX_ATTEMPTS {
                        let backoff = exponential_backoff_for_attempt(attempt);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }

                    self.record_dlq(target, message, attempt, error.message.clone());
                    return Err(error.message.into());
                }
            }
        }

        let error = format!("Slack delivery exhausted retries for {key}");
        self.record_dlq(target, message, MAX_ATTEMPTS, error.clone());
        Err(error.into())
    }

    pub async fn send_webhook(
        &self,
        webhook_url: &str,
        message: &SinkMessage,
    ) -> std::result::Result<(), SlackSendError> {
        let response = self
            .webhook_client
            .post(webhook_url)
            .json(&webhook_payload(message))
            .send()
            .await
            .map_err(|error| SlackSendError {
                message: format!("Slack webhook request failed: {error}"),
                retry_after: None,
            })?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        let retry_after = parse_retry_after(status, &headers, &body);

        Err(SlackSendError {
            message: format!("Slack webhook request failed with {status}: {body}"),
            retry_after,
        })
    }

    fn allow_request(&self, key: &str) -> bool {
        let mut state = self.state.lock().expect("slack state lock");
        state
            .circuits
            .entry(key.to_string())
            .or_insert_with(|| {
                CircuitBreaker::new(
                    CIRCUIT_FAILURE_THRESHOLD,
                    Duration::from_secs(CIRCUIT_COOLDOWN_SECS),
                )
            })
            .allow_request()
    }

    fn rate_limit_delay(&self, key: &str) -> Duration {
        let mut state = self.state.lock().expect("slack state lock");
        state.limiter.delay_for(key)
    }

    fn record_success(&self, key: &str) {
        let mut state = self.state.lock().expect("slack state lock");
        state
            .circuits
            .entry(key.to_string())
            .or_insert_with(|| {
                CircuitBreaker::new(
                    CIRCUIT_FAILURE_THRESHOLD,
                    Duration::from_secs(CIRCUIT_COOLDOWN_SECS),
                )
            })
            .record_success();
    }

    fn record_failure(&self, key: &str) {
        let mut state = self.state.lock().expect("slack state lock");
        state
            .circuits
            .entry(key.to_string())
            .or_insert_with(|| {
                CircuitBreaker::new(
                    CIRCUIT_FAILURE_THRESHOLD,
                    Duration::from_secs(CIRCUIT_COOLDOWN_SECS),
                )
            })
            .record_failure();
    }

    fn record_dlq(&self, target: &SinkTarget, message: &SinkMessage, attempts: u32, error: String) {
        let entry = DlqEntry {
            original_topic: message.event_kind.clone(),
            retry_count: attempts,
            last_error: error,
            target: slack_rate_limit_key(match target {
                SinkTarget::SlackWebhook(url) => url,
                SinkTarget::DiscordChannel(id) => id,
                SinkTarget::DiscordWebhook(url) => url,
            }),
            event_kind: message.event_kind.clone(),
            format: message.format.as_str().to_string(),
            content: message.content.clone(),
            payload: message.payload.clone(),
        };

        eprintln!(
            "hermip dlq bury: {}",
            serde_json::to_string(&entry)
                .unwrap_or_else(|_| "{\"error\":\"dlq serialize failed\"}".to_string())
        );

        let mut state = self.state.lock().expect("slack state lock");
        state.dlq.push(entry);
    }

    #[cfg(test)]
    fn dlq_entries(&self) -> Vec<DlqEntry> {
        self.state
            .lock()
            .expect("slack state lock")
            .dlq
            .entries()
            .to_vec()
    }
}

impl Default for SlackClient {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_retry_after(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    _body: &str,
) -> Option<Duration> {
    if status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }

    // Slack returns Retry-After header for rate limiting (seconds)
    if let Some(value) = headers.get(RETRY_AFTER)
        && let Ok(seconds_str) = value.to_str()
        && let Ok(seconds) = seconds_str.trim().parse::<f64>()
    {
        return Some(Duration::from_secs_f64(seconds));
    }

    None
}

fn slack_rate_limit_key(webhook_url: &str) -> String {
    format!("slack:webhook:{webhook_url}")
}

fn jitter_for_attempt(attempt: u32) -> Duration {
    Duration::from_millis(JITTER_MS * u64::from(attempt))
}

fn exponential_backoff_for_attempt(attempt: u32) -> Duration {
    // Base delay doubles each attempt: 200ms, 400ms + jitter
    let base_ms: u64 = 200 * 2u64.pow(attempt - 1);
    let jitter = jitter_for_attempt(attempt);
    Duration::from_millis(base_ms) + jitter
}

fn webhook_payload(message: &SinkMessage) -> Value {
    let mut payload = json!({
        "text": message.content,
    });

    if matches!(
        message.format,
        MessageFormat::Compact | MessageFormat::Alert
    ) {
        payload["blocks"] = json!(slack_blocks(message));
    }

    payload
}

fn slack_blocks(message: &SinkMessage) -> Vec<Value> {
    let label = match message.format {
        MessageFormat::Alert => ":rotating_light: *Alert*",
        _ => ":speech_balloon: *Notification*",
    };

    vec![
        json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": label,
            }
        }),
        json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": message.content,
            }
        }),
        json!({
            "type": "context",
            "elements": [
                {
                    "type": "mrkdwn",
                    "text": format!("event `{}` · format `{}`", message.event_kind, message.format.as_str()),
                }
            ]
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn compact_payload_includes_block_kit_sections() {
        let payload = webhook_payload(&SinkMessage {
            event_kind: "tmux.keyword".into(),
            format: MessageFormat::Compact,
            content: "tmux:ops matched 'error' => boom".into(),
            payload: serde_json::json!({}),
        });

        assert_eq!(
            payload.get("text").and_then(Value::as_str),
            Some("tmux:ops matched 'error' => boom")
        );
        let blocks = payload
            .get("blocks")
            .and_then(Value::as_array)
            .expect("blocks");
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0]["text"]["text"].as_str(),
            Some(":speech_balloon: *Notification*")
        );
    }

    #[test]
    fn alert_payload_uses_alert_label() {
        let payload = webhook_payload(&SinkMessage {
            event_kind: "github.ci-failed".into(),
            format: MessageFormat::Alert,
            content: "🚨 deploy <failed> & paging".into(),
            payload: serde_json::json!({}),
        });

        let blocks = payload
            .get("blocks")
            .and_then(Value::as_array)
            .expect("blocks");
        assert_eq!(
            blocks[0]["text"]["text"].as_str(),
            Some(":rotating_light: *Alert*")
        );
        assert_eq!(
            blocks[1]["text"]["text"].as_str(),
            Some("🚨 deploy <failed> & paging")
        );
    }

    #[test]
    fn parses_retry_after_header_for_429() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, "0.25".parse().unwrap());
        assert_eq!(
            parse_retry_after(StatusCode::TOO_MANY_REQUESTS, &headers, ""),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            parse_retry_after(StatusCode::BAD_REQUEST, &headers, ""),
            None
        );
    }

    #[test]
    fn parses_integer_retry_after_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(
            parse_retry_after(StatusCode::TOO_MANY_REQUESTS, &headers, ""),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn retry_after_returns_none_for_non_429() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, "1".parse().unwrap());
        assert_eq!(
            parse_retry_after(StatusCode::INTERNAL_SERVER_ERROR, &headers, "error"),
            None
        );
    }

    #[test]
    fn jitter_scales_with_attempt() {
        assert_eq!(jitter_for_attempt(1), Duration::from_millis(50));
        assert_eq!(jitter_for_attempt(2), Duration::from_millis(100));
        assert_eq!(jitter_for_attempt(3), Duration::from_millis(150));
    }

    #[test]
    fn exponential_backoff_doubles_each_attempt() {
        let b1 = exponential_backoff_for_attempt(1);
        let b2 = exponential_backoff_for_attempt(2);
        let b3 = exponential_backoff_for_attempt(3);
        // Base: 200ms, 400ms, 800ms + jitter
        assert!(b1 >= Duration::from_millis(200));
        assert!(b2 >= Duration::from_millis(400));
        assert!(b3 >= Duration::from_millis(800));
        // Each should be larger than the previous
        assert!(b2 > b1);
        assert!(b3 > b2);
    }

    #[test]
    fn rate_limit_key_format() {
        assert_eq!(
            slack_rate_limit_key("https://hooks.slack.com/services/T/B/abc"),
            "slack:webhook:https://hooks.slack.com/services/T/B/abc"
        );
    }

    // --- Resilience integration tests ---

    #[tokio::test]
    async fn slack_sink_retries_429_then_succeeds() {
        // VAL-EVENT-001: Slack sink retries on 429 with jitter and backoff
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for idx in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0_u8; 4096];
                let _ = stream.read(&mut buf).await.unwrap();
                if idx == 0 {
                    let body = "429: Too Many Requests";
                    let response = format!(
                        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 0.01\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                } else {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .await
                        .unwrap();
                }
            }
        });

        let client = SlackClient::new();
        let message = SinkMessage {
            event_kind: "tmux.keyword".into(),
            format: MessageFormat::Compact,
            content: "hello".into(),
            payload: json!({"session": "ops"}),
        };
        client
            .send(
                &SinkTarget::SlackWebhook(format!("http://{addr}/webhook")),
                &message,
            )
            .await
            .unwrap();
        server.await.unwrap();
        assert!(client.dlq_entries().is_empty());
    }

    #[tokio::test]
    async fn slack_sink_429_handling_exhausted_retries_land_in_dlq() {
        // VAL-EVENT-001: Slack sink handles 429 with exponential backoff
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0_u8; 4096];
                let _ = stream.read(&mut buf).await.unwrap();
                let body = "429: Too Many Requests";
                let response = format!(
                    "HTTP/1.1 429 Too Many Requests\r\nretry-after: 0.0\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = SlackClient::new();
        let message = SinkMessage {
            event_kind: "github.ci-failed".into(),
            format: MessageFormat::Alert,
            content: "boom".into(),
            payload: json!({"repo": "hermip"}),
        };
        let error = client
            .send(
                &SinkTarget::SlackWebhook(format!("http://{addr}/webhook")),
                &message,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("429"));
        server.await.unwrap();
        let dlq = client.dlq_entries();
        assert_eq!(dlq.len(), 1);
        assert_eq!(dlq[0].payload["repo"], "hermip");
        assert_eq!(dlq[0].retry_count, 3);
    }

    #[tokio::test]
    async fn slack_sink_circuit_breaker_activates_on_sustained_failures() {
        // VAL-EVENT-002: Slack sink circuit breaker activates on sustained failures
        let client = SlackClient::new();
        let key = slack_rate_limit_key("http://test/webhook");

        // Simulate sustained failures to open the circuit
        for _ in 0..CIRCUIT_FAILURE_THRESHOLD {
            client.record_failure(&key);
        }

        // Circuit should now be open, rejecting requests
        assert!(!client.allow_request(&key));

        // Verify DLQ gets the rejected message
        let message = SinkMessage {
            event_kind: "test.event".into(),
            format: MessageFormat::Compact,
            content: "test".into(),
            payload: json!({}),
        };

        let result = client
            .send(
                &SinkTarget::SlackWebhook("http://test/webhook".into()),
                &message,
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("circuit open"));
        assert_eq!(client.dlq_entries().len(), 1);
    }

    #[tokio::test]
    async fn slack_sink_circuit_breaker_probes_after_cooldown() {
        // VAL-EVENT-002: Circuit breaker transitions cool-down → probe → resume.
        // Use a dedicated CircuitBreaker with short cooldown for deterministic testing.
        let mut breaker = CircuitBreaker::new(CIRCUIT_FAILURE_THRESHOLD, Duration::from_millis(1));

        // Trip the circuit open
        for _ in 0..CIRCUIT_FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        assert_eq!(breaker.state_name(), "open");

        // Wait for cooldown → half-open
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(breaker.allow_request());
        assert_eq!(breaker.state_name(), "half-open");

        // Successful probe → closed
        breaker.record_success();
        assert_eq!(breaker.state_name(), "closed");
        assert!(breaker.allow_request());
    }

    #[tokio::test]
    async fn slack_sink_circuit_breaker_reopens_on_probe_failure() {
        // VAL-EVENT-002: Circuit breaker re-opens if the probe request fails.
        // Use a dedicated CircuitBreaker with short cooldown for deterministic testing.
        let mut breaker = CircuitBreaker::new(CIRCUIT_FAILURE_THRESHOLD, Duration::from_millis(1));

        // Trip the circuit open
        for _ in 0..CIRCUIT_FAILURE_THRESHOLD {
            breaker.record_failure();
        }
        assert_eq!(breaker.state_name(), "open");
        assert!(!breaker.allow_request());

        // Wait for cooldown to expire, then allow_request transitions to half-open
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(breaker.allow_request());
        assert_eq!(breaker.state_name(), "half-open");

        // Probe request fails → circuit goes back to open
        breaker.record_failure();
        assert_eq!(breaker.state_name(), "open");
        assert!(!breaker.allow_request());
    }

    #[tokio::test]
    async fn slack_sink_dlq_captures_failed_deliveries() {
        // VAL-EVENT-003: Slack sink has in-memory DLQ for exhausted retries
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0_u8; 4096];
                let _ = stream.read(&mut buf).await.unwrap();
                let body = "Internal Server Error";
                let response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = SlackClient::new();
        let message = SinkMessage {
            event_kind: "droid.mission-failed".into(),
            format: MessageFormat::Alert,
            content: "mission failed".into(),
            payload: json!({"mission_id": "abc-123"}),
        };
        let error = client
            .send(
                &SinkTarget::SlackWebhook(format!("http://{addr}/webhook")),
                &message,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("500"));
        server.await.unwrap();

        let dlq = client.dlq_entries();
        assert_eq!(dlq.len(), 1);
        assert_eq!(dlq[0].event_kind, "droid.mission-failed");
        assert_eq!(dlq[0].payload["mission_id"], "abc-123");
        assert_eq!(dlq[0].retry_count, 3);
        assert!(dlq[0].last_error.contains("500"));
    }

    #[tokio::test]
    async fn slack_sink_retries_server_errors_with_exponential_backoff() {
        // VAL-EVENT-001: Slack sink retries on failures with jitter
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for idx in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0_u8; 4096];
                let _ = stream.read(&mut buf).await.unwrap();
                if idx < 2 {
                    let body = "Internal Server Error";
                    let response = format!(
                        "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain\r\ncontent-length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                } else {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .await
                        .unwrap();
                }
            }
        });

        let client = SlackClient::new();
        let message = SinkMessage {
            event_kind: "github.pr-opened".into(),
            format: MessageFormat::Compact,
            content: "new PR".into(),
            payload: json!({"repo": "hermip"}),
        };
        client
            .send(
                &SinkTarget::SlackWebhook(format!("http://{addr}/webhook")),
                &message,
            )
            .await
            .unwrap();
        server.await.unwrap();
        assert!(client.dlq_entries().is_empty());
    }

    #[tokio::test]
    async fn slack_sink_success_resets_circuit_breaker() {
        // VAL-EVENT-002: Circuit breaker resets on success (cool-down → probe → resume)
        let client = SlackClient::new();
        let key = slack_rate_limit_key("http://test-reset/webhook");

        // Record some failures (not enough to trip the circuit)
        client.record_failure(&key);
        client.record_failure(&key);
        assert!(client.allow_request(&key)); // Still closed

        // Success resets the failure counter
        client.record_success(&key);

        // Now it should take another CIRCUIT_FAILURE_THRESHOLD failures to trip
        for _ in 0..(CIRCUIT_FAILURE_THRESHOLD - 1) {
            client.record_failure(&key);
        }
        assert!(client.allow_request(&key)); // Still closed, not yet at threshold
    }

    #[tokio::test]
    async fn slack_sink_dlq_preserves_full_context() {
        // VAL-EVENT-003: DLQ preserves full message context for inspection
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = vec![0_u8; 4096];
                let _ = stream.read(&mut buf).await.unwrap();
                let response =
                    "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 9\r\n\r\nunavailable";
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = SlackClient::new();
        let message = SinkMessage {
            event_kind: "tmux.stale".into(),
            format: MessageFormat::Alert,
            content: "stale pane detected".into(),
            payload: json!({"session": "build", "pane": 3}),
        };
        let _ = client
            .send(
                &SinkTarget::SlackWebhook(format!("http://{addr}/webhook")),
                &message,
            )
            .await;
        server.await.unwrap();

        let dlq = client.dlq_entries();
        assert_eq!(dlq.len(), 1);
        let entry = &dlq[0];
        assert_eq!(entry.event_kind, "tmux.stale");
        assert_eq!(entry.format, "alert");
        assert_eq!(entry.content, "stale pane detected");
        assert_eq!(entry.payload["session"], "build");
        assert_eq!(entry.payload["pane"], 3);
        assert!(entry.last_error.contains("503"));
        assert!(entry.target.starts_with("slack:webhook:"));
    }

    #[tokio::test]
    async fn slack_sink_rejects_discord_targets() {
        let client = SlackClient::new();
        let message = SinkMessage {
            event_kind: "test".into(),
            format: MessageFormat::Compact,
            content: "hello".into(),
            payload: json!({}),
        };

        let result = client
            .send(&SinkTarget::DiscordChannel("123".into()), &message)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Discord target"));
    }

    // ---------------------------------------------------------------------------
    // VAL-CROSS-002: Resilience parity between Discord and Slack sinks
    // ---------------------------------------------------------------------------

    #[test]
    fn slack_resilience_constants_match_discord() {
        // Verify that Slack sink resilience constants match Discord sink values.
        // Discord uses the same constants (MAX_ATTEMPTS=3, JITTER_MS=50,
        // CIRCUIT_FAILURE_THRESHOLD=3, CIRCUIT_COOLDOWN_SECS=5).
        assert_eq!(MAX_ATTEMPTS, 3, "Slack MAX_ATTEMPTS should match Discord");
        assert_eq!(JITTER_MS, 50, "Slack JITTER_MS should match Discord");
        assert_eq!(
            CIRCUIT_FAILURE_THRESHOLD, 3,
            "Slack CIRCUIT_FAILURE_THRESHOLD should match Discord"
        );
        assert_eq!(
            CIRCUIT_COOLDOWN_SECS, 5,
            "Slack CIRCUIT_COOLDOWN_SECS should match Discord"
        );
    }

    #[test]
    fn slack_has_same_retry_count_as_discord() {
        // Both Discord and Slack should use the same maximum retry count.
        // This test verifies structural parity - the constant is the same.
        assert_eq!(
            MAX_ATTEMPTS,
            crate::discord::MAX_ATTEMPTS,
            "Slack MAX_ATTEMPTS must match Discord MAX_ATTEMPTS for resilience parity"
        );
    }
}
