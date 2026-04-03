use std::{
    env, fs, io,
    os::unix::fs::OpenOptionsExt as _,
    path::PathBuf,
    process::{self, Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use backon::BackoffBuilder as _;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use ureq::http::StatusCode;

use crate::{
    markup,
    polybar_module::{RenderablePolybarModule, TCP_REMOTE_TIMEOUT},
    theme::{self, ICON_WARNING},
};

/// Inference API usage module
pub(crate) struct InferenceUsageModule {
    client: ureq::Agent,
    amp_usage_re: Regex,
    token_path: PathBuf,
    claude_rate_limit_backoff_builder: backon::ExponentialBuilder,
    claude_rate_limit_backoff: backon::ExponentialBackoff,
    claude_skip_until: Option<Instant>,
    claude_auth_failed_mtime: Option<SystemTime>,
    chatgpt_h5_limit_re: Regex,
    chatgpt_weekly_limit_re: Regex,
    codex_session: Option<ChatGptSession>,
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

/// `ChatGPT` usage fetch status
#[derive(Debug, PartialEq)]
pub(crate) enum ChatGptUsageStatus {
    /// Successfully fetched usage data
    Available {
        /// 5-hour remaining percentage
        h5_left: f64,
        /// Weekly remaining percentage
        weekly_left: f64,
    },
    /// Generic error
    Error,
}

/// Inference usage state
#[derive(Debug, PartialEq)]
pub(crate) struct InferenceUsageModuleState {
    amp_free_credit: Option<f64>,
    claude_status: ClaudeUsageStatus,
    chatgpt_status: ChatGptUsageStatus,
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
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/codex/settings/usage";
const CHATGPT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CHATGPT_MAX_TRIES: usize = 120;
const CHATGPT_SEND_EVERY: usize = 7;
const CHATGPT_TRUST_PROMPT: &str = "Do you trust the contents of this directory";
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

struct ChatGptSession {
    name: String,
    _workdir: TempDir,
    tty_child: Child,
    next_is_status_cmd: bool,
}

impl ChatGptSession {
    fn new() -> anyhow::Result<Self> {
        let session_name = Self::session_name();
        let workdir = tempfile::tempdir()?;

        let session_start_status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-x",
                "160",
                "-y",
                "48",
                "-s",
                &session_name,
                "bash",
                "-lc",
                "cd \"$POLYBAR_CHATGPT_USAGE_WORKDIR\" && exec codex",
            ])
            .env("POLYBAR_CHATGPT_USAGE_WORKDIR", workdir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to start tmux session for ChatGPT usage")?;
        session_start_status
            .exit_ok()
            .context("tmux new-session exited with error")?;

        let attach_cmd = format!("tmux attach-session -t {session_name} -f read-only,ignore-size");
        let tty_child = match Command::new("script")
            .args(["-qefc", &attach_cmd, "/dev/null"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                let _ = Self::tmux_status(&["kill-session", "-t", &session_name]);
                return Err(e.into());
            }
        };

        Ok(Self {
            name: session_name,
            _workdir: workdir,
            tty_child,
            next_is_status_cmd: true,
        })
    }

    fn tmux_status(args: &[&str]) -> anyhow::Result<()> {
        let status = Command::new("tmux")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run tmux")?;
        status.exit_ok().context("tmux exited with error")?;
        Ok(())
    }

    fn tmux_output(args: &[&str]) -> anyhow::Result<String> {
        let output = Command::new("tmux")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .context("Failed to run tmux")?;
        output.status.exit_ok().context("tmux exited with error")?;
        String::from_utf8(output.stdout).context("tmux output is not UTF-8")
    }

    fn session_name() -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map_or(0_u128, |d| d.as_millis());
        format!("polybar_chatgpt_usage_{}_{}", process::id(), ts)
    }

    fn is_alive(&self) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", &self.name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn capture_pane(&self) -> anyhow::Result<String> {
        Self::tmux_output(&["capture-pane", "-pJ", "-t", &self.name, "-S", "-500"])
            .context("Failed to capture tmux pane")
    }

    fn send_keys(&self, keys: &str) -> anyhow::Result<()> {
        Self::tmux_status(&["send-keys", "-t", &self.name, keys])
    }

    fn cleanup(&mut self) {
        let _ = self.tty_child.kill();
        let _ = self.tty_child.wait();
        let _ = Self::tmux_status(&["kill-session", "-t", &self.name]);
    }
}

enum ClaudeFetchError {
    AuthInvalid,
    RateLimited,
    Other(anyhow::Error),
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeCredentials {
    claude_ai_oauth: ClaudeOauth,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOauth {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    scopes: Vec<String>,
    subscription_type: String,
    rate_limit_tier: String,
}

#[derive(Deserialize)]
struct ClaudeUsageResponse {
    five_hour: ClaudeUsageWindow,
    seven_day: ClaudeUsageWindow,
}

#[derive(Deserialize)]
struct ClaudeUsageWindow {
    utilization: f64,
}

#[derive(Serialize)]
struct ClaudeTokenRequest {
    grant_type: &'static str,
    refresh_token: String,
    client_id: &'static str,
    scope: String,
}

#[derive(Deserialize)]
struct ClaudeTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
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
                .http_status_as_error(false)
                .build(),
        );
        let amp_usage_re = Regex::new(r"\$([0-9]+\.?[0-9]*)").unwrap();
        let home = env::var("HOME").unwrap();
        let token_path = PathBuf::from(home).join(".claude/.credentials.json");
        let chatgpt_h5_limit_re = Regex::new("5h limit:.* ([0-9]{1,3})% left").unwrap();
        let chatgpt_weekly_limit_re = Regex::new("Weekly limit:.* ([0-9]{1,3})% left").unwrap();
        let claude_rate_limit_backoff_builder = backon::ExponentialBuilder::default()
            .with_jitter()
            .with_min_delay(Duration::from_mins(5))
            .with_max_delay(Duration::from_hours(1))
            .without_max_times();
        let claude_rate_limit_backoff = claude_rate_limit_backoff_builder.build();
        Self {
            client,
            amp_usage_re,
            token_path,
            claude_rate_limit_backoff_builder,
            claude_rate_limit_backoff,
            claude_skip_until: None,
            claude_auth_failed_mtime: None,
            chatgpt_h5_limit_re,
            chatgpt_weekly_limit_re,
            codex_session: None,
        }
    }

    fn reset_codex_session(&mut self) {
        if let Some(mut session) = self.codex_session.take() {
            session.cleanup();
        }
    }

    fn ensure_codex_session(&mut self) -> anyhow::Result<()> {
        if self
            .codex_session
            .as_ref()
            .is_some_and(ChatGptSession::is_alive)
        {
            return Ok(());
        }
        self.reset_codex_session();
        self.codex_session = Some(ChatGptSession::new()?);
        Ok(())
    }

    fn parse_chatgpt_left_pct(re: &Regex, output: &str, label: &str) -> anyhow::Result<u8> {
        let cap = re
            .captures_iter(output)
            .last()
            .ok_or_else(|| anyhow::anyhow!("Unable to find {label} usage in codex output"))?;
        cap.get(1)
            .ok_or_else(|| anyhow::anyhow!("Unable to capture {label} percentage"))?
            .as_str()
            .parse()
            .context("Failed to parse ChatGPT usage percentage")
    }

    fn fetch_chatgpt_usage_once(&mut self) -> anyhow::Result<(f64, f64)> {
        let raw_output = {
            let session = self
                .codex_session
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Codex session missing"))?;
            let mut raw_output = String::new();
            let mut sent = false;
            let mut last_send = 0;

            for i in 0..CHATGPT_MAX_TRIES {
                sleep(CHATGPT_POLL_INTERVAL);
                raw_output = session.capture_pane()?;

                if raw_output.contains(CHATGPT_TRUST_PROMPT) {
                    session.send_keys("Enter")?;
                }

                if raw_output.contains("5h limit:") && raw_output.contains("Weekly limit:") {
                    break;
                }

                if raw_output.contains("gpt-")
                    && raw_output.contains("left")
                    && (!sent || i.saturating_sub(last_send) >= CHATGPT_SEND_EVERY)
                {
                    session.send_keys("C-u")?;
                    if session.next_is_status_cmd {
                        session.send_keys("/status")?;
                    } else {
                        session.send_keys("/usage")?;
                    }
                    session.send_keys("Enter")?;
                    session.next_is_status_cmd = !session.next_is_status_cmd;
                    sent = true;
                    last_send = i;
                }
            }

            raw_output
        };

        if !(raw_output.contains("5h limit:") && raw_output.contains("Weekly limit:")) {
            return Err(anyhow::anyhow!(
                "Timed out waiting for ChatGPT usage in tmux session"
            ));
        }

        let h5_left = f64::from(Self::parse_chatgpt_left_pct(
            &self.chatgpt_h5_limit_re,
            &raw_output,
            "5h",
        )?);
        let weekly_left = f64::from(Self::parse_chatgpt_left_pct(
            &self.chatgpt_weekly_limit_re,
            &raw_output,
            "weekly",
        )?);
        Ok((h5_left, weekly_left))
    }

    fn fetch_chatgpt_usage(&mut self) -> anyhow::Result<(f64, f64)> {
        self.ensure_codex_session()?;
        match self.fetch_chatgpt_usage_once() {
            Ok(usage) => Ok(usage),
            Err(e) => {
                log::warn!("ChatGPT usage first attempt failed, restarting session: {e}");
                self.reset_codex_session();
                self.ensure_codex_session()?;
                self.fetch_chatgpt_usage_once()
            }
        }
    }

    fn fetch_amp_usage(&self) -> anyhow::Result<f64> {
        let output = Command::new("/usr/bin/amp")
            .arg("usage")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .context("Failed to run amp usage")?;
        output.status.exit_ok()?;
        let stdout = String::from_utf8(output.stdout)?;
        let cap = self
            .amp_usage_re
            .captures(&stdout)
            .ok_or_else(|| anyhow::anyhow!("No dollar amount found in amp usage output"))?;
        let amount: f64 = cap
            .get(1)
            .unwrap()
            .as_str()
            .parse()
            .context("Failed to parse dollar amount")?;
        Ok(amount)
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
            .map_err(|e| ClaudeFetchError::Other(e.into()))?;

        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ClaudeFetchError::AuthInvalid);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(ClaudeFetchError::RateLimited);
        }
        if status.is_client_error() || status.is_server_error() {
            return Err(ClaudeFetchError::Other(anyhow::anyhow!(
                "HTTP status {status}"
            )));
        }

        let body: ClaudeUsageResponse = serde_json::from_str(
            &response
                .into_body()
                .read_to_string()
                .map_err(|e| ClaudeFetchError::Other(e.into()))?,
        )
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
        let request_str = serde_json::to_string(&request_body)?;

        let response = self
            .client
            .post("https://platform.claude.com/v1/oauth/token")
            .header("Content-Type", "application/json")
            .send(&*request_str)?;

        if !response.status().is_success() {
            anyhow::bail!("Token refresh failed with status {}", response.status());
        }

        let tok: ClaudeTokenResponse =
            serde_json::from_str(&response.into_body().read_to_string()?)?;

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

        let new_data = serde_json::to_string(&creds)?;
        fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&self.token_path)
            .and_then(|file| io::Write::write_all(&mut &file, new_data.as_bytes()))
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
        #[expect(clippy::indexing_slicing)]
        let icon = if pct == 0 {
            PROGRESS_ICONS[0]
        } else {
            // Map 1–100% linearly to slice_1 (index 1) through slice_8 (index 8)
            PROGRESS_ICONS[1 + (pct - 1) * 7 / 99]
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
}

impl Drop for InferenceUsageModule {
    fn drop(&mut self) {
        self.reset_codex_session();
    }
}

impl RenderablePolybarModule for InferenceUsageModule {
    type State = InferenceUsageModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_mins(3));
        }
    }

    fn update(&mut self) -> Self::State {
        let amp_usage_dollars = match self.fetch_amp_usage() {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("AMP usage: {e}");
                None
            }
        };

        let claude_status = self.update_claude_status();

        let chatgpt_status = match self.fetch_chatgpt_usage() {
            Ok((h5_left, weekly_left)) => ChatGptUsageStatus::Available {
                h5_left,
                weekly_left,
            },
            Err(e) => {
                log::error!("ChatGPT usage: {e}");
                ChatGptUsageStatus::Error
            }
        };

        InferenceUsageModuleState {
            amp_free_credit: amp_usage_dollars,
            claude_status,
            chatgpt_status,
        }
    }

    fn render(&self, state: &Self::State) -> String {
        let mut fragments =
            vec![markup::Markup::new(ICON_INFERENCE_USAGE).fg(theme::Color::MainIcon)];

        // AMP ($10 = 100%)
        match state.amp_free_credit {
            Some(dollars) => {
                fragments.push(Self::provider_markup(
                    ICON_AMP,
                    Self::render_progress(dollars / 10.0 * 100.0),
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

        match &state.chatgpt_status {
            ChatGptUsageStatus::Available {
                h5_left,
                weekly_left,
            } => {
                fragments.push(Self::provider_markup(
                    ICON_CHATGPT,
                    format!(
                        "{}{}",
                        Self::render_progress(*h5_left),
                        Self::render_progress(*weekly_left),
                    ),
                    CHATGPT_USAGE_URL,
                ));
            }
            ChatGptUsageStatus::Error => {
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

        // AMP $4.50 = 45%, Claude 5h=50% used (50% remaining) 7d=20% used (80% remaining)
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(4.5),
            claude_status: ClaudeUsageStatus::Available { h5: 50.0, d7: 20.0 },
            chatgpt_status: ChatGptUsageStatus::Available {
                h5_left: 81.0,
                weekly_left: 90.0,
            },
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
            amp_free_credit: None,
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_status: ChatGptUsageStatus::Error,
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

        // AMP $10 = 100% (full ramp), Claude error
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(10.0),
            claude_status: ClaudeUsageStatus::Error,
            chatgpt_status: ChatGptUsageStatus::Error,
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

        // AMP $0.50 = 5% (low/Attention), Claude 5% used (95% remaining)
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(0.5),
            claude_status: ClaudeUsageStatus::Available { h5: 5.0, d7: 5.0 },
            chatgpt_status: ChatGptUsageStatus::Available {
                h5_left: 95.0,
                weekly_left: 95.0,
            },
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
            amp_free_credit: Some(5.0),
            claude_status: ClaudeUsageStatus::AuthInvalid,
            chatgpt_status: ChatGptUsageStatus::Available {
                h5_left: 20.0,
                weekly_left: 5.0,
            },
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
    }

    #[test]
    fn test_parse_chatgpt_usage_output() {
        let module = InferenceUsageModule::new();
        let output = "5h limit:             [████████████████░░░░] 81% left (resets 19:47)
Weekly limit:         [██████████████████░░] 90% left (resets 19:45 on 30 Mar)";
        assert_eq!(
            InferenceUsageModule::parse_chatgpt_left_pct(&module.chatgpt_h5_limit_re, output, "5h")
                .unwrap(),
            81
        );
        assert_eq!(
            InferenceUsageModule::parse_chatgpt_left_pct(
                &module.chatgpt_weekly_limit_re,
                output,
                "weekly"
            )
            .unwrap(),
            90
        );
    }
}
