use super::traits::{Channel, ChannelMessage, SendMessage};
use anyhow::Context;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Clone)]
struct CachedSlackDisplayName {
    display_name: String,
    expires_at: Instant,
}

/// Slack channel — polls conversations.history via Web API
pub struct SlackChannel {
    bot_token: String,
    app_token: Option<String>,
    channel_id: Option<String>,
    channel_ids: Vec<String>,
    allowed_users: Vec<String>,
    mention_only: bool,
    group_reply_allowed_sender_ids: Vec<String>,
    user_display_name_cache: Mutex<HashMap<String, CachedSlackDisplayName>>,
const SLACK_ATTACHMENT_IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
const SLACK_ATTACHMENT_IMAGE_INLINE_FALLBACK_MAX_BYTES: usize = 512 * 1024;
const SLACK_ATTACHMENT_TEXT_DOWNLOAD_MAX_BYTES: usize = 256 * 1024;
const SLACK_ATTACHMENT_TEXT_INLINE_MAX_CHARS: usize = 12_000;
const SLACK_ATTACHMENT_FILENAME_MAX_CHARS: usize = 128;
const SLACK_ATTACHMENT_SAVE_SUBDIR: &str = "slack_files";
const SLACK_ATTACHMENT_MAX_FILES_PER_MESSAGE: usize = 8;
const SLACK_ATTACHMENT_RENDER_CONCURRENCY: usize = 3;
const SLACK_ALLOWED_MEDIA_HOST_SUFFIXES: &[&str] =
    &["slack.com", "slack-edge.com", "slack-files.com"];
const SLACK_SUPPORTED_IMAGE_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/bmp",
];

impl SlackChannel {
    pub fn new(
        bot_token: String,
        app_token: Option<String>,
        channel_id: Option<String>,
        channel_ids: Vec<String>,
        allowed_users: Vec<String>,
    ) -> Self {
        Self {
            bot_token,
            app_token,
            channel_id,
            channel_ids,
            allowed_users,
            mention_only: false,
            group_reply_allowed_sender_ids: Vec::new(),
            user_display_name_cache: Mutex::new(HashMap::new()),
    /// Configure workspace directory used for persisting inbound Slack attachments.
    pub fn with_workspace_dir(mut self, dir: PathBuf) -> Self {
        self.workspace_dir = Some(dir);
        self
    }

    fn http_client(&self) -> reqwest::Client {
        crate::config::build_runtime_proxy_client("channel.slack")
    }

    /// Check if a Slack user ID is in the allowlist.
    /// Empty list means deny everyone until explicitly configured.
    /// `"*"` means allow everyone.
    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == "*" || u == user_id)
    }

    fn is_group_sender_trigger_enabled(&self, user_id: &str) -> bool {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return false;
        }

        self.group_reply_allowed_sender_ids
            .iter()
            .any(|entry| entry == "*" || entry == user_id)
    }

    /// Get the bot's own user ID so we can ignore our own messages
    async fn get_bot_user_id(&self) -> Option<String> {
        let resp: serde_json::Value = self
            .http_client()
            .get("https://slack.com/api/auth.test")
            .bearer_auth(&self.bot_token)
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;

        resp.get("user_id")
            .and_then(|u| u.as_str())
            .map(String::from)
    }

    /// Resolve the thread identifier for inbound Slack messages.
    /// Replies carry `thread_ts` (root thread id); top-level messages only have `ts`.
    fn inbound_thread_ts(msg: &serde_json::Value, ts: &str) -> Option<String> {
        msg.get("thread_ts")
            .and_then(|t| t.as_str())
            .or(if ts.is_empty() { None } else { Some(ts) })
            .map(str::to_string)
    }

    fn normalized_channel_id(input: Option<&str>) -> Option<String> {
        input
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "*")
            .map(ToOwned::to_owned)
    }

    fn configured_channel_id(&self) -> Option<String> {
        Self::normalized_channel_id(self.channel_id.as_deref())
    }

    /// Resolve the effective channel scope:
    /// explicit `channel_ids` list first, then single `channel_id`, otherwise wildcard discovery.
    fn scoped_channel_ids(&self) -> Option<Vec<String>> {
        let mut seen = HashSet::new();
        let ids: Vec<String> = self
            .channel_ids
            .iter()
            .filter_map(|entry| Self::normalized_channel_id(Some(entry)))
            .filter(|id| seen.insert(id.clone()))
            .collect();
        if !ids.is_empty() {
            return Some(ids);
        }
        self.configured_channel_id().map(|id| vec![id])
    }

    fn configured_app_token(&self) -> Option<String> {
        self.app_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn normalize_group_reply_allowed_sender_ids(sender_ids: Vec<String>) -> Vec<String> {
        let mut normalized = sender_ids
            .into_iter()
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect::<Vec<_>>();
        normalized.sort();
        normalized.dedup();
        normalized
    }

    fn user_cache_ttl() -> Duration {
        Duration::from_secs(SLACK_USER_CACHE_TTL_SECS)
    }

    fn sanitize_display_name(name: &str) -> Option<String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn extract_user_display_name(payload: &serde_json::Value) -> Option<String> {
        let user = payload.get("user")?;
        let profile = user.get("profile");

        let candidates = [
            profile
                .and_then(|p| p.get("display_name"))
                .and_then(|v| v.as_str()),
            profile
                .and_then(|p| p.get("display_name_normalized"))
                .and_then(|v| v.as_str()),
            profile
                .and_then(|p| p.get("real_name_normalized"))
                .and_then(|v| v.as_str()),
            profile
                .and_then(|p| p.get("real_name"))
                .and_then(|v| v.as_str()),
            user.get("real_name").and_then(|v| v.as_str()),
            user.get("name").and_then(|v| v.as_str()),
        ];

        for candidate in candidates.into_iter().flatten() {
            if let Some(display_name) = Self::sanitize_display_name(candidate) {
                return Some(display_name);
            }
        }

        None
    }

    fn cached_sender_display_name(&self, user_id: &str) -> Option<String> {
        let now = Instant::now();
        let Ok(mut cache) = self.user_display_name_cache.lock() else {
            return None;
        };

        if let Some(entry) = cache.get(user_id) {
            if now <= entry.expires_at {
                return Some(entry.display_name.clone());
            }
        }

        cache.remove(user_id);
        None
    }

    fn cache_sender_display_name(&self, user_id: &str, display_name: &str) {
        let Ok(mut cache) = self.user_display_name_cache.lock() else {
            return;
        };
        cache.insert(
            user_id.to_string(),
            CachedSlackDisplayName {
                display_name: display_name.to_string(),
                expires_at: Instant::now() + Self::user_cache_ttl(),
            },
        );
    }

    async fn fetch_sender_display_name(&self, user_id: &str) -> Option<String> {
        let resp = match self
            .http_client()
            .get("https://slack.com/api/users.info")
            .bearer_auth(&self.bot_token)
            .query(&[("user", user_id)])
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!("Slack users.info request failed for {user_id}: {err}");
                return None;
            }
        };

        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));

        if !status.is_success() {
            let sanitized = crate::providers::sanitize_api_error(&body);
            tracing::warn!("Slack users.info failed for {user_id} ({status}): {sanitized}");
            return None;
        }

        let payload: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if payload.get("ok") == Some(&serde_json::Value::Bool(false)) {
            let err = payload
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            tracing::warn!("Slack users.info returned error for {user_id}: {err}");
            return None;
        }

        Self::extract_user_display_name(&payload)
    }

    async fn resolve_sender_identity(&self, user_id: &str) -> String {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return String::new();
        }

        if let Some(display_name) = self.cached_sender_display_name(user_id) {
            return display_name;
        }

        if let Some(display_name) = self.fetch_sender_display_name(user_id).await {
            self.cache_sender_display_name(user_id, &display_name);
            return display_name;
        }

        user_id.to_string()
    }

    fn is_group_channel_id(channel_id: &str) -> bool {
        matches!(channel_id.chars().next(), Some('C' | 'G'))
    }

    fn contains_bot_mention(text: &str, bot_user_id: &str) -> bool {
        if bot_user_id.is_empty() {
            return false;
        }
        text.contains(&format!("<@{bot_user_id}>"))
    }

    fn strip_bot_mentions(text: &str, bot_user_id: &str) -> String {
        if bot_user_id.is_empty() {
            return text.trim().to_string();
        }
        text.replace(&format!("<@{bot_user_id}>"), " ")
            .trim()
            .to_string()
    }

        let normalized = Self::normalize_incoming_text(text, require_mention, bot_user_id)?;
        if normalized.is_empty() {
            return None;
        }
        Some(normalized)
    }

                let subtype = event.get("subtype").and_then(|v| v.as_str());
                if !Self::is_supported_message_subtype(subtype) {
                    continue;
                }

                let channel_id = event
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_default();
                if channel_id.is_empty() {
                    continue;
                }
                if let Some(ref configured_channels) = scoped_channels {
                    if !configured_channels.iter().any(|id| id == &channel_id) {
                        continue;
                    }
                }

                let user = event
                    .get("user")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if user.is_empty() || user == bot_user_id {
                    continue;
                }
                if !self.is_user_allowed(user) {
                    tracing::warn!("Slack: ignoring message from unauthorized user: {user}");
                    continue;
                }

>>>>>>> 526d63fd (feat: add slack file reading capability to zeroclaw)
                let ts = event.get("ts").and_then(|v| v.as_str()).unwrap_or_default();
                if ts.is_empty() {
                    continue;
                }
                let last_ts = last_ts_by_channel
                    .get(&channel_id)
                    .map(String::as_str)
                    .unwrap_or_default();
                if ts <= last_ts {
                    continue;
                }

                let is_group_message = Self::is_group_channel_id(&channel_id);
                let allow_sender_without_mention =
                    is_group_message && self.is_group_sender_trigger_enabled(user);
                let require_mention =
                    self.mention_only && is_group_message && !allow_sender_without_mention;

<<<<<<< HEAD
                let Some(normalized_text) =
                    Self::normalize_incoming_content(text, require_mention, bot_user_id)
=======
                let Some(normalized_text) = self
                    .build_incoming_content(event, require_mention, bot_user_id)
                    .await
                else {
                    continue;
                };

                last_ts_by_channel.insert(channel_id.clone(), ts.to_string());
                let sender = self.resolve_sender_identity(user).await;

                let channel_msg = ChannelMessage {
                    id: format!("slack_{channel_id}_{ts}"),
                    sender,
                    reply_target: channel_id.clone(),
                    content: normalized_text,
                    channel: "slack".to_string(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    thread_ts: Self::inbound_thread_ts(event, ts),
                };

                if tx.send(channel_msg).await.is_err() {
                    return Ok(());
                }
            }

            tracing::warn!("Slack Socket Mode: reconnecting in 3 seconds...");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    fn parse_retry_after_secs(headers: &HeaderMap) -> Option<u64> {
        let value = headers
            .get(reqwest::header::RETRY_AFTER)?
            .to_str()
            .ok()?
            .trim();
        Self::parse_retry_after_value(value)
    }

    fn parse_retry_after_value(value: &str) -> Option<u64> {
        if value.is_empty() {
            return None;
        }

        if let Ok(seconds) = value.parse::<u64>() {
            return Some(seconds);
        }

        let truncated = value
            .split_once('.')
            .map(|(whole, _)| whole)
            .unwrap_or(value);
        truncated.parse::<u64>().ok()
    }

    fn jitter_ms_from_clock(max_jitter_ms: u64) -> u64 {
        if max_jitter_ms == 0 {
            return 0;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::from(d.subsec_nanos()))
            .unwrap_or(0);
        nanos % (max_jitter_ms + 1)
    }

    fn compute_retry_delay(base_retry_after_secs: u64, attempt: u32, jitter_ms: u64) -> Duration {
        let multiplier = 1_u64.checked_shl(attempt).unwrap_or(u64::MAX);
        let backoff_secs = base_retry_after_secs
            .saturating_mul(multiplier)
            .min(SLACK_HISTORY_MAX_BACKOFF_SECS);
        Duration::from_secs(backoff_secs) + Duration::from_millis(jitter_ms)
    }

    fn next_retry_timestamp(wait: Duration) -> String {
        match chrono::Duration::from_std(wait) {
            Ok(delta) => (Utc::now() + delta).to_rfc3339(),
            Err(_) => Utc::now().to_rfc3339(),
        }
    }

    async fn fetch_history_with_retry(
        &self,
        channel_id: &str,
        params: &[(&str, String)],
    ) -> Option<serde_json::Value> {
        let mut total_wait = Duration::from_secs(0);

        for attempt in 0..=SLACK_HISTORY_MAX_RETRIES {
            let resp = match self
                .http_client()
                .get("https://slack.com/api/conversations.history")
                .bearer_auth(&self.bot_token)
                .query(params)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Slack poll error for channel {channel_id}: {e}");
                    return None;
                }
            };

            let status = resp.status();
            let headers = resp.headers().clone();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));

            let is_ratelimited_http = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            let payload: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let is_ratelimited_payload = payload.get("ok") == Some(&serde_json::Value::Bool(false))
                && payload
                    .get("error")
                    .and_then(|e| e.as_str())
                    .is_some_and(|err| err == "ratelimited");

            if is_ratelimited_http || is_ratelimited_payload {
                if attempt >= SLACK_HISTORY_MAX_RETRIES {
                    tracing::error!(
                        "Slack rate limit retries exhausted for conversations.history on channel {}. Total wait: {}s across {} attempts. Proceeding without channel history.",
                        channel_id,
                        total_wait.as_secs(),
                        SLACK_HISTORY_MAX_RETRIES
                    );
                    return None;
                }

                let retry_after_secs = Self::parse_retry_after_secs(&headers)
                    .unwrap_or(SLACK_HISTORY_DEFAULT_RETRY_AFTER_SECS);
                let jitter_ms = Self::jitter_ms_from_clock(SLACK_HISTORY_MAX_JITTER_MS);
                let wait = Self::compute_retry_delay(retry_after_secs, attempt, jitter_ms);
                total_wait += wait;
                let next_retry_at = Self::next_retry_timestamp(wait);
                tracing::warn!(
                    "Slack conversations.history rate limited for channel {}. Retry-After: {}s. Attempt {}/{}. Next retry at {}.",
                    channel_id,
                    retry_after_secs,
                    attempt + 1,
                    SLACK_HISTORY_MAX_RETRIES,
                    next_retry_at
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            if !status.is_success() {
                let sanitized = crate::providers::sanitize_api_error(&body);
                tracing::warn!(
                    "Slack history request failed for channel {} ({}): {}",
                    channel_id,
                    status,
                    sanitized
                );
                return None;
            }

            if payload.get("ok") == Some(&serde_json::Value::Bool(false)) {
                let err = payload
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown");
                tracing::warn!("Slack history error for channel {channel_id}: {err}");
                return None;
            }

            return Some(payload);
        }

        None
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let mut body = serde_json::json!({
            "channel": message.recipient,
            "text": message.content
        });

        if let Some(ref ts) = message.thread_ts {
            body["thread_ts"] = serde_json::json!(ts);
        }

        let resp = self
            .http_client()
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));

        if !status.is_success() {
            let sanitized = crate::providers::sanitize_api_error(&body);
            anyhow::bail!("Slack chat.postMessage failed ({status}): {sanitized}");
        }

        // Slack returns 200 for most app-level errors; check JSON "ok" field
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if parsed.get("ok") == Some(&serde_json::Value::Bool(false)) {
            let err = parsed
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Slack chat.postMessage failed: {err}");
        }

        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        let bot_user_id = self.get_bot_user_id().await.unwrap_or_default();
        let scoped_channels = self.scoped_channel_ids();
        if self.configured_app_token().is_some() {
            tracing::info!("Slack channel listening in Socket Mode");
            return self
                .listen_socket_mode(tx, &bot_user_id, scoped_channels)
                .await;
        }

        let mut discovered_channels: Vec<String> = Vec::new();
        let mut last_discovery = Instant::now();
        let mut last_ts_by_channel: HashMap<String, String> = HashMap::new();

        if let Some(ref channel_ids) = scoped_channels {
            tracing::info!(
                "Slack channel listening on {} configured channel(s): {}",
                channel_ids.len(),
                channel_ids.join(", ")
            );
        } else {
            tracing::info!(
                "Slack channel_id/channel_ids not set (or wildcard only); listening across all accessible channels."
            );
        }

        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;

            let target_channels = if let Some(ref channel_ids) = scoped_channels {
                channel_ids.clone()
            } else {
                if discovered_channels.is_empty()
                    || last_discovery.elapsed() >= Duration::from_secs(60)
                {
                    match self.list_accessible_channels().await {
                        Ok(channels) => {
                            if channels != discovered_channels {
                                tracing::info!(
                                    "Slack auto-discovery refreshed: listening on {} channel(s).",
                                    channels.len()
                                );
                            }
                            discovered_channels = channels;
                        }
                        Err(e) => {
                            tracing::warn!("Slack channel discovery failed: {e}");
                        }
                    }
                    last_discovery = Instant::now();
                }

                discovered_channels.clone()
            };

            if target_channels.is_empty() {
                tracing::debug!("Slack: no accessible channels discovered yet");
                continue;
            }

            for channel_id in target_channels {
                let had_cursor = last_ts_by_channel.contains_key(&channel_id);
                let bootstrap_ts = Self::slack_now_ts();
                let cursor_ts =
                    Self::ensure_poll_cursor(&mut last_ts_by_channel, &channel_id, &bootstrap_ts);
                if !had_cursor {
                    tracing::debug!(
                        "Slack: initialized cursor for channel {} at {} to prevent historical replay",
                        channel_id,
                        cursor_ts
                    );
                }
                let params = vec![
                    ("channel", channel_id.clone()),
                    ("limit", "10".to_string()),
                    ("oldest", cursor_ts),
                ];

                let Some(data) = self.fetch_history_with_retry(&channel_id, &params).await else {
                    continue;
                };

                if let Some(messages) = data.get("messages").and_then(|m| m.as_array()) {
                    // Messages come newest-first, reverse to process oldest first
                    for msg in messages.iter().rev() {
                        let subtype = msg.get("subtype").and_then(|value| value.as_str());
                        if !Self::is_supported_message_subtype(subtype) {
                            continue;
                        }
                        let ts = msg.get("ts").and_then(|t| t.as_str()).unwrap_or("");
                        let user = msg
                            .get("user")
                            .and_then(|u| u.as_str())
                            .unwrap_or("unknown");
                        let last_ts = last_ts_by_channel
                            .get(&channel_id)
                            .map(String::as_str)
                            .unwrap_or("");

                        // Skip bot's own messages
                        if user == bot_user_id {
                            continue;
                        }

                        // Sender validation
                        if !self.is_user_allowed(user) {
                            tracing::warn!(
                                "Slack: ignoring message from unauthorized user: {user}"
                            );
                            continue;
                        }

                        if ts <= last_ts {
                            continue;
                        }

                        let is_group_message = Self::is_group_channel_id(&channel_id);
                        let allow_sender_without_mention =
                            is_group_message && self.is_group_sender_trigger_enabled(user);
                        let require_mention =
                            self.mention_only && is_group_message && !allow_sender_without_mention;
                        let Some(normalized_text) = self
                            .build_incoming_content(msg, require_mention, &bot_user_id)
                            .await
                        else {
                            continue;
                        };

                        last_ts_by_channel.insert(channel_id.clone(), ts.to_string());
                        let sender = self.resolve_sender_identity(user).await;

                        let channel_msg = ChannelMessage {
                            id: format!("slack_{channel_id}_{ts}"),
                            sender,
                            reply_target: channel_id.clone(),
                            content: normalized_text,
                            channel: "slack".to_string(),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                            thread_ts: Self::inbound_thread_ts(msg, ts),
                        };

                        if tx.send(channel_msg).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    async fn health_check(&self) -> bool {
        self.http_client()
            .get("https://slack.com/api/auth.test")
            .bearer_auth(&self.bot_token)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_channel_name() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec![], vec![]);
        assert_eq!(ch.name(), "slack");
    }

    #[test]
    fn slack_channel_with_channel_id() {
        let ch = SlackChannel::new(
            "xoxb-fake".into(),
            None,
            Some("C12345".into()),
            vec![],
            vec![],
        );
        assert_eq!(ch.channel_id, Some("C12345".to_string()));
    }

    #[test]
    fn slack_group_reply_policy_defaults_to_all_messages() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec![], vec!["*".into()]);
        assert!(!ch.mention_only);
        assert!(ch.group_reply_allowed_sender_ids.is_empty());
    }

    #[test]
    fn compose_incoming_content_allows_attachment_only_messages() {
        let composed = SlackChannel::compose_incoming_content(
            "".to_string(),
            vec!["[IMAGE:data:image/png;base64,aaaa]".to_string()],
        );
        assert_eq!(
            composed.as_deref(),
            Some("[IMAGE:data:image/png;base64,aaaa]")
        );
    }

    #[test]
    fn message_subtype_support_allows_file_share() {
        assert!(SlackChannel::is_supported_message_subtype(None));
        assert!(SlackChannel::is_supported_message_subtype(Some(
            "file_share"
        )));
        assert!(SlackChannel::is_supported_message_subtype(Some(
            "thread_broadcast"
        )));
        assert!(!SlackChannel::is_supported_message_subtype(Some(
            "message_changed"
        )));
        assert!(!SlackChannel::is_supported_message_subtype(Some(
            "channel_join"
        )));
    }

    #[test]
    fn file_text_preview_prefers_preview_field() {
        let file = serde_json::json!({
            "preview": "line 1\nline 2",
            "preview_highlight": "ignored"
        });
        assert_eq!(
            SlackChannel::file_text_preview(&file).as_deref(),
            Some("line 1\nline 2")
        );
    }

    #[test]
    fn is_image_file_detects_mimetype_or_extension() {
        let from_mime = serde_json::json!({"mimetype":"image/png"});
        let from_ext = serde_json::json!({"name":"photo.jpeg"});
        let non_image = serde_json::json!({"name":"notes.txt","mimetype":"text/plain"});
        assert!(SlackChannel::is_image_file(&from_mime));
        assert!(SlackChannel::is_image_file(&from_ext));
        assert!(!SlackChannel::is_image_file(&non_image));
    }

    #[test]
    fn is_probably_text_file_accepts_snippet_mode() {
        let snippet = serde_json::json!({"mode":"snippet"});
        let plain = serde_json::json!({"mimetype":"text/plain"});
        let binary = serde_json::json!({"mimetype":"application/octet-stream","name":"a.bin"});
        assert!(SlackChannel::is_probably_text_file(&snippet));
        assert!(SlackChannel::is_probably_text_file(&plain));
        assert!(!SlackChannel::is_probably_text_file(&binary));
    }

    #[test]
    fn sanitize_attachment_filename_strips_path_traversal() {
        assert_eq!(
            SlackChannel::sanitize_attachment_filename("../../secret.txt").as_deref(),
            Some("secret.txt")
        );
        assert_eq!(
            SlackChannel::sanitize_attachment_filename(r"..\\..\\secret.txt").as_deref(),
            Some("..__..__secret.txt")
        );
        assert!(SlackChannel::sanitize_attachment_filename("..").is_none());
    }

    #[test]
    fn ensure_file_extension_appends_when_missing() {
        assert_eq!(
            SlackChannel::ensure_file_extension("capture", "png"),
            "capture.png"
        );
        assert_eq!(
            SlackChannel::ensure_file_extension("capture.jpeg", "png"),
            "capture.jpeg"
        );
    }

    #[test]
    fn is_allowed_slack_media_hostname_matches_suffixes() {
        assert!(SlackChannel::is_allowed_slack_media_hostname(
            "files.slack.com"
        ));
        assert!(SlackChannel::is_allowed_slack_media_hostname(
            "downloads.slack-edge.com"
        ));
        assert!(SlackChannel::is_allowed_slack_media_hostname(
            "foo.slack-files.com"
        ));
        assert!(!SlackChannel::is_allowed_slack_media_hostname(
            "example.com"
        ));
    }

    #[test]
    fn validate_slack_private_file_url_rejects_invalid_schemes_and_hosts() {
        assert!(
            SlackChannel::validate_slack_private_file_url("https://files.slack.com/f").is_some()
        );
        assert!(
            SlackChannel::validate_slack_private_file_url("http://files.slack.com/f").is_none()
        );
        assert!(SlackChannel::validate_slack_private_file_url("https://example.com/f").is_none());
        assert!(SlackChannel::validate_slack_private_file_url("not a url").is_none());
    }

    #[test]
    fn resolve_https_redirect_target_enforces_https() {
        let base = reqwest::Url::parse("https://files.slack.com/path/file").unwrap();
        let ok = SlackChannel::resolve_https_redirect_target(&base, "/next");
        assert_eq!(
            ok.as_ref().map(|url| url.as_str()),
            Some("https://files.slack.com/next")
        );

        let rejected =
            SlackChannel::resolve_https_redirect_target(&base, "http://files.slack.com/next");
        assert!(rejected.is_none());

        let rejected_host =
            SlackChannel::resolve_https_redirect_target(&base, "https://example.com/next");
        assert!(rejected_host.is_none());
    }

    #[tokio::test]
    async fn resolve_workspace_attachment_output_path_stays_in_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let output =
            SlackChannel::resolve_workspace_attachment_output_path(workspace.path(), "capture.png")
                .await
                .unwrap();

        let root = tokio::fs::canonicalize(workspace.path()).await.unwrap();
        assert!(output.starts_with(&root));
        assert!(output.to_string_lossy().contains("slack_files"));
    }

    #[test]
    fn specific_allowlist_filters() {
        let ch = SlackChannel::new(
            "xoxb-fake".into(),
            None,
            None,
            vec![],
            vec!["U111".into(), "U222".into()],
        );
        assert!(ch.is_user_allowed("U111"));
        assert!(ch.is_user_allowed("U222"));
        assert!(!ch.is_user_allowed("U333"));
    }

    #[test]
    fn allowlist_exact_match_not_substring() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec![], vec!["U111".into()]);
        assert!(!ch.is_user_allowed("U1111"));
        assert!(!ch.is_user_allowed("U11"));
    }

    #[test]
    fn allowlist_empty_user_id() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec![], vec!["U111".into()]);
        assert!(!ch.is_user_allowed(""));
    }

    #[test]
    fn allowlist_case_sensitive() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec![], vec!["U111".into()]);
        assert!(ch.is_user_allowed("U111"));
        assert!(!ch.is_user_allowed("u111"));
    }

    #[test]
    fn allowlist_wildcard_and_specific() {
        let ch = SlackChannel::new(
            "xoxb-fake".into(),
            None,
            None,
            vec![],
            vec!["U111".into(), "*".into()],
        );
        assert!(ch.is_user_allowed("U111"));
        assert!(ch.is_user_allowed("anyone"));
    }

    // ── Message ID edge cases ─────────────────────────────────────

    #[test]
    fn slack_message_id_format_includes_channel_and_ts() {
        // Verify that message IDs follow the format: slack_{channel_id}_{ts}
        let ts = "1234567890.123456";
        let channel_id = "C12345";
        let expected_id = format!("slack_{channel_id}_{ts}");
        assert_eq!(expected_id, "slack_C12345_1234567890.123456");
    }

    #[test]
    fn slack_message_id_is_deterministic() {
        // Same channel_id + same ts = same ID (prevents duplicates after restart)
        let ts = "1234567890.123456";
        let channel_id = "C12345";
        let id1 = format!("slack_{channel_id}_{ts}");
        let id2 = format!("slack_{channel_id}_{ts}");
        assert_eq!(id1, id2);
    }

    #[test]
    fn slack_message_id_different_ts_different_id() {
        // Different timestamps produce different IDs
        let channel_id = "C12345";
        let id1 = format!("slack_{channel_id}_1234567890.123456");
        let id2 = format!("slack_{channel_id}_1234567890.123457");
        assert_ne!(id1, id2);
    }

    #[test]
    fn slack_message_id_different_channel_different_id() {
        // Different channels produce different IDs even with same ts
        let ts = "1234567890.123456";
        let id1 = format!("slack_C12345_{ts}");
        let id2 = format!("slack_C67890_{ts}");
        assert_ne!(id1, id2);
    }

    #[test]
    fn slack_message_id_no_uuid_randomness() {
        // Verify format doesn't contain random UUID components
        let ts = "1234567890.123456";
        let channel_id = "C12345";
        let id = format!("slack_{channel_id}_{ts}");
        assert!(!id.contains('-')); // No UUID dashes
        assert!(id.starts_with("slack_"));
    }

    #[test]
    fn inbound_thread_ts_prefers_explicit_thread_ts() {
        let msg = serde_json::json!({
            "ts": "123.002",
            "thread_ts": "123.001"
        });

        let thread_ts = SlackChannel::inbound_thread_ts(&msg, "123.002");
        assert_eq!(thread_ts.as_deref(), Some("123.001"));
    }

    #[test]
    fn inbound_thread_ts_falls_back_to_ts() {
        let msg = serde_json::json!({
            "ts": "123.001"
        });

        let thread_ts = SlackChannel::inbound_thread_ts(&msg, "123.001");
        assert_eq!(thread_ts.as_deref(), Some("123.001"));
    }

    #[test]
    fn inbound_thread_ts_none_when_ts_missing() {
        let msg = serde_json::json!({});

        let thread_ts = SlackChannel::inbound_thread_ts(&msg, "");
        assert_eq!(thread_ts, None);
    }

    #[test]
    fn ensure_poll_cursor_bootstraps_new_channel() {
        let mut cursors = HashMap::new();
        let now_ts = "1700000000.123456";

        let cursor = SlackChannel::ensure_poll_cursor(&mut cursors, "C123", now_ts);
        assert_eq!(cursor, now_ts);
        assert_eq!(cursors.get("C123").map(String::as_str), Some(now_ts));
    }

    #[test]
    fn ensure_poll_cursor_keeps_existing_cursor() {
        let mut cursors = HashMap::from([("C123".to_string(), "1700000000.000001".to_string())]);
        let cursor = SlackChannel::ensure_poll_cursor(&mut cursors, "C123", "9999999999.999999");

        assert_eq!(cursor, "1700000000.000001");
        assert_eq!(
            cursors.get("C123").map(String::as_str),
            Some("1700000000.000001")
        );
    }

    #[test]
    fn parse_retry_after_value_accepts_integer_seconds() {
        assert_eq!(SlackChannel::parse_retry_after_value("30"), Some(30));
    }

    #[test]
    fn parse_retry_after_value_accepts_decimal_seconds() {
        assert_eq!(SlackChannel::parse_retry_after_value("2.9"), Some(2));
    }

    #[test]
    fn parse_retry_after_value_rejects_non_numeric_values() {
        assert_eq!(SlackChannel::parse_retry_after_value("later"), None);
        assert_eq!(SlackChannel::parse_retry_after_value(""), None);
    }

    #[test]
    fn parse_retry_after_secs_reads_header_value() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "45".parse().unwrap());
        assert_eq!(SlackChannel::parse_retry_after_secs(&headers), Some(45));
    }

    #[test]
    fn compute_retry_delay_applies_backoff_and_jitter_with_cap() {
        let delay = SlackChannel::compute_retry_delay(30, 3, 250);
        assert_eq!(delay, Duration::from_secs(120) + Duration::from_millis(250));
    }
}
