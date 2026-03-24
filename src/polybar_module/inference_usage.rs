use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant, SystemTime},
};

use anyhow::Context as _;
use regex::Regex;
use serde::Deserialize;

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
    claude_skip_until: Option<Instant>,
    claude_auth_failed_mtime: Option<SystemTime>,
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
    amp_free_credit: Option<f64>,
    claude_status: ClaudeUsageStatus,
}

const RAMP_ICONS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const ICON_INFERENCE_USAGE: &str = "󱩅";
const ICON_AMP: &str = "󰞍";
const ICON_CLAUDE: &str = "";

enum ClaudeFetchError {
    AuthInvalid,
    RateLimited,
    Other(anyhow::Error),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeCredentials {
    claude_ai_oauth: ClaudeOauth,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOauth {
    access_token: String,
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
        Self {
            client,
            amp_usage_re,
            token_path,
            claude_skip_until: None,
            claude_auth_failed_mtime: None,
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

    fn skip_claude_update(&self) -> Option<ClaudeUsageStatus> {
        if let Some(failed_mtime) = self.claude_auth_failed_mtime
            && self.claude_token_mtime() == Some(failed_mtime)
        {
            log::debug!("Skipping Claude usage: auth invalid, token unchanged");
            return Some(ClaudeUsageStatus::AuthInvalid);
        }
        if self.claude_skip_until.is_some_and(|t| Instant::now() < t) {
            log::debug!("Skipping Claude usage: rate limited");
            return Some(ClaudeUsageStatus::Error);
        }
        None
    }

    fn fetch_claude_usage(&mut self) -> Result<(f64, f64), ClaudeFetchError> {
        // Capture mtime before reading the file to avoid a race where a login
        // refreshes the token between our read and the mtime probe
        let pre_request_mtime = self.claude_token_mtime();

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

        let status = response.status().as_u16();
        if status == 401 {
            self.claude_auth_failed_mtime = pre_request_mtime;
            return Err(ClaudeFetchError::AuthInvalid);
        }
        if status == 429 {
            self.claude_skip_until = Some(Instant::now() + Duration::from_secs(300));
            return Err(ClaudeFetchError::RateLimited);
        }
        if status >= 400 {
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

        // Successful fetch clears any previous error state
        self.claude_auth_failed_mtime = None;
        self.claude_skip_until = None;

        Ok((body.five_hour.utilization, body.seven_day.utilization))
    }

    fn render_ramp(utilization: f64) -> String {
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pct = utilization.clamp(0.0, 100.0) as usize;
        #[expect(clippy::indexing_slicing)]
        let icon = RAMP_ICONS[pct.min(99) / (100 / (RAMP_ICONS.len() - 1))];
        let color = if utilization > 30.0 {
            Some(theme::Color::Good)
        } else if utilization >= 10.0 {
            Some(theme::Color::Notice)
        } else {
            Some(theme::Color::Attention)
        };
        markup::font_index(&markup::style(icon, color, None, None, None), 0)
    }
}

impl RenderablePolybarModule for InferenceUsageModule {
    type State = InferenceUsageModuleState;

    fn wait_update(&mut self, prev_state: Option<&Self::State>) {
        if prev_state.is_some() {
            sleep(Duration::from_secs(120));
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

        let claude_status = if let Some(skipped) = self.skip_claude_update() {
            skipped
        } else {
            match self.fetch_claude_usage() {
                Ok((h5, d7)) => ClaudeUsageStatus::Available { h5, d7 },
                Err(ClaudeFetchError::AuthInvalid) => {
                    log::warn!("Claude usage: authentication invalid (401)");
                    ClaudeUsageStatus::AuthInvalid
                }
                Err(ClaudeFetchError::RateLimited) => {
                    log::warn!("Claude usage: rate limited");
                    ClaudeUsageStatus::Error
                }
                Err(ClaudeFetchError::Other(e)) => {
                    log::error!("Claude usage: {e}");
                    ClaudeUsageStatus::Error
                }
            }
        };

        InferenceUsageModuleState {
            amp_free_credit: amp_usage_dollars,
            claude_status,
        }
    }

    fn render(&self, state: &Self::State) -> String {
        let mut fragments: Vec<String> = Vec::new();

        fragments.push(markup::style(
            ICON_INFERENCE_USAGE,
            Some(theme::Color::MainIcon),
            None,
            None,
            None,
        ));

        // AMP ($10 = 100%)
        match state.amp_free_credit {
            Some(dollars) => {
                fragments.push(format!(
                    "{} {}",
                    ICON_AMP,
                    Self::render_ramp(dollars / 10.0 * 100.0),
                ));
            }
            None => {
                fragments.push(format!(
                    "{} {}",
                    ICON_AMP,
                    markup::style(
                        ICON_WARNING,
                        Some(theme::Color::Attention),
                        None,
                        None,
                        None
                    ),
                ));
            }
        }

        // Claude
        match &state.claude_status {
            ClaudeUsageStatus::Available { h5, d7 } => {
                fragments.push(format!(
                    "{} {}{}",
                    ICON_CLAUDE,
                    Self::render_ramp(100.0 - h5),
                    Self::render_ramp(100.0 - d7),
                ));
            }
            ClaudeUsageStatus::AuthInvalid => {
                fragments.push(format!(
                    "{} {}",
                    ICON_CLAUDE,
                    markup::style(ICON_WARNING, None, None, None, None),
                ));
            }
            ClaudeUsageStatus::Error => {
                fragments.push(format!(
                    "{} {}",
                    ICON_CLAUDE,
                    markup::style(
                        ICON_WARNING,
                        Some(theme::Color::Attention),
                        None,
                        None,
                        None
                    ),
                ));
            }
        }

        fragments.join(" ")
    }
}

#[cfg(test)]
#[expect(clippy::shadow_unrelated)]
mod tests {
    use super::*;

    #[test]
    fn test_render_ramp() {
        assert_eq!(
            InferenceUsageModule::render_ramp(50.0),
            "%{T0}%{F#819500}▄%{F-}%{T-}"
        );
        assert_eq!(
            InferenceUsageModule::render_ramp(20.0),
            "%{T0}%{F#ac8300}▂%{F-}%{T-}"
        );
        assert_eq!(
            InferenceUsageModule::render_ramp(5.0),
            "%{T0}%{F#d56500}▁%{F-}%{T-}"
        );
        assert_eq!(
            InferenceUsageModule::render_ramp(100.0),
            "%{T0}%{F#819500}█%{F-}%{T-}"
        );
        assert_eq!(
            InferenceUsageModule::render_ramp(0.0),
            "%{T0}%{F#d56500}▁%{F-}%{T-}"
        );
    }

    #[test]
    fn test_render() {
        let module = InferenceUsageModule::new();

        let mi = |s| markup::style(s, Some(theme::Color::MainIcon), None, None, None);
        let att_warn = markup::style(
            ICON_WARNING,
            Some(theme::Color::Attention),
            None,
            None,
            None,
        );

        // AMP $4.50 = 45%, Claude 5h=50% used (50% remaining) 7d=20% used (80% remaining)
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(4.5),
            claude_status: ClaudeUsageStatus::Available { h5: 50.0, d7: 20.0 },
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {ICON_AMP} %{{T0}}%{{F#819500}}▄%{{F-}}%{{T-}} {ICON_CLAUDE} %{{T0}}%{{F#819500}}▄%{{F-}}%{{T-}}%{{T0}}%{{F#819500}}▆%{{F-}}%{{T-}}",
                mi(ICON_INFERENCE_USAGE),
            )
        );

        // All errors
        let state = InferenceUsageModuleState {
            amp_free_credit: None,
            claude_status: ClaudeUsageStatus::Error,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {ICON_AMP} {} {ICON_CLAUDE} {}",
                mi(ICON_INFERENCE_USAGE),
                att_warn,
                att_warn,
            )
        );

        // AMP $10 = 100% (full ramp), Claude error
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(10.0),
            claude_status: ClaudeUsageStatus::Error,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {ICON_AMP} %{{T0}}%{{F#819500}}█%{{F-}}%{{T-}} {ICON_CLAUDE} {}",
                mi(ICON_INFERENCE_USAGE),
                att_warn,
            )
        );

        // AMP $0.50 = 5% (low/Attention), Claude 5% used (95% remaining)
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(0.5),
            claude_status: ClaudeUsageStatus::Available { h5: 5.0, d7: 5.0 },
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {ICON_AMP} %{{T0}}%{{F#d56500}}▁%{{F-}}%{{T-}} {ICON_CLAUDE} %{{T0}}%{{F#819500}}▇%{{F-}}%{{T-}}%{{T0}}%{{F#819500}}▇%{{F-}}%{{T-}}",
                mi(ICON_INFERENCE_USAGE),
            )
        );

        // Claude auth invalid (401)
        let state = InferenceUsageModuleState {
            amp_free_credit: Some(5.0),
            claude_status: ClaudeUsageStatus::AuthInvalid,
        };
        assert_eq!(
            module.render(&state),
            format!(
                "{} {ICON_AMP} %{{T0}}%{{F#819500}}▄%{{F-}}%{{T-}} {ICON_CLAUDE} {ICON_WARNING}",
                mi(ICON_INFERENCE_USAGE),
            )
        );
    }
}
