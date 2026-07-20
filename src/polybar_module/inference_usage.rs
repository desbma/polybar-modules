use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use backon::BackoffBuilder as _;
use chrono::{DateTime, Utc};
use itertools::Itertools as _;

use crate::{
    markup,
    polybar_module::{
        RenderablePolybarModule, TCP_REMOTE_TIMEOUT, sleep_suspend_aware, wait_network_ready,
    },
    theme::{self, ICON_WARNING},
};

/// Inference API usage module
pub(crate) struct InferenceUsageModule {
    client: ureq::Agent,
    amp_usage_re: regex::Regex,
    token_path: PathBuf,
    claude_rate_limit: RateLimitBackoff,
    chatgpt_rate_limit: RateLimitBackoff,
    claude_auth_failed_mtime: Option<SystemTime>,
    codex_auth_path: PathBuf,
    amp_workdir: tempfile::TempDir,
    degraded_backoff: backon::ExponentialBackoff,
    /// Start of the current run of degraded updates
    degraded_since: Option<SystemTime>,
    /// Last state in which every provider reported its usage
    last_complete_state: Option<InferenceUsageModuleState>,
}

/// Escalating delay leaving a provider alone after it answered with a rate limit
///
/// Deadlines are on the wall clock rather than a monotonic one, so that a suspend counts towards
/// them: the throttling they wait out is the provider's, and it expires while we are asleep.
struct RateLimitBackoff {
    backoff: backon::ExponentialBackoff,
    skip_until: Option<SystemTime>,
}

impl RateLimitBackoff {
    fn new() -> Self {
        Self {
            backoff: RATE_LIMIT_BACKOFF.build(),
            skip_until: None,
        }
    }

    /// Whether requests are currently held back
    fn active(&self) -> bool {
        self.skip_until.is_some_and(|t| SystemTime::now() < t)
    }

    /// Hold requests back for the next delay, and return it
    fn hit(&mut self) -> Duration {
        let delay = self.backoff.next().unwrap();
        self.skip_until = Some(SystemTime::now() + delay);
        delay
    }

    fn reset(&mut self) {
        self.skip_until = None;
        self.backoff = RATE_LIMIT_BACKOFF.build();
    }
}

/// Usage of a single rate limit window
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageWindow {
    quota_left_pct: f64,
    /// Share of the window duration left before it resets, `None` if the window is not running
    time_left_frac: Option<f64>,
}

/// Claude usage fetch status
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ClaudeUsageStatus {
    /// Successfully fetched usage data
    Available {
        /// 5-hour window
        h5: UsageWindow,
        /// 7-day window
        d7: UsageWindow,
    },
    /// Authentication failed (401), waiting for token refresh
    AuthInvalid,
    /// Generic error
    Error,
}

/// Inference usage state
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InferenceUsageModuleState {
    amp_free_pct: Option<f64>,
    claude_status: ClaudeUsageStatus,
    chatgpt_windows: Option<Vec<UsageWindow>>,
}

impl InferenceUsageModuleState {
    /// Whether some provider usage is missing
    fn is_degraded(&self) -> bool {
        self.amp_free_pct.is_none()
            || self.chatgpt_windows.is_none()
            || !matches!(self.claude_status, ClaudeUsageStatus::Available { .. })
    }
}

const ICON_INFERENCE_USAGE: &str = "󱩅";
const ICON_AMP: &str = "󰞍";
const ICON_CLAUDE: &str = "";
const ICON_CHATGPT: &str = "󰫈";
const ICON_UNAUTHORIZED: &str = "";
const QUOTA_ICONS: [&str; 9] = [
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
/// Duration of the Claude short rolling window
const CLAUDE_H5_WINDOW: Duration = Duration::from_hours(5);
/// Duration of the Claude long rolling window
const CLAUDE_D7_WINDOW: Duration = Duration::from_hours(7 * 24);
const AMP_USAGE_URL: &str = "https://ampcode.com/settings";
const CLAUDE_USAGE_URL: &str = "https://claude.ai/settings/usage";
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/codex/settings/usage";
const CHATGPT_USAGE_API_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.144.6";
/// Delay between updates while at least one provider is reachable
const UPDATE_INTERVAL: Duration = Duration::from_mins(3);
/// Timeout of the resolve and connect phases
///
/// Bounds how long an update takes to give up when the network is down, without shortening the
/// global timeout granted to a server that does answer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Duration the last complete usage stays displayed while an update is degraded
const DEGRADED_HOLD: Duration = Duration::from_mins(1);
/// Shortest delay between updates while retrying through a degraded update
const DEGRADED_MIN_DELAY: Duration = Duration::from_secs(3);
/// Shortest delay a provider is left alone after answering with a rate limit, before jitter
const RATE_LIMIT_MIN_DELAY: Duration = Duration::from_mins(5);
/// Longest delay a provider is left alone after answering with a rate limit, before jitter
///
/// Jitter stretches an emitted delay by up to as much again, so it is a ceiling on the escalation
/// rather than on the wait itself.
const RATE_LIMIT_MAX_DELAY: Duration = Duration::from_hours(1);
/// Escalation curve of the delay a rate limited provider is left alone for
const RATE_LIMIT_BACKOFF: backon::ExponentialBuilder = backon::ExponentialBuilder::new()
    .with_jitter()
    .with_min_delay(RATE_LIMIT_MIN_DELAY)
    .with_max_delay(RATE_LIMIT_MAX_DELAY)
    .without_max_times();
/// Escalation curve of the delay between updates while retrying through a degraded update
const DEGRADED_BACKOFF: backon::ExponentialBuilder = backon::ExponentialBuilder::new()
    .with_jitter()
    .with_factor(1.5)
    .with_min_delay(DEGRADED_MIN_DELAY)
    .with_max_delay(UPDATE_INTERVAL)
    .without_max_times();

/// Failure of a provider usage fetch
#[derive(Debug, thiserror::Error)]
enum ProviderFetchError {
    #[error("Authentication invalid")]
    AuthInvalid,
    #[error("Rate limited (429)")]
    RateLimited,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ProviderFetchError {
    /// Classify a Claude token refresh failure
    ///
    /// A request the server turned down leaves nothing to retry, the stored credentials are spent.
    /// Anything else may well resolve on its own, and says nothing about them.
    fn from_claude_refresh(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(429) => Self::RateLimited,
            ureq::Error::StatusCode(400..500) => Self::AuthInvalid,
            error => Self::Other(error.into()),
        }
    }
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
    /// Null while no window is running, ie. nothing was consumed since the last reset
    resets_at: Option<DateTime<Utc>>,
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
    reset_after_seconds: u64,
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
                .timeout_resolve(Some(CONNECT_TIMEOUT))
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .build(),
        );
        let amp_usage_re = regex::Regex::new(r"Amp Free: ([0-9]+\.?[0-9]*)% remaining").unwrap();
        let home = env::var("HOME").unwrap();
        let token_path = PathBuf::from(&home).join(".config/claude/.credentials.json");
        let codex_auth_path = PathBuf::from(&home).join(".config/codex/auth.json");
        let amp_workdir = tempfile::tempdir().unwrap();
        Self {
            client,
            amp_usage_re,
            token_path,
            claude_rate_limit: RateLimitBackoff::new(),
            chatgpt_rate_limit: RateLimitBackoff::new(),
            claude_auth_failed_mtime: None,
            codex_auth_path,
            amp_workdir,
            degraded_backoff: DEGRADED_BACKOFF.build(),
            degraded_since: None,
            last_complete_state: None,
        }
    }

    /// Delay before the next update, shortened while retrying through a degraded update
    ///
    /// A provider throttling us is the one thing worth waiting out, so it holds the update back to
    /// its nominal interval.
    fn next_delay(&mut self) -> Duration {
        let delay = if self.degraded_since.is_some() && !self.rate_limited() {
            self.degraded_backoff.next().unwrap()
        } else {
            self.degraded_backoff = DEGRADED_BACKOFF.build();
            UPDATE_INTERVAL
        };
        // Wake up in time to drop a held state the moment it gets too stale, which a rate limit
        // would otherwise postpone to the nominal interval
        self.degraded_since
            .filter(|_| self.last_complete_state.is_some())
            .and_then(Self::degraded_hold_left)
            .map_or(delay, |left| delay.min(left).max(DEGRADED_MIN_DELAY))
    }

    /// Time left before the last complete state gets too stale to display, `None` once it is
    fn degraded_hold_left(since: SystemTime) -> Option<Duration> {
        DEGRADED_HOLD.checked_sub(since.elapsed().ok()?)
    }

    /// Whether a provider is currently holding us back
    fn rate_limited(&self) -> bool {
        self.claude_rate_limit.active() || self.chatgpt_rate_limit.active()
    }

    fn fetch_chatgpt_usage(&self) -> Result<Vec<UsageWindow>, ProviderFetchError> {
        let auth_data = fs::read_to_string(&self.codex_auth_path)
            .context("Failed to read codex auth")
            .map_err(ProviderFetchError::Other)?;
        let auth: CodexAuth =
            serde_json::from_str(&auth_data).map_err(|e| ProviderFetchError::Other(e.into()))?;

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
        // An exhausted quota is reported in the body, not by an error code
        let response = request.call().map_err(|error| match error {
            ureq::Error::StatusCode(401) => ProviderFetchError::AuthInvalid,
            ureq::Error::StatusCode(429) => ProviderFetchError::RateLimited,
            error => ProviderFetchError::Other(error.into()),
        })?;

        let body: ChatGptUsageResponse = response
            .into_body()
            .read_json()
            .map_err(|e| ProviderFetchError::Other(e.into()))?;

        Ok(Self::extract_chatgpt_windows(&body.rate_limit))
    }

    /// Each active rate-limit window, ordered by increasing window duration
    #[expect(clippy::cast_precision_loss)]
    fn extract_chatgpt_windows(rate_limit: &ChatGptRateLimit) -> Vec<UsageWindow> {
        [&rate_limit.primary_window, &rate_limit.secondary_window]
            .into_iter()
            .flatten()
            .sorted_by_key(|window| window.limit_window_seconds)
            .map(|window| UsageWindow {
                quota_left_pct: 100.0 - window.used_percent,
                time_left_frac: Some(
                    (window.reset_after_seconds as f64 / window.limit_window_seconds as f64)
                        .clamp(0.0, 1.0),
                ),
            })
            .collect()
    }

    fn update_chatgpt_usage(&mut self) -> Option<Vec<UsageWindow>> {
        if self.chatgpt_rate_limit.active() {
            log::debug!("Skipping ChatGPT usage: rate limited");
            return None;
        }

        let result = self.fetch_chatgpt_usage().or_else(|error| {
            if !matches!(error, ProviderFetchError::AuthInvalid) {
                return Err(error);
            }
            log::warn!("ChatGPT usage: authentication invalid (401), attempting token refresh");
            self.refresh_chatgpt_token()?;
            self.fetch_chatgpt_usage()
        });

        match result {
            Ok(windows) => {
                self.chatgpt_rate_limit.reset();
                if windows.is_empty() {
                    log::error!("ChatGPT usage: no rate limit windows");
                    None
                } else {
                    Some(windows)
                }
            }
            Err(ProviderFetchError::RateLimited) => {
                let delay = self.chatgpt_rate_limit.hit();
                log::warn!("ChatGPT usage: rate limited, backing off for {delay:?}");
                None
            }
            Err(error) => {
                log::error!("ChatGPT usage: {error}");
                None
            }
        }
    }

    fn refresh_chatgpt_token(&self) -> Result<(), ProviderFetchError> {
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
            .send_json(&request_body)
            .map_err(|error| match error {
                ureq::Error::StatusCode(429) => ProviderFetchError::RateLimited,
                error => ProviderFetchError::Other(error.into()),
            })?
            .into_body()
            .read_json()
            .map_err(|e| ProviderFetchError::Other(e.into()))?;

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

    /// Quota left and share of `window_len` remaining before `window` resets
    fn claude_window(
        window: &ClaudeUsageWindow,
        window_len: Duration,
        now: DateTime<Utc>,
    ) -> UsageWindow {
        UsageWindow {
            quota_left_pct: 100.0 - window.utilization,
            time_left_frac: window.resets_at.map(|resets_at| {
                (resets_at - now)
                    .to_std()
                    .unwrap_or_default()
                    .div_duration_f64(window_len)
                    .clamp(0.0, 1.0)
            }),
        }
    }

    fn fetch_claude_usage(&self) -> Result<(UsageWindow, UsageWindow), ProviderFetchError> {
        let creds_data = fs::read_to_string(&self.token_path)
            .context("Failed to read credentials")
            .map_err(ProviderFetchError::Other)?;
        let creds: ClaudeCredentials =
            serde_json::from_str(&creds_data).map_err(|e| ProviderFetchError::Other(e.into()))?;

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
                ureq::Error::StatusCode(401) => ProviderFetchError::AuthInvalid,
                ureq::Error::StatusCode(429) => ProviderFetchError::RateLimited,
                error => ProviderFetchError::Other(error.into()),
            })?;

        let body: ClaudeUsageResponse = response
            .into_body()
            .read_json()
            .map_err(|e| ProviderFetchError::Other(e.into()))?;

        let now = Utc::now();
        Ok((
            Self::claude_window(&body.five_hour, CLAUDE_H5_WINDOW, now),
            Self::claude_window(&body.seven_day, CLAUDE_D7_WINDOW, now),
        ))
    }

    fn update_claude_status(&mut self) -> ClaudeUsageStatus {
        // Skip if auth failed and token file unchanged, or if rate-limit backoff active
        if let Some(failed_mtime) = self.claude_auth_failed_mtime
            && self.claude_token_mtime() == Some(failed_mtime)
        {
            log::debug!("Skipping Claude usage: auth invalid, token unchanged");
            return ClaudeUsageStatus::AuthInvalid;
        }
        if self.claude_rate_limit.active() {
            log::debug!("Skipping Claude usage: rate limited");
            return ClaudeUsageStatus::Error;
        }

        // Mtime of the credentials the failing request used, captured before each attempt to avoid
        // a race where a login rewrites them between our read and the mtime probe
        let mut tried_creds_mtime = self.claude_token_mtime();

        let result = self.fetch_claude_usage().or_else(|error| {
            if !matches!(error, ProviderFetchError::AuthInvalid) {
                return Err(error);
            }
            log::warn!("Claude usage: authentication invalid (401), attempting token refresh");
            self.refresh_claude_token()?;
            tried_creds_mtime = self.claude_token_mtime();
            self.fetch_claude_usage()
        });

        match result {
            Ok((h5, d7)) => {
                self.claude_auth_failed_mtime = None;
                self.claude_rate_limit.reset();
                ClaudeUsageStatus::Available { h5, d7 }
            }
            Err(ProviderFetchError::AuthInvalid) => {
                log::error!(
                    "Claude usage: authentication invalid, waiting for a credentials change"
                );
                self.claude_auth_failed_mtime = tried_creds_mtime;
                ClaudeUsageStatus::AuthInvalid
            }
            Err(ProviderFetchError::RateLimited) => {
                let delay = self.claude_rate_limit.hit();
                log::warn!("Claude usage: rate limited, backing off for {delay:?}");
                ClaudeUsageStatus::Error
            }
            Err(ProviderFetchError::Other(error)) => {
                log::error!("Claude usage: {error}");
                ClaudeUsageStatus::Error
            }
        }
    }

    fn refresh_claude_token(&self) -> Result<(), ProviderFetchError> {
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
            .send_json(&request_body)
            .map_err(ProviderFetchError::from_claude_refresh)?
            .into_body()
            .read_json()
            .map_err(|e| ProviderFetchError::Other(e.into()))?;

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

    fn quota_color(quota_left_pct: f64) -> theme::Color {
        if quota_left_pct > 30.0 {
            theme::Color::Good
        } else if quota_left_pct >= 10.0 {
            theme::Color::Notice
        } else {
            theme::Color::Attention
        }
    }

    fn render_quota(quota_left_pct: f64) -> String {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = quota_left_pct.clamp(0.0, 100.0) as usize;
        let icon = if pct == 0 {
            QUOTA_ICONS[0]
        } else {
            #[expect(clippy::indexing_slicing)]
            QUOTA_ICONS[1 + (pct - 1) * (QUOTA_ICONS.len() - 2) / 99]
        };
        markup::Markup::new(icon)
            .fg(Self::quota_color(quota_left_pct))
            .into_string()
    }

    /// Render each window quota, followed by the time left before reset for each running window
    fn render_windows<'a, I>(windows: I) -> String
    where
        I: IntoIterator<Item = &'a UsageWindow>,
    {
        windows
            .into_iter()
            .map(|window| {
                let mut quota = Self::render_quota(window.quota_left_pct);
                if let Some(time_left_frac) = window.time_left_frac {
                    quota +=
                        &markup::ramp(time_left_frac, Self::quota_color(window.quota_left_pct));
                }
                quota
            })
            .collect()
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
            sleep_suspend_aware(self.next_delay());
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

        let chatgpt_windows = self.update_chatgpt_usage();

        let state = InferenceUsageModuleState {
            amp_free_pct,
            claude_status,
            chatgpt_windows,
        };

        if !state.is_degraded() {
            self.degraded_since = None;
            self.last_complete_state = Some(state.clone());
            return state;
        }

        // Resuming from suspend leaves the network unusable for a few seconds; keep showing the
        // last complete usage instead of flashing an error, until it gets too stale to be trusted
        let since = *self.degraded_since.get_or_insert_with(SystemTime::now);
        match &self.last_complete_state {
            Some(last) if Self::degraded_hold_left(since).is_some() => {
                log::warn!("Update degraded, holding last complete usage");
                last.clone()
            }
            _ => state,
        }
    }

    fn render(&self, state: &Self::State) -> String {
        let warning = || {
            markup::Markup::new(ICON_WARNING)
                .fg(theme::Color::Attention)
                .into_string()
        };
        let amp = state.amp_free_pct.map_or_else(warning, Self::render_quota);
        let claude = match &state.claude_status {
            ClaudeUsageStatus::Available { h5, d7 } => Self::render_windows([h5, d7]),
            ClaudeUsageStatus::AuthInvalid => ICON_UNAUTHORIZED.to_owned(),
            ClaudeUsageStatus::Error => warning(),
        };
        let chatgpt = state
            .chatgpt_windows
            .as_ref()
            .map_or_else(warning, Self::render_windows);

        [
            markup::Markup::new(ICON_INFERENCE_USAGE).fg(theme::Color::MainIcon),
            Self::provider_markup(ICON_AMP, amp, AMP_USAGE_URL),
            Self::provider_markup(ICON_CLAUDE, claude, CLAUDE_USAGE_URL),
            Self::provider_markup(ICON_CHATGPT, chatgpt, CHATGPT_USAGE_URL),
        ]
        .into_iter()
        .map(markup::Markup::into_string)
        .collect::<Vec<_>>()
        .join(" ")
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use std::iter;

    use super::*;

    /// Slack absorbing the time a test itself takes
    const MARGIN: Duration = Duration::from_secs(1);

    #[test]
    fn test_render_quota() {
        for (quota_left_pct, expected) in [
            (0.0, "%{F#d56500}󰗖%{F-}"),
            (1.0, "%{F#d56500}󰪞%{F-}"),
            (5.0, "%{F#d56500}󰪞%{F-}"),
            (10.0, "%{F#ac8300}󰪞%{F-}"),
            (20.0, "%{F#ac8300}󰪟%{F-}"),
            (30.0, "%{F#ac8300}󰪠%{F-}"),
            (40.0, "%{F#819500}󰪠%{F-}"),
            (50.0, "%{F#819500}󰪡%{F-}"),
            (60.0, "%{F#819500}󰪢%{F-}"),
            (70.0, "%{F#819500}󰪢%{F-}"),
            (80.0, "%{F#819500}󰪣%{F-}"),
            (90.0, "%{F#819500}󰪤%{F-}"),
            (100.0, "%{F#819500}󰪥%{F-}"),
        ] {
            assert_eq!(InferenceUsageModule::render_quota(quota_left_pct), expected);
        }
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
        let provider = |label: &str, usage: &str, url: &str| {
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
        let w = |quota_left_pct, time_left_frac| UsageWindow {
            quota_left_pct,
            time_left_frac: Some(time_left_frac),
        };
        let assert_render = |state: &InferenceUsageModuleState,
                             [amp, claude, chatgpt]: [&str; 3]| {
            assert_eq!(
                module.render(state),
                [
                    mi(ICON_INFERENCE_USAGE),
                    provider(ICON_AMP, amp, AMP_USAGE_URL),
                    provider(ICON_CLAUDE, claude, CLAUDE_USAGE_URL),
                    provider(ICON_CHATGPT, chatgpt, CHATGPT_USAGE_URL),
                ]
                .join(" ")
            );
        };

        let state = InferenceUsageModuleState {
            amp_free_pct: Some(45.0),
            claude_status: ClaudeUsageStatus::Available {
                h5: w(50.0, 0.75),
                d7: w(80.0, 0.9),
            },
            chatgpt_windows: Some(vec![w(81.0, 0.5), w(90.0, 1.0)]),
        };
        assert_render(
            &state,
            [
                "%{F#819500}󰪡%{F-}",
                "%{F#819500}󰪡%{F-}%{F#819500}▆%{F-}%{F#819500}󰪣%{F-}%{F#819500}█%{F-}",
                "%{F#819500}󰪣%{F-}%{F#819500}▄%{F-}%{F#819500}󰪤%{F-}%{F#819500}█%{F-}",
            ],
        );

        // All errors
        let state = InferenceUsageModuleState {
            amp_free_pct: None,
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_windows: None,
        };
        assert_render(&state, [&att_warn, &att_warn, &att_warn]);

        let state = InferenceUsageModuleState {
            amp_free_pct: Some(100.0),
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_windows: None,
        };
        assert_render(&state, ["%{F#819500}󰪥%{F-}", &att_warn, &att_warn]);

        let state = InferenceUsageModuleState {
            amp_free_pct: Some(5.0),
            claude_status: ClaudeUsageStatus::Available {
                h5: w(95.0, 0.125),
                d7: w(95.0, 0.4),
            },
            chatgpt_windows: Some(vec![w(95.0, 0.0), w(95.0, 0.6)]),
        };
        assert_render(
            &state,
            [
                "%{F#d56500}󰪞%{F-}",
                "%{F#819500}󰪤%{F-}%{F#819500}▁%{F-}%{F#819500}󰪤%{F-}%{F#819500}▄%{F-}",
                "%{F#819500}󰪤%{F-}%{F#819500}▁%{F-}%{F#819500}󰪤%{F-}%{F#819500}▅%{F-}",
            ],
        );

        // Claude auth invalid (401)
        let state = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::AuthInvalid,
            chatgpt_windows: Some(vec![w(20.0, 0.3), w(5.0, 0.8)]),
        };
        assert_render(
            &state,
            [
                "%{F#819500}󰪡%{F-}",
                ICON_UNAUTHORIZED,
                "%{F#ac8300}󰪟%{F-}%{F#ac8300}▃%{F-}%{F#d56500}󰪞%{F-}%{F#d56500}▇%{F-}",
            ],
        );

        // Claude 5h window not running yet: full quota, no reset bar
        let state = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::Available {
                h5: UsageWindow {
                    quota_left_pct: 100.0,
                    time_left_frac: None,
                },
                d7: w(80.0, 0.9),
            },
            chatgpt_windows: None,
        };
        assert_render(
            &state,
            [
                "%{F#819500}󰪡%{F-}",
                "%{F#819500}󰪥%{F-}%{F#819500}󰪣%{F-}%{F#819500}█%{F-}",
                &att_warn,
            ],
        );

        // ChatGPT with a single window renders a single quota icon, still with its reset bar
        let state = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_windows: Some(vec![w(82.0, 1.0)]),
        };
        assert_render(
            &state,
            [
                "%{F#819500}󰪡%{F-}",
                &att_warn,
                "%{F#819500}󰪣%{F-}%{F#819500}█%{F-}",
            ],
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
    fn test_claude_window() {
        let body = r#"{"utilization":12.0,"resets_at":"2026-05-14T19:40:00+00:00"}"#;
        let window: ClaudeUsageWindow = serde_json::from_str(body).unwrap();
        let now = "2026-05-14T17:10:00+00:00"
            .parse::<DateTime<Utc>>()
            .unwrap();
        assert_eq!(
            InferenceUsageModule::claude_window(&window, Duration::from_hours(5), now),
            UsageWindow {
                quota_left_pct: 88.0,
                time_left_frac: Some(0.5),
            }
        );
    }

    #[test]
    fn test_claude_window_past_reset() {
        let body = r#"{"utilization":0.0,"resets_at":"2026-05-14T19:40:00+00:00"}"#;
        let window: ClaudeUsageWindow = serde_json::from_str(body).unwrap();
        let now = "2026-05-15T00:00:00+00:00"
            .parse::<DateTime<Utc>>()
            .unwrap();
        assert_eq!(
            InferenceUsageModule::claude_window(&window, Duration::from_hours(5), now),
            UsageWindow {
                quota_left_pct: 100.0,
                time_left_frac: Some(0.0),
            }
        );
    }

    #[test]
    fn test_claude_window_no_active_window() {
        let body = r#"{"utilization":0.0,"resets_at":null}"#;
        let window: ClaudeUsageWindow = serde_json::from_str(body).unwrap();
        let now = "2026-05-14T17:10:00+00:00"
            .parse::<DateTime<Utc>>()
            .unwrap();
        assert_eq!(
            InferenceUsageModule::claude_window(&window, Duration::from_hours(5), now),
            UsageWindow {
                quota_left_pct: 100.0,
                time_left_frac: None,
            }
        );
    }

    #[test]
    fn test_extract_chatgpt_windows_single() {
        let body = r#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":18,"limit_window_seconds":604800,"reset_after_seconds":567359,"reset_at":1784539045},"secondary_window":null}}"#;
        let resp: ChatGptUsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            InferenceUsageModule::extract_chatgpt_windows(&resp.rate_limit),
            vec![UsageWindow {
                quota_left_pct: 82.0,
                time_left_frac: Some(567_359.0 / 604_800.0),
            }]
        );
    }

    #[test]
    fn test_extract_chatgpt_windows_both_sorted_by_duration() {
        // Backend lists the weekly window first; output must be ordered by increasing duration
        let body = r#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_after_seconds":302400,"reset_at":1},"secondary_window":{"used_percent":19,"limit_window_seconds":18000,"reset_after_seconds":4500,"reset_at":1}}}"#;
        let resp: ChatGptUsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            InferenceUsageModule::extract_chatgpt_windows(&resp.rate_limit),
            vec![
                UsageWindow {
                    quota_left_pct: 81.0,
                    time_left_frac: Some(0.25),
                },
                UsageWindow {
                    quota_left_pct: 90.0,
                    time_left_frac: Some(0.5),
                },
            ]
        );
    }

    #[test]
    fn test_is_degraded() {
        let w = || UsageWindow {
            quota_left_pct: 50.0,
            time_left_frac: Some(0.5),
        };
        let complete = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::Available { h5: w(), d7: w() },
            chatgpt_windows: Some(vec![w()]),
        };
        assert!(!complete.is_degraded());

        for state in [
            InferenceUsageModuleState {
                amp_free_pct: None,
                ..complete.clone()
            },
            InferenceUsageModuleState {
                claude_status: ClaudeUsageStatus::Error,
                ..complete.clone()
            },
            InferenceUsageModuleState {
                claude_status: ClaudeUsageStatus::AuthInvalid,
                ..complete.clone()
            },
            InferenceUsageModuleState {
                chatgpt_windows: None,
                ..complete.clone()
            },
        ] {
            assert!(state.is_degraded());
        }
    }

    #[test]
    fn test_next_delay_shortened_while_degraded() {
        // Jitter can double each computed delay
        let base_range = DEGRADED_MIN_DELAY..(2 * DEGRADED_MIN_DELAY);
        let mut module = InferenceUsageModule::new();
        assert_eq!(module.next_delay(), UPDATE_INTERVAL);

        module.degraded_since = Some(SystemTime::now());
        assert!(base_range.contains(&module.next_delay()));
        // Grows, then converges back to the nominal interval instead of drifting further away
        let grown = iter::repeat_with(|| module.next_delay()).nth(4).unwrap();
        assert!(grown > base_range.end);
        let capped = iter::repeat_with(|| module.next_delay()).nth(19).unwrap();
        assert!((UPDATE_INTERVAL..(2 * UPDATE_INTERVAL)).contains(&capped));

        // Complete again: back to the nominal interval, and the backoff restarts from its floor
        module.degraded_since = None;
        assert_eq!(module.next_delay(), UPDATE_INTERVAL);
        module.degraded_since = Some(SystemTime::now());
        assert!(base_range.contains(&module.next_delay()));
    }

    #[test]
    fn test_next_delay_nominal_while_rate_limited() {
        // Either provider throttling us holds the update back, they answer 429 for the same reason
        for claude in [true, false] {
            let mut module = InferenceUsageModule::new();
            module.degraded_since = Some(SystemTime::now());
            if claude {
                module.claude_rate_limit.hit();
            } else {
                module.chatgpt_rate_limit.hit();
            }
            assert_eq!(module.next_delay(), UPDATE_INTERVAL);
            assert_eq!(module.next_delay(), UPDATE_INTERVAL);

            // Once it stops, the backoff resumes from its floor rather than mid-curve
            if claude {
                module.claude_rate_limit.reset();
            } else {
                module.chatgpt_rate_limit.reset();
            }
            assert!((DEGRADED_MIN_DELAY..(2 * DEGRADED_MIN_DELAY)).contains(&module.next_delay()));
        }
    }

    #[test]
    fn test_next_delay_capped_by_degraded_hold() {
        let w = UsageWindow {
            quota_left_pct: 50.0,
            time_left_frac: Some(0.5),
        };
        let complete = InferenceUsageModuleState {
            amp_free_pct: Some(50.0),
            claude_status: ClaudeUsageStatus::Available {
                h5: w.clone(),
                d7: w.clone(),
            },
            chatgpt_windows: Some(vec![w]),
        };
        let mut module = InferenceUsageModule::new();
        module.degraded_since = Some(SystemTime::now());
        module.last_complete_state = Some(complete.clone());

        // A rate limit stretches the interval past the hold, the held state still expires on time
        module.claude_rate_limit.hit();
        assert!(module.next_delay() <= DEGRADED_HOLD);

        // A hold about to run out does not collapse the delay into a busy retry
        module.degraded_since =
            SystemTime::now().checked_sub(DEGRADED_HOLD.checked_sub(MARGIN).unwrap());
        assert_eq!(module.next_delay(), DEGRADED_MIN_DELAY);

        // Once the hold has run out there is nothing left to expire early
        module.degraded_since = SystemTime::now().checked_sub(2 * DEGRADED_HOLD);
        assert_eq!(module.next_delay(), UPDATE_INTERVAL);

        // Neither is there when no complete state is being held
        module.degraded_since = Some(SystemTime::now());
        module.last_complete_state = None;
        assert_eq!(module.next_delay(), UPDATE_INTERVAL);

        // An escalated retry is capped too, its ceiling is above the hold even without a rate limit
        let mut ordinary = InferenceUsageModule::new();
        ordinary.degraded_since = SystemTime::now().checked_sub(DEGRADED_HOLD / 2);
        ordinary.last_complete_state = Some(complete);
        let delay = iter::repeat_with(|| ordinary.next_delay()).nth(19).unwrap();
        let hold_left = DEGRADED_HOLD / 2;
        assert!((hold_left.checked_sub(MARGIN).unwrap()..=hold_left).contains(&delay));
    }

    #[test]
    fn test_degraded_hold_left_none_when_clock_moves_backwards() {
        // A hold starting in the future has an unknown age, it does not get to count as fresh
        let future_start = SystemTime::now() + UPDATE_INTERVAL;
        assert_eq!(InferenceUsageModule::degraded_hold_left(future_start), None);
    }

    #[test]
    fn test_provider_fetch_error_from_claude_refresh() {
        // Whichever way the server words it, a rejected refresh means the credentials are spent
        for status in (400..500).filter(|status| *status != 429) {
            assert!(matches!(
                ProviderFetchError::from_claude_refresh(ureq::Error::StatusCode(status)),
                ProviderFetchError::AuthInvalid
            ));
        }
        assert!(matches!(
            ProviderFetchError::from_claude_refresh(ureq::Error::StatusCode(429)),
            ProviderFetchError::RateLimited
        ));
        // A server or transport failure says nothing about the credentials
        for error in [
            ureq::Error::StatusCode(500),
            ureq::Error::StatusCode(503),
            ureq::Error::HostNotFound,
            ureq::Error::ConnectionFailed,
        ] {
            assert!(matches!(
                ProviderFetchError::from_claude_refresh(error),
                ProviderFetchError::Other(_)
            ));
        }
    }

    #[test]
    fn test_rate_limit_backoff() {
        let mut backoff = RateLimitBackoff::new();
        assert!(!backoff.active());

        // Jitter can double each computed delay
        let delay = backoff.hit();
        assert!((RATE_LIMIT_MIN_DELAY..(2 * RATE_LIMIT_MIN_DELAY)).contains(&delay));
        assert!(backoff.active());

        // Consecutive rate limits escalate, up to the ceiling
        assert!(backoff.hit() > delay);
        let escalated = iter::repeat_with(|| backoff.hit()).nth(9).unwrap();
        assert!((RATE_LIMIT_MAX_DELAY..(2 * RATE_LIMIT_MAX_DELAY)).contains(&escalated));

        backoff.reset();
        assert!(!backoff.active());
        assert!((RATE_LIMIT_MIN_DELAY..(2 * RATE_LIMIT_MIN_DELAY)).contains(&backoff.hit()));

        backoff.skip_until = SystemTime::now().checked_sub(Duration::from_secs(1));
        assert!(!backoff.active());
    }

    #[test]
    fn test_extract_chatgpt_windows_none() {
        let body = r#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#;
        let resp: ChatGptUsageResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            InferenceUsageModule::extract_chatgpt_windows(&resp.rate_limit),
            Vec::new()
        );
    }
}
