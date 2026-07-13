use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use backon::BackoffBuilder as _;
use itertools::Itertools as _;

use crate::{
    markup,
    polybar_module::{RenderablePolybarModule, TCP_REMOTE_TIMEOUT, wait_network_ready},
    theme::{self, ICON_WARNING},
};

/// Inference API usage module
pub(crate) struct InferenceUsageModule {
    client: ureq::Agent,
    amp_usage_re: regex::Regex,
    token_path: PathBuf,
    claude_rate_limit_backoff_builder: backon::ExponentialBuilder,
    claude_rate_limit_backoff: backon::ExponentialBackoff,
    claude_skip_until: Option<Instant>,
    claude_auth_failed_mtime: Option<SystemTime>,
    codex_auth_path: PathBuf,
    amp_workdir: tempfile::TempDir,
}

/// Claude usage fetch status
#[derive(Debug, PartialEq)]
pub(crate) enum ClaudeUsageStatus {
    /// Successfully fetched usage data
    Available {
        /// 5-hour utilization percentage
        h5: f64,
        /// 7-day utilization percentage
        d7: f64,
    },
    /// Authentication failed (401), waiting for token refresh
    AuthInvalid,
    /// Generic error
    Error,
}

/// Inference usage state
#[derive(Debug, PartialEq)]
pub(crate) struct InferenceUsageModuleState {
    amp_free_pct: Option<f64>,
    claude_status: ClaudeUsageStatus,
    chatgpt_windows_left: Option<Vec<f64>>,
}

const ICON_INFERENCE_USAGE: &str = "󱩅";
const ICON_AMP: &str = "󰞍";
const ICON_CLAUDE: &str = "";
const ICON_CHATGPT: &str = "󰫈";
const ICON_UNAUTHORIZED: &str = "";
const PROGRESS_ICONS: [&str; 9] = [
    "󰗖", // nf-md-alert_circle_outline
    "󰪞", // nf-md-circle_slice_1
    "󰪟", // nf-md-circle_slice_2
    "󰪠", // nf-md-circle_slice_3
    "󰪡", // nf-md-circle_slice_4
    "󰪢", // nf-md-circle_slice_5
    "󰪣", // nf-md-circle_slice_6
    "󰪤", // nf-md-circle_slice_7
    "󰪥", // nf-md-circle_slice_8
];
const AMP_USAGE_URL: &str = "https://ampcode.com/settings";
const CLAUDE_USAGE_URL: &str = "https://claude.ai/settings/usage";
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/codex/settings/usage";
const CHATGPT_USAGE_API_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.144.1";

enum ClaudeFetchError {
    AuthInvalid,
    RateLimited,
    Other(anyhow::Error),
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeCredentials {
    claude_ai_oauth: ClaudeOauth,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOauth {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    scopes: Vec<String>,
    subscription_type: String,
    rate_limit_tier: String,
}

#[derive(serde::Deserialize)]
struct ClaudeUsageResponse {
    five_hour: ClaudeUsageWindow,
    seven_day: ClaudeUsageWindow,
}

#[derive(serde::Deserialize)]
struct ClaudeUsageWindow {
    utilization: f64,
}

#[derive(serde::Serialize)]
struct ClaudeTokenRequest {
    grant_type: &'static str,
    refresh_token: String,
    client_id: &'static str,
    scope: String,
}

#[derive(serde::Deserialize)]
struct ClaudeTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Debug, thiserror::Error)]
enum ChatGptFetchError {
    #[error("Authentication invalid (401)")]
    AuthInvalid,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(serde::Deserialize)]
struct CodexAuth {
    tokens: CodexTokens,
}

#[derive(serde::Deserialize)]
struct CodexTokens {
    access_token: String,
    account_id: Option<String>,
}

#[derive(serde::Deserialize)]
struct ChatGptUsageResponse {
    rate_limit: ChatGptRateLimit,
}

#[derive(serde::Deserialize)]
struct ChatGptRateLimit {
    primary_window: Option<ChatGptRateLimitWindow>,
    secondary_window: Option<ChatGptRateLimitWindow>,
}

#[derive(serde::Deserialize)]
struct ChatGptRateLimitWindow {
    used_percent: f64,
    limit_window_seconds: u64,
}

#[derive(serde::Serialize)]
struct CodexTokenRequest {
    client_id: &'static str,
    grant_type: &'static str,
    refresh_token: String,
}

#[expect(clippy::struct_field_names)]
#[derive(serde::Deserialize)]
struct CodexTokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

impl InferenceUsageModule {
    pub(crate) fn new() -> Self {
        let client = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .provider(ureq::tls::TlsProvider::NativeTls)
                        .build(),
                )
                .timeout_global(Some(TCP_REMOTE_TIMEOUT))
                .build(),
        );
        let amp_usage_re = regex::Regex::new(r"Amp Free: ([0-9]+\.?[0-9]*)% remaining").unwrap();
        let home = env::var("HOME").unwrap();
        let token_path = PathBuf::from(&home).join(".config/claude/.credentials.json");
        let codex_auth_path = PathBuf::from(&home).join(".config/codex/auth.json");
        let claude_rate_limit_backoff_builder = backon::ExponentialBuilder::default()
            .with_jitter()
            .with_min_delay(Duration::from_mins(5))
            .with_max_delay(Duration::from_hours(1))
            .without_max_times();
        let claude_rate_limit_backoff = claude_rate_limit_backoff_builder.build();
        let amp_workdir = tempfile::tempdir().unwrap();
        Self {
            client,
            amp_usage_re,
            token_path,
            claude_rate_limit_backoff_builder,
            claude_rate_limit_backoff,
            claude_skip_until: None,
            claude_auth_failed_mtime: None,
            codex_auth_path,
            amp_workdir,
        }
    }

    fn fetch_chatgpt_usage(&self) -> Result<Vec<f64>, ChatGptFetchError> {
        let auth_data = fs::read_to_string(&self.codex_auth_path)
            .context("Failed to read codex auth")
            .map_err(ChatGptFetchError::Other)?;
        let auth: CodexAuth =
            serde_json::from_str(&auth_data).map_err(|e| ChatGptFetchError::Other(e.into()))?;

        let mut request = self
            .client
            .get(CHATGPT_USAGE_API_URL)
            .header("User-Agent", CODEX_USER_AGENT)
            .header(
                "Authorization",
                &format!("Bearer {}", auth.tokens.access_token),
            );
        if let Some(account_id) = &auth.tokens.account_id {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let response = request.call().map_err(|error| match error {
            ureq::Error::StatusCode(401) => ChatGptFetchError::AuthInvalid,
            error => ChatGptFetchError::Other(error.into()),
        })?;

        let body: ChatGptUsageResponse = response
            .into_body()
            .read_json()
            .map_err(|e| ChatGptFetchError::Other(e.into()))?;

        Ok(Self::extract_chatgpt_windows(&body.rate_limit))
    }

    /// Remaining percentage of each active rate-limit window, ordered by increasing window duration
    fn extract_chatgpt_windows(rate_limit: &ChatGptRateLimit) -> Vec<f64> {
        [&rate_limit.primary_window, &rate_limit.secondary_window]
            .into_iter()
            .flatten()
            .sorted_by_key(|window| window.limit_window_seconds)
            .map(|window| 100.0 - window.used_percent)
            .collect()
    }

    fn update_chatgpt_usage(&self) -> Option<Vec<f64>> {
        let result = self.fetch_chatgpt_usage().or_else(|error| {
            if !matches!(error, ChatGptFetchError::AuthInvalid) {
                return Err(error);
            }
            log::warn!("ChatGPT usage: authentication invalid (401), attempting token refresh");
            self.refresh_chatgpt_token()?;
            self.fetch_chatgpt_usage()
        });

        match result {
            Ok(windows_left) if !windows_left.is_empty() => Some(windows_left),
            Ok(_) => {
                log::error!("ChatGPT usage: no rate limit windows");
                None
            }
            Err(error) => {
                log::error!("ChatGPT usage: {error}");
                None
            }
        }
    }

    fn refresh_chatgpt_token(&self) -> anyhow::Result<()> {
        let auth_data = fs::read_to_string(&self.codex_auth_path)
            .context("Failed to read codex auth for refresh")?;
        let mut auth: serde_json::Value =
            serde_json::from_str(&auth_data).context("Failed to deserialize codex auth")?;

        let refresh_token = auth
            .get("tokens")
            .and_then(|t| t.get("refresh_token"))
            .and_then(serde_json::Value::as_str)
            .context("Missing refresh_token in codex auth")?;

        let request_body = CodexTokenRequest {
            client_id: CODEX_OAUTH_CLIENT_ID,
            grant_type: "refresh_token",
            refresh_token: refresh_token.to_owned(),
        };

        let tok: CodexTokenResponse = self
            .client
            .post(CODEX_TOKEN_URL)
            .send_json(&request_body)?
            .into_body()
            .read_json()?;

        let tokens = auth
            .get_mut("tokens")
            .and_then(serde_json::Value::as_object_mut)
            .context("Missing tokens object in codex auth")?;
        if let Some(access_token) = tok.access_token {
            tokens.insert("access_token".to_owned(), access_token.into());
        }
        if let Some(new_refresh_token) = tok.refresh_token {
            tokens.insert("refresh_token".to_owned(), new_refresh_token.into());
        }
        if let Some(id_token) = tok.id_token {
            tokens.insert("id_token".to_owned(), id_token.into());
        }

        Self::overwrite_json(&self.codex_auth_path, &auth)
            .context("Failed to write refreshed codex auth")?;

        log::info!("Codex token refreshed");
        Ok(())
    }

    fn fetch_amp_usage(&self) -> anyhow::Result<f64> {
        let output = Command::new("amp")
            .arg("usage")
            .current_dir(self.amp_workdir.path())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .context("Failed to run amp usage")?;
        output.status.exit_ok()?;
        let stdout = String::from_utf8(output.stdout)?;
        Self::parse_amp_usage(&self.amp_usage_re, &stdout)
    }

    /// Parse the free credit percentage from the `Amp Free: N% remaining` line
    fn parse_amp_usage(re: &regex::Regex, usage: &str) -> anyhow::Result<f64> {
        let cap = re
            .captures(usage)
            .ok_or_else(|| anyhow::anyhow!("No Amp Free credit found in amp usage output"))?;
        cap.get(1)
            .unwrap()
            .as_str()
            .parse()
            .context("Failed to parse remaining Amp credit percentage")
    }

    fn claude_token_mtime(&self) -> Option<SystemTime> {
        fs::metadata(&self.token_path)
            .and_then(|m| m.modified())
            .ok()
    }

    fn fetch_claude_usage(&self) -> Result<(f64, f64), ClaudeFetchError> {
        let creds_data = fs::read_to_string(&self.token_path)
            .context("Failed to read credentials")
            .map_err(ClaudeFetchError::Other)?;
        let creds: ClaudeCredentials =
            serde_json::from_str(&creds_data).map_err(|e| ClaudeFetchError::Other(e.into()))?;

        let response = self
            .client
            .get("https://api.anthropic.com/api/oauth/usage")
            .header(
                "Authorization",
                &format!("Bearer {}", creds.claude_ai_oauth.access_token),
            )
            .header("anthropic-beta", "oauth-2025-04-20")
            .call()
            .map_err(|error| match error {
                ureq::Error::StatusCode(401) => ClaudeFetchError::AuthInvalid,
                ureq::Error::StatusCode(429) => ClaudeFetchError::RateLimited,
                error => ClaudeFetchError::Other(error.into()),
            })?;

        let body: ClaudeUsageResponse = response
            .into_body()
            .read_json()
            .map_err(|e| ClaudeFetchError::Other(e.into()))?;

        Ok((body.five_hour.utilization, body.seven_day.utilization))
    }

    fn update_claude_status(&mut self) -> ClaudeUsageStatus {
        // Skip if auth failed and token file unchanged, or if rate-limit backoff active
        if let Some(failed_mtime) = self.claude_auth_failed_mtime
            && self.claude_token_mtime() == Some(failed_mtime)
        {
            log::debug!("Skipping Claude usage: auth invalid, token unchanged");
            return ClaudeUsageStatus::AuthInvalid;
        }
        if self.claude_skip_until.is_some_and(|t| Instant::now() < t) {
            log::debug!("Skipping Claude usage: rate limited");
            return ClaudeUsageStatus::Error;
        }

        // Capture mtime before fetching to avoid a race where a login
        // refreshes the token between our read and the mtime probe
        let pre_fetch_mtime = self.claude_token_mtime();

        match self.fetch_claude_usage() {
            Ok((h5, d7)) => {
                self.claude_auth_failed_mtime = None;
                self.claude_skip_until = None;
                self.claude_rate_limit_backoff = self.claude_rate_limit_backoff_builder.build();
                ClaudeUsageStatus::Available { h5, d7 }
            }
            Err(ClaudeFetchError::AuthInvalid) => {
                log::warn!("Claude usage: authentication invalid (401), attempting token refresh");
                if let Err(e) = self.refresh_claude_token() {
                    log::error!("Claude token refresh failed: {e}");
                    self.claude_auth_failed_mtime = pre_fetch_mtime;
                    return ClaudeUsageStatus::AuthInvalid;
                }
                match self.fetch_claude_usage() {
                    Ok((h5, d7)) => {
                        self.claude_auth_failed_mtime = None;
                        ClaudeUsageStatus::Available { h5, d7 }
                    }
                    Err(ClaudeFetchError::AuthInvalid) => {
                        log::error!("Claude usage still unauthorized after refresh");
                        self.claude_auth_failed_mtime = self.claude_token_mtime();
                        ClaudeUsageStatus::AuthInvalid
                    }
                    Err(ClaudeFetchError::RateLimited) => {
                        log::warn!("Claude usage rate limited after refresh");
                        self.apply_claude_rate_limit_backoff()
                    }
                    Err(ClaudeFetchError::Other(e)) => {
                        log::error!("Claude usage after refresh: {e}");
                        ClaudeUsageStatus::Error
                    }
                }
            }
            Err(ClaudeFetchError::RateLimited) => {
                log::warn!("Claude usage: rate limited");
                self.apply_claude_rate_limit_backoff()
            }
            Err(ClaudeFetchError::Other(e)) => {
                log::error!("Claude usage: {e}");
                ClaudeUsageStatus::Error
            }
        }
    }

    fn apply_claude_rate_limit_backoff(&mut self) -> ClaudeUsageStatus {
        let delay = self.claude_rate_limit_backoff.next().unwrap();
        log::warn!("Claude rate limited, backing off for {delay:?}");
        self.claude_skip_until = Some(Instant::now() + delay);
        ClaudeUsageStatus::Error
    }

    fn refresh_claude_token(&self) -> anyhow::Result<()> {
        let creds_data = fs::read_to_string(&self.token_path)
            .context("Failed to read credentials for refresh")?;
        let mut creds: ClaudeCredentials =
            serde_json::from_str(&creds_data).context("Failed to deserialize credentials")?;

        let request_body = ClaudeTokenRequest {
            grant_type: "refresh_token",
            refresh_token: creds.claude_ai_oauth.refresh_token.clone(),
            client_id: CLAUDE_OAUTH_CLIENT_ID,
            scope: creds.claude_ai_oauth.scopes.join(" "),
        };
        let tok: ClaudeTokenResponse = self
            .client
            .post("https://platform.claude.com/v1/oauth/token")
            .send_json(&request_body)?
            .into_body()
            .read_json()?;

        creds.claude_ai_oauth.access_token = tok.access_token;
        if let Some(new_refresh) = tok.refresh_token {
            creds.claude_ai_oauth.refresh_token = new_refresh;
        }
        #[expect(clippy::cast_possible_truncation)]
        let expires_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + tok.expires_in * 1000;
        creds.claude_ai_oauth.expires_at = expires_at;

        Self::overwrite_json(&self.token_path, &creds)
            .context("Failed to write refreshed credentials")?;

        log::info!(
            "Claude token refreshed, expires in {} seconds",
            tok.expires_in
        );
        Ok(())
    }

    fn render_progress(utilization: f64) -> String {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = utilization.clamp(0.0, 100.0) as usize;
        let icon = if pct == 0 {
            PROGRESS_ICONS[0]
        } else {
            #[expect(clippy::indexing_slicing)]
            PROGRESS_ICONS[1 + (pct - 1) * (PROGRESS_ICONS.len() - 2) / 99]
        };
        let color = if utilization > 30.0 {
            theme::Color::Good
        } else if utilization >= 10.0 {
            theme::Color::Notice
        } else {
            theme::Color::Attention
        };
        markup::Markup::new(icon).fg(color).into_string()
    }

    fn provider_markup<S>(label: &str, usage: S, url: &str) -> markup::Markup
    where
        S: Into<String>,
    {
        markup::Markup::new(format!("{} {}", label, usage.into())).action(
            markup::PolybarActionType::ClickLeft,
            format!("firefox --new-tab '{url}'"),
        )
    }

    /// Serialize `value` to a sibling temporary file and atomically rename it over `path`
    fn overwrite_json<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
        let dir = path
            .parent()
            .with_context(|| format!("Path has no parent directory: {path:?}"))?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut file, value)?;
        file.persist(path)?;
        Ok(())
    }
}

impl RenderablePolybarModule for InferenceUsageModule {
    type State = InferenceUsageModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_mins(3));
        } else {
            wait_network_ready().unwrap();
        }
    }

    fn update(&mut self) -> Self::State {
        let amp_free_pct = match self.fetch_amp_usage() {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("AMP usage: {e}");
                None
            }
        };

        let claude_status = self.update_claude_status();

        let chatgpt_windows_left = self.update_chatgpt_usage();

        InferenceUsageModuleState {
            amp_free_pct,
            claude_status,
            chatgpt_windows_left,
        }
    }

    fn render(&self, state: &Self::State) -> String {
        let mut fragments =
            vec![markup::Markup::new(ICON_INFERENCE_USAGE).fg(theme::Color::MainIcon)];

        match state.amp_free_pct {
            Some(pct) => {
                fragments.push(Self::provider_markup(
                    ICON_AMP,
                    Self::render_progress(pct),
                    AMP_USAGE_URL,
                ));
            }
            None => {
                fragments.push(Self::provider_markup(
                    ICON_AMP,
                    markup::Markup::new(ICON_WARNING).fg(theme::Color::Attention),
                    AMP_USAGE_URL,
                ));
            }
        }

        // Claude
        match &state.claude_status {
            ClaudeUsageStatus::Available { h5, d7 } => {
                fragments.push(Self::provider_markup(
                    ICON_CLAUDE,
                    format!(
                        "{}{}",
                        Self::render_progress(100.0 - h5),
                        Self::render_progress(100.0 - d7),
                    ),
                    CLAUDE_USAGE_URL,
                ));
            }
            ClaudeUsageStatus::AuthInvalid => {
                fragments.push(Self::provider_markup(
                    ICON_CLAUDE,
                    markup::Markup::new(ICON_UNAUTHORIZED),
                    CLAUDE_USAGE_URL,
                ));
            }
            ClaudeUsageStatus::Error => {
                fragments.push(Self::provider_markup(
                    ICON_CLAUDE,
                    markup::Markup::new(ICON_WARNING).fg(theme::Color::Attention),
                    CLAUDE_USAGE_URL,
                ));
            }
        }

        match &state.chatgpt_windows_left {
            Some(windows_left) => {
                let usage: String = windows_left
                    .iter()
                    .map(|pct| Self::render_progress(*pct))
                    .collect();
                fragments.push(Self::provider_markup(
                    ICON_CHATGPT,
                    usage,
                    CHATGPT_USAGE_URL,
                ));
            }
            None => {
                fragments.push(Self::provider_markup(
                    ICON_CHATGPT,
                    markup::Markup::new(ICON_WARNING).fg(theme::Color::Attention),
                    CHATGPT_USAGE_URL,
                ));
            }
        }

        fragments
            .into_iter()
            .map(markup::Markup::into_string)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render_progress() {
        assert_eq!(
            InferenceUsageModule::render_progress(0.0),
            "%{F#d56500}󰗖%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(1.0),
            "%{F#d56500}󰪞%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(5.0),
            "%{F#d56500}󰪞%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(10.0),
            "%{F#ac8300}󰪞%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(20.0),
            "%{F#ac8300}󰪟%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(30.0),
            "%{F#ac8300}󰪠%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(40.0),
            "%{F#819500}󰪠%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(50.0),
            "%{F#819500}󰪡%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(60.0),
            "%{F#819500}󰪢%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(70.0),
            "%{F#819500}󰪢%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(80.0),
            "%{F#819500}󰪣%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(90.0),
            "%{F#819500}󰪤%{F-}"
        );
        assert_eq!(
            InferenceUsageModule::render_progress(100.0),
            "%{F#819500}󰪥%{F-}"
        );
    }

    #[expect(clippy::too_many_lines)]
    #[test]
    fn test_render() {
        let module = InferenceUsageModule::new();

        let mi = |s| {
            markup::Markup::new(s)
                .fg(theme::Color::MainIcon)
                .into_string()
        };
        let provider = |label, usage, url| {
            markup::Markup::new(format!("{label} {usage}"))
                .action(
                    markup::PolybarActionType::ClickLeft,
                    format!("firefox --new-tab '{url}'"),
                )
                .into_string()
        };
        let att_warn = markup::Markup::new(ICON_WARNING)
            .fg(theme::Color::Attention)
            .into_string();

        let state = InferenceUsageModuleState {
            amp_free_pct: Some(45.0),
            claude_status: ClaudeUsageStatus::Available { h5: 50.0, d7: 20.0 },
            chatgpt_windows_left: Some(vec![81.0, 90.0]),
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {} {} {}",
                mi(ICON_INFERENCE_USAGE),
                provider(ICON_AMP, "%{F#819500}󰪡%{F-}", AMP_USAGE_URL,),
                provider(
                    ICON_CLAUDE,
                    "%{F#819500}󰪡%{F-}%{F#819500}󰪣%{F-}",
                    CLAUDE_USAGE_URL,
                ),
                provider(
                    ICON_CHATGPT,
                    "%{F#819500}󰪣%{F-}%{F#819500}󰪤%{F-}",
                    CHATGPT_USAGE_URL,
                ),
            )
        );

        // All errors
        let state = InferenceUsageModuleState {
            amp_free_pct: None,
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_windows_left: None,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {} {} {}",
                mi(ICON_INFERENCE_USAGE),
                provider(ICON_AMP, &att_warn, AMP_USAGE_URL),
                provider(ICON_CLAUDE, &att_warn, CLAUDE_USAGE_URL),
                provider(ICON_CHATGPT, &att_warn, CHATGPT_USAGE_URL),
            )
        );

        let state = InferenceUsageModuleState {
            amp_free_pct: Some(100.0),
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_windows_left: None,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {} {} {}",
                mi(ICON_INFERENCE_USAGE),
                provider(ICON_AMP, "%{F#819500}󰪥%{F-}", AMP_USAGE_URL,),
                provider(ICON_CLAUDE, &att_warn, CLAUDE_USAGE_URL),
                provider(ICON_CHATGPT, &att_warn, CHATGPT_USAGE_URL),
            )
        );

        let state = InferenceUsageModuleState {
            amp_free_pct: Some(5.0),
            claude_status: ClaudeUsageStatus::Available { h5: 5.0, d7: 5.0 },
            chatgpt_windows_left: Some(vec![95.0, 95.0]),
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {} {} {}",
                mi(ICON_INFERENCE_USAGE),
                provider(ICON_AMP, "%{F#d56500}󰪞%{F-}", AMP_USAGE_URL,),
                provider(
                    ICON_CLAUDE,
                    "%{F#819500}󰪤%{F-}%{F#819500}󰪤%{F-}",
                    CLAUDE_USAGE_URL,
                ),
                provider(
                    ICON_CHATGPT,
                    "%{F#819500}󰪤%{F-}%{F#819500}󰪤%{F-}",
                    CHATGPT_USAGE_URL,
                ),
            )
        );

        // Claude auth invalid (401)
        let state = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::AuthInvalid,
            chatgpt_windows_left: Some(vec![20.0, 5.0]),
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {} {} {}",
                mi(ICON_INFERENCE_USAGE),
                provider(ICON_AMP, "%{F#819500}󰪡%{F-}", AMP_USAGE_URL,),
                provider(ICON_CLAUDE, ICON_UNAUTHORIZED, CLAUDE_USAGE_URL),
                provider(
                    ICON_CHATGPT,
                    "%{F#ac8300}󰪟%{F-}%{F#d56500}󰪞%{F-}",
                    CHATGPT_USAGE_URL,
                ),
            )
        );

        // ChatGPT with a single window renders a single icon
        let state = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_windows_left: Some(vec![82.0]),
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {} {} {}",
                mi(ICON_INFERENCE_USAGE),
                provider(ICON_AMP, "%{F#819500}󰪡%{F-}", AMP_USAGE_URL,),
                provider(ICON_CLAUDE, &att_warn, CLAUDE_USAGE_URL),
                provider(ICON_CHATGPT, "%{F#819500}󰪣%{F-}", CHATGPT_USAGE_URL),
            )
        );
    }

    #[test]
    #[expect(clippy::float_cmp)]
    fn test_parse_amp_usage() {
        let module = InferenceUsageModule::new();
        let output = "Signed in as user@example.com (user)
Amp Free: 100% remaining today (resets daily) - https://ampcode.com/settings#amp-free
Individual credits: $5.56 remaining (set up automatic top-up to avoid running out) - https://ampcode.com/settings";
        assert_eq!(
            InferenceUsageModule::parse_amp_usage(&module.amp_usage_re, output).unwrap(),
            100.0
        );

        let output = "Amp Free: 50% remaining today (resets daily)";
        assert_eq!(
            InferenceUsageModule::parse_amp_usage(&module.amp_usage_re, output).unwrap(),
            50.0
        );
    }

    #[test]
    fn test_extract_chatgpt_windows_single() {
        let body = r#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":18,"limit_window_seconds":604800,"reset_after_seconds":567359,"reset_at":1784539045},"secondary_window":null}}"#;
        let resp: ChatGptUsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            InferenceUsageModule::extract_chatgpt_windows(&resp.rate_limit),
            vec![82.0]
        );
    }

    #[test]
    fn test_extract_chatgpt_windows_both_sorted_by_duration() {
        // Backend lists the weekly window first; output must be ordered by increasing duration
        let body = r#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_after_seconds":1,"reset_at":1},"secondary_window":{"used_percent":19,"limit_window_seconds":18000,"reset_after_seconds":1,"reset_at":1}}}"#;
        let resp: ChatGptUsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            InferenceUsageModule::extract_chatgpt_windows(&resp.rate_limit),
            vec![81.0, 90.0]
        );
    }

    #[test]
    fn test_extract_chatgpt_windows_none() {
        let body = r#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#;
        let resp: ChatGptUsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            InferenceUsageModule::extract_chatgpt_windows(&resp.rate_limit),
            Vec::<f64>::new()
        );
    }
}
