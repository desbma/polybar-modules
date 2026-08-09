use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
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
    home_path: String,
    /// Claude state of each credentials file
    claude_accounts: HashMap<PathBuf, ClaudeAccount>,
    /// `ChatGPT` state of each auth file
    chatgpt_accounts: HashMap<PathBuf, RateLimitBackoff>,
    degraded_backoff: backon::ExponentialBackoff,
    /// Start of the current run of degraded updates
    degraded_since: Option<SystemTime>,
    /// Last state in which every account reported its usage
    last_complete_state: Option<InferenceUsageModuleState>,
}

/// Claude state of a single account
#[derive(Default)]
struct ClaudeAccount {
    rate_limit: RateLimitBackoff,
    /// Mtime of the credentials whose authentication failed
    auth_failed_mtime: Option<SystemTime>,
}

/// Escalating delay holding a single account's requests back after a rate limit
///
/// Deadlines are on the wall clock rather than a monotonic one, so that a suspend counts towards
/// them: the throttling they wait out is the provider's, and it expires while we are asleep.
struct RateLimitBackoff {
    backoff: backon::ExponentialBackoff,
    skip_until: Option<SystemTime>,
}

impl Default for RateLimitBackoff {
    fn default() -> Self {
        Self {
            backoff: RATE_LIMIT_BACKOFF.build(),
            skip_until: None,
        }
    }
}

impl RateLimitBackoff {
    /// Return whether requests are currently held back
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

/// Inference usage state, with one entry per account of each provider
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InferenceUsageModuleState {
    claude_statuses: Vec<ClaudeUsageStatus>,
    chatgpt_statuses: Vec<Option<Vec<UsageWindow>>>,
}

impl InferenceUsageModuleState {
    /// Return whether some account usage is missing
    fn is_degraded(&self) -> bool {
        self.chatgpt_statuses.iter().any(Option::is_none)
            || self
                .claude_statuses
                .iter()
                .any(|status| !matches!(status, ClaudeUsageStatus::Available { .. }))
    }
}

const ICON_INFERENCE_USAGE: &str = "󱩅";
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
/// Prefix of the Claude credentials files, relative to the home directory
const CLAUDE_TOKEN_PREFIX: &str = ".config/claude/.credentials";
/// Prefix of the Codex auth files, relative to the home directory
const CODEX_AUTH_PREFIX: &str = ".config/codex/auth";
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
/// Shortest delay an account is left alone after a rate limit, before jitter
const RATE_LIMIT_MIN_DELAY: Duration = Duration::from_mins(5);
/// Longest delay an account is left alone after a rate limit, before jitter
///
/// Jitter stretches an emitted delay by up to as much again, so it is a ceiling on the escalation
/// rather than on the wait itself.
const RATE_LIMIT_MAX_DELAY: Duration = Duration::from_hours(1);
/// Escalation curve of the delay a rate limited account is left alone for
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
        Self {
            client,
            home_path: env::var("HOME").unwrap(),
            claude_accounts: HashMap::new(),
            chatgpt_accounts: HashMap::new(),
            degraded_backoff: DEGRADED_BACKOFF.build(),
            degraded_since: None,
            last_complete_state: None,
        }
    }

    /// Return the `<prefix>.json` file of the default account, followed by the `<prefix>-*.json`
    /// files of the extra ones
    ///
    /// The default path is listed whether it exists or not.
    fn account_paths(home: &str, prefix: &str) -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from(format!("{home}/{prefix}.json"))];
        // Escape the home directory, it would otherwise be a glob pattern of its own
        let extra_pattern = format!("{}/{prefix}-*.json", glob::Pattern::escape(home));
        match glob::glob(&extra_pattern) {
            Ok(entries) => paths.extend(entries.filter_map(|entry| {
                entry
                    .inspect_err(|error| log::error!("Failed to list credentials: {error}"))
                    .ok()
            })),
            Err(error) => log::error!("Invalid credentials pattern {extra_pattern:?}: {error}"),
        }
        paths
    }

    /// Drop the state of the accounts missing from `paths`
    fn retain_accounts<T>(accounts: &mut HashMap<PathBuf, T>, paths: &[PathBuf]) {
        accounts.retain(|path, _| paths.contains(path));
    }

    /// Delay before the next update, shortened while retrying through a degraded update
    ///
    /// A rate limited account is the one thing worth waiting out, so it holds the update back to
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

    /// Return whether an account is currently holding us back
    fn rate_limited(&self) -> bool {
        self.claude_accounts
            .values()
            .any(|account| account.rate_limit.active())
            || self.chatgpt_accounts.values().any(RateLimitBackoff::active)
    }

    fn fetch_chatgpt_usage(&self, path: &Path) -> Result<Vec<UsageWindow>, ProviderFetchError> {
        let auth_data = fs::read_to_string(path)
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

    fn update_chatgpt_usage(&mut self, path: &Path) -> Option<Vec<UsageWindow>> {
        if self
            .chatgpt_accounts
            .get(path)
            .is_some_and(RateLimitBackoff::active)
        {
            log::debug!("Skipping ChatGPT usage for {path:?}: rate limited");
            return None;
        }

        let result = self.fetch_chatgpt_usage(path).or_else(|error| {
            if !matches!(error, ProviderFetchError::AuthInvalid) {
                return Err(error);
            }
            log::warn!("ChatGPT usage for {path:?}: auth invalid (401), refreshing token");
            self.refresh_chatgpt_token(path)?;
            self.fetch_chatgpt_usage(path)
        });

        let rate_limit = self.chatgpt_accounts.entry(path.to_owned()).or_default();
        match result {
            Ok(windows) => {
                rate_limit.reset();
                if windows.is_empty() {
                    log::error!("ChatGPT usage for {path:?}: no rate limit windows");
                    None
                } else {
                    Some(windows)
                }
            }
            Err(ProviderFetchError::RateLimited) => {
                let delay = rate_limit.hit();
                log::warn!("ChatGPT usage for {path:?}: rate limited, backing off for {delay:?}");
                None
            }
            Err(error) => {
                log::error!("ChatGPT usage for {path:?}: {error}");
                None
            }
        }
    }

    fn refresh_chatgpt_token(&self, path: &Path) -> Result<(), ProviderFetchError> {
        let auth_data =
            fs::read_to_string(path).context("Failed to read codex auth for refresh")?;
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

        Self::overwrite_json(path, &auth).context("Failed to write refreshed codex auth")?;

        log::info!("Codex token refreshed for {path:?}");
        Ok(())
    }

    fn token_mtime(path: &Path) -> Option<SystemTime> {
        fs::metadata(path).and_then(|m| m.modified()).ok()
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

    fn fetch_claude_usage(
        &self,
        path: &Path,
    ) -> Result<(UsageWindow, UsageWindow), ProviderFetchError> {
        let creds_data = fs::read_to_string(path)
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

    fn update_claude_status(&mut self, path: &Path) -> ClaudeUsageStatus {
        // Skip if auth failed and token file unchanged, or if rate-limit backoff active
        if let Some(account) = self.claude_accounts.get(path) {
            if let Some(failed_mtime) = account.auth_failed_mtime
                && Self::token_mtime(path) == Some(failed_mtime)
            {
                log::debug!("Skipping Claude usage for {path:?}: auth invalid, token unchanged");
                return ClaudeUsageStatus::AuthInvalid;
            }
            if account.rate_limit.active() {
                log::debug!("Skipping Claude usage for {path:?}: rate limited");
                return ClaudeUsageStatus::Error;
            }
        }

        // Mtime of the credentials the failing request used, captured before each attempt to avoid
        // a race where a login rewrites them between our read and the mtime probe
        let mut tried_creds_mtime = Self::token_mtime(path);

        let result = self.fetch_claude_usage(path).or_else(|error| {
            if !matches!(error, ProviderFetchError::AuthInvalid) {
                return Err(error);
            }
            log::warn!("Claude usage for {path:?}: auth invalid (401), refreshing token");
            self.refresh_claude_token(path)?;
            tried_creds_mtime = Self::token_mtime(path);
            self.fetch_claude_usage(path)
        });

        let account = self.claude_accounts.entry(path.to_owned()).or_default();
        match result {
            Ok((h5, d7)) => {
                account.auth_failed_mtime = None;
                account.rate_limit.reset();
                ClaudeUsageStatus::Available { h5, d7 }
            }
            Err(ProviderFetchError::AuthInvalid) => {
                log::error!("Claude usage for {path:?}: auth invalid until credentials change");
                account.auth_failed_mtime = tried_creds_mtime;
                ClaudeUsageStatus::AuthInvalid
            }
            Err(ProviderFetchError::RateLimited) => {
                let delay = account.rate_limit.hit();
                log::warn!("Claude usage for {path:?}: rate limited, backing off for {delay:?}");
                ClaudeUsageStatus::Error
            }
            Err(ProviderFetchError::Other(error)) => {
                log::error!("Claude usage for {path:?}: {error}");
                ClaudeUsageStatus::Error
            }
        }
    }

    fn refresh_claude_token(&self, path: &Path) -> Result<(), ProviderFetchError> {
        let creds_data =
            fs::read_to_string(path).context("Failed to read credentials for refresh")?;
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

        Self::overwrite_json(path, &creds).context("Failed to write refreshed credentials")?;

        log::info!(
            "Claude token refreshed for {path:?}, expires in {} seconds",
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

    fn provider_markup(label: &str, usage: &str, default: bool, url: &str) -> String {
        if !default {
            return usage.to_owned();
        }
        markup::Markup::new(format!("{label} {usage}"))
            .action(
                markup::PolybarActionType::ClickLeft,
                format!("firefox --new-tab '{url}'"),
            )
            .into_string()
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
        let claude_paths = Self::account_paths(&self.home_path, CLAUDE_TOKEN_PREFIX);
        Self::retain_accounts(&mut self.claude_accounts, &claude_paths);
        let claude_statuses = claude_paths
            .iter()
            .map(|path| self.update_claude_status(path))
            .collect();

        let chatgpt_paths = Self::account_paths(&self.home_path, CODEX_AUTH_PREFIX);
        Self::retain_accounts(&mut self.chatgpt_accounts, &chatgpt_paths);
        let chatgpt_statuses = chatgpt_paths
            .iter()
            .map(|path| self.update_chatgpt_usage(path))
            .collect();

        let state = InferenceUsageModuleState {
            claude_statuses,
            chatgpt_statuses,
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
        // Accounts render in credentials file order, the default one first
        let claude = state
            .claude_statuses
            .iter()
            .map(|status| match status {
                ClaudeUsageStatus::Available { h5, d7 } => Self::render_windows([h5, d7]),
                ClaudeUsageStatus::AuthInvalid => ICON_UNAUTHORIZED.to_owned(),
                ClaudeUsageStatus::Error => warning(),
            })
            .enumerate()
            .map(|(index, usage)| {
                Self::provider_markup(ICON_CLAUDE, &usage, index == 0, CLAUDE_USAGE_URL)
            })
            .join(" ");
        let chatgpt = state
            .chatgpt_statuses
            .iter()
            .map(|windows| windows.as_ref().map_or_else(warning, Self::render_windows))
            .enumerate()
            .map(|(index, usage)| {
                Self::provider_markup(ICON_CHATGPT, &usage, index == 0, CHATGPT_USAGE_URL)
            })
            .join(" ");

        [
            markup::Markup::new(ICON_INFERENCE_USAGE)
                .fg(theme::Color::MainIcon)
                .into_string(),
            claude,
            chatgpt,
        ]
        .join(" ")
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use std::{collections::HashSet, iter};

    use super::*;

    /// Slack absorbing the time a test itself takes
    const MARGIN: Duration = Duration::from_secs(1);

    /// Return a test account's rate limit backoff, created on first use
    fn rate_limit(module: &mut InferenceUsageModule, claude: bool) -> &mut RateLimitBackoff {
        let path = PathBuf::from("account");
        if claude {
            &mut module.claude_accounts.entry(path).or_default().rate_limit
        } else {
            module.chatgpt_accounts.entry(path).or_default()
        }
    }

    fn usage_window(quota_left_pct: f64, time_left_frac: f64) -> UsageWindow {
        UsageWindow {
            quota_left_pct,
            time_left_frac: Some(time_left_frac),
        }
    }

    #[test]
    fn test_account_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path().to_str().unwrap();
        let default = dir.path().join(".credentials.json");

        // A provider with no credentials file at all still has its default account
        assert_eq!(
            InferenceUsageModule::account_paths(home, ".credentials"),
            vec![default.clone()]
        );

        for name in [
            ".credentials.json",
            ".credentials-work.json",
            ".credentials-perso.json",
            ".credentials-work.json.bak",
            "auth-work.json",
        ] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        // Default first and only once, extra accounts sorted after it, unrelated files left out
        assert_eq!(
            InferenceUsageModule::account_paths(home, ".credentials"),
            vec![
                default,
                dir.path().join(".credentials-perso.json"),
                dir.path().join(".credentials-work.json"),
            ]
        );
    }

    #[test]
    fn test_claude_state_is_per_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let failed = dir.path().join("failed.json");
        let limited = dir.path().join("limited.json");
        let other = dir.path().join("other.json");
        // Credentials no request can be built from, so none is sent
        fs::write(&failed, "{}").unwrap();
        fs::write(&other, "{}").unwrap();

        let mut module = InferenceUsageModule::new();
        module
            .claude_accounts
            .entry(failed.clone())
            .or_default()
            .auth_failed_mtime = InferenceUsageModule::token_mtime(&failed);
        module
            .claude_accounts
            .entry(limited)
            .or_default()
            .rate_limit
            .hit();

        // A skipped account leaves the others alone, which an entry of their own attests
        assert_eq!(
            module.update_claude_status(&failed),
            ClaudeUsageStatus::AuthInvalid
        );
        assert_eq!(
            module.update_claude_status(&other),
            ClaudeUsageStatus::Error
        );
        assert!(module.claude_accounts.contains_key(&other));
    }

    #[test]
    fn test_chatgpt_state_is_per_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let limited = dir.path().join("limited.json");
        let other = dir.path().join("other.json");
        // Credentials no request can be built from, so none is sent
        fs::write(&other, "{}").unwrap();

        let mut module = InferenceUsageModule::new();
        module.chatgpt_accounts.entry(limited).or_default().hit();

        // A skipped account leaves the others alone, which an entry of their own attests
        assert_eq!(module.update_chatgpt_usage(&other), None);
        assert!(module.chatgpt_accounts.contains_key(&other));
    }

    #[test]
    fn test_update_discovers_accounts_and_prunes_deleted() {
        let home = tempfile::TempDir::new().unwrap();
        for relative in [
            ".config/claude/.credentials.json",
            ".config/claude/.credentials-work.json",
            ".config/codex/auth.json",
            ".config/codex/auth-personal.json",
            ".config/codex/auth-work.json",
        ] {
            let path = home.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            // Credentials no request can be built from, so none is sent
            fs::write(path, "{}").unwrap();
        }
        let claude_default = home.path().join(".config/claude/.credentials.json");
        let claude_work = home.path().join(".config/claude/.credentials-work.json");
        let chatgpt_default = home.path().join(".config/codex/auth.json");
        let chatgpt_perso = home.path().join(".config/codex/auth-personal.json");
        let chatgpt_work = home.path().join(".config/codex/auth-work.json");

        let mut module = InferenceUsageModule::new();
        module.home_path = home.path().to_str().unwrap().to_owned();
        for path in [
            claude_work.clone(),
            home.path().join(".config/claude/.credentials-deleted.json"),
        ] {
            module
                .claude_accounts
                .entry(path)
                .or_default()
                .rate_limit
                .hit();
        }
        for path in [
            chatgpt_work.clone(),
            home.path().join(".config/codex/auth-deleted.json"),
        ] {
            module.chatgpt_accounts.entry(path).or_default().hit();
        }

        // Every discovered account is updated, the default one first
        assert_eq!(
            module.update(),
            InferenceUsageModuleState {
                claude_statuses: vec![ClaudeUsageStatus::Error, ClaudeUsageStatus::Error],
                chatgpt_statuses: vec![None, None, None],
            }
        );
        // A deleted credentials file takes its state with it
        assert_eq!(
            module.claude_accounts.keys().collect::<HashSet<_>>(),
            HashSet::from([&claude_default, &claude_work])
        );
        assert_eq!(
            module.chatgpt_accounts.keys().collect::<HashSet<_>>(),
            HashSet::from([&chatgpt_default, &chatgpt_perso, &chatgpt_work])
        );
        // A live one keeps its backoff, which no update of its own would set back
        assert!(module.rate_limited());
    }

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

    /// Assert `state` renders with the given usage for each account of each provider
    fn assert_render(state: &InferenceUsageModuleState, [claude, chatgpt]: [&[&str]; 2]) {
        let provider = |label: &str, usages: &[&str], url: &str| {
            let (default, extra) = usages.split_first().unwrap();
            let default = markup::Markup::new(format!("{label} {default}"))
                .action(
                    markup::PolybarActionType::ClickLeft,
                    format!("firefox --new-tab '{url}'"),
                )
                .into_string();
            iter::once(default.as_str())
                .chain(extra.iter().copied())
                .join(" ")
        };
        assert_eq!(
            InferenceUsageModule::new().render(state),
            [
                markup::Markup::new(ICON_INFERENCE_USAGE)
                    .fg(theme::Color::MainIcon)
                    .into_string(),
                provider(ICON_CLAUDE, claude, CLAUDE_USAGE_URL),
                provider(ICON_CHATGPT, chatgpt, CHATGPT_USAGE_URL),
            ]
            .join(" ")
        );
    }

    #[test]
    fn test_render_accounts() {
        let att_warn = markup::Markup::new(ICON_WARNING)
            .fg(theme::Color::Attention)
            .into_string();

        // Extra accounts render after the default one, space separated
        let state = InferenceUsageModuleState {
            claude_statuses: vec![
                ClaudeUsageStatus::Available {
                    h5: usage_window(50.0, 0.75),
                    d7: usage_window(80.0, 0.9),
                },
                ClaudeUsageStatus::AuthInvalid,
            ],
            chatgpt_statuses: vec![
                Some(vec![usage_window(81.0, 0.5)]),
                None,
                Some(vec![usage_window(20.0, 0.3)]),
            ],
        };
        assert_render(
            &state,
            [
                &[
                    "%{F#819500}󰪡%{F-}%{F#819500}▆%{F-}%{F#819500}󰪣%{F-}%{F#819500}█%{F-}",
                    ICON_UNAUTHORIZED,
                ],
                &[
                    "%{F#819500}󰪣%{F-}%{F#819500}▄%{F-}",
                    &att_warn,
                    "%{F#ac8300}󰪟%{F-}%{F#ac8300}▃%{F-}",
                ],
            ],
        );
    }

    #[test]
    fn test_render() {
        let att_warn = markup::Markup::new(ICON_WARNING)
            .fg(theme::Color::Attention)
            .into_string();

        let state = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::Available {
                h5: usage_window(50.0, 0.75),
                d7: usage_window(80.0, 0.9),
            }],
            chatgpt_statuses: vec![Some(vec![usage_window(81.0, 0.5), usage_window(90.0, 1.0)])],
        };
        assert_render(
            &state,
            [
                &["%{F#819500}󰪡%{F-}%{F#819500}▆%{F-}%{F#819500}󰪣%{F-}%{F#819500}█%{F-}"],
                &["%{F#819500}󰪣%{F-}%{F#819500}▄%{F-}%{F#819500}󰪤%{F-}%{F#819500}█%{F-}"],
            ],
        );

        // All errors
        let state = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::Error],
            chatgpt_statuses: vec![None],
        };
        assert_render(&state, [&[&att_warn], &[&att_warn]]);

        let state = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::Available {
                h5: usage_window(95.0, 0.125),
                d7: usage_window(95.0, 0.4),
            }],
            chatgpt_statuses: vec![Some(vec![usage_window(95.0, 0.0), usage_window(95.0, 0.6)])],
        };
        assert_render(
            &state,
            [
                &["%{F#819500}󰪤%{F-}%{F#819500}▁%{F-}%{F#819500}󰪤%{F-}%{F#819500}▄%{F-}"],
                &["%{F#819500}󰪤%{F-}%{F#819500}▁%{F-}%{F#819500}󰪤%{F-}%{F#819500}▅%{F-}"],
            ],
        );

        // Claude auth invalid (401)
        let state = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::AuthInvalid],
            chatgpt_statuses: vec![Some(vec![usage_window(20.0, 0.3), usage_window(5.0, 0.8)])],
        };
        assert_render(
            &state,
            [
                &[ICON_UNAUTHORIZED],
                &["%{F#ac8300}󰪟%{F-}%{F#ac8300}▃%{F-}%{F#d56500}󰪞%{F-}%{F#d56500}▇%{F-}"],
            ],
        );

        // Claude 5h window not running yet: full quota, no reset bar
        let state = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::Available {
                h5: UsageWindow {
                    quota_left_pct: 100.0,
                    time_left_frac: None,
                },
                d7: usage_window(80.0, 0.9),
            }],
            chatgpt_statuses: vec![None],
        };
        assert_render(
            &state,
            [
                &["%{F#819500}󰪥%{F-}%{F#819500}󰪣%{F-}%{F#819500}█%{F-}"],
                &[&att_warn],
            ],
        );

        // ChatGPT with a single window renders a single quota icon, still with its reset bar
        let state = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::Error],
            chatgpt_statuses: vec![Some(vec![usage_window(82.0, 1.0)])],
        };
        assert_render(
            &state,
            [&[&att_warn], &["%{F#819500}󰪣%{F-}%{F#819500}█%{F-}"]],
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
        let available = || ClaudeUsageStatus::Available {
            h5: usage_window(50.0, 0.5),
            d7: usage_window(50.0, 0.5),
        };
        let complete = InferenceUsageModuleState {
            claude_statuses: vec![available()],
            chatgpt_statuses: vec![Some(vec![usage_window(50.0, 0.5)])],
        };
        assert!(!complete.is_degraded());

        for state in [
            InferenceUsageModuleState {
                claude_statuses: vec![ClaudeUsageStatus::AuthInvalid],
                ..complete.clone()
            },
            // A single failing account is enough
            InferenceUsageModuleState {
                claude_statuses: vec![available(), ClaudeUsageStatus::Error],
                ..complete.clone()
            },
            InferenceUsageModuleState {
                chatgpt_statuses: vec![Some(vec![usage_window(50.0, 0.5)]), None],
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
            rate_limit(&mut module, claude).hit();
            assert_eq!(module.next_delay(), UPDATE_INTERVAL);
            assert_eq!(module.next_delay(), UPDATE_INTERVAL);

            // Once it stops, the backoff resumes from its floor rather than mid-curve
            rate_limit(&mut module, claude).reset();
            assert!((DEGRADED_MIN_DELAY..(2 * DEGRADED_MIN_DELAY)).contains(&module.next_delay()));
        }
    }

    #[test]
    fn test_next_delay_capped_by_degraded_hold() {
        let complete = InferenceUsageModuleState {
            claude_statuses: vec![ClaudeUsageStatus::Available {
                h5: usage_window(50.0, 0.5),
                d7: usage_window(50.0, 0.5),
            }],
            chatgpt_statuses: vec![Some(vec![usage_window(50.0, 0.5)])],
        };
        let mut module = InferenceUsageModule::new();
        module.degraded_since = Some(SystemTime::now());
        module.last_complete_state = Some(complete.clone());

        // A rate limit stretches the interval past the hold, the held state still expires on time
        rate_limit(&mut module, true).hit();
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
        let mut backoff = RateLimitBackoff::default();
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
