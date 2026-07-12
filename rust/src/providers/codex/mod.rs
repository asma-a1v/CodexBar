//! Codex (OpenAI/ChatGPT) provider implementation
//!
//! Fetches usage data from ChatGPT's backend API using OAuth credentials
//! stored by the Codex CLI in ~/.codex/auth.json

mod api;
mod rpc;

use async_trait::async_trait;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    SourceMode,
};

pub use api::CodexApi;

/// Codex provider for fetching AI usage limits
pub struct CodexProvider {
    metadata: ProviderMetadata,
    api: CodexApi,
    cli: rpc::CodexCli,
}

impl CodexProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Codex,
                display_name: "Codex",
                session_label: "Session",
                weekly_label: "Weekly",
                supports_opus: false,
                supports_credits: true,
                default_enabled: true,
                is_primary: true,
                dashboard_url: Some("https://chatgpt.com/codex/settings/usage"),
                status_page_url: Some("https://status.openai.com"),
            },
            api: CodexApi::new(),
            cli: rpc::CodexCli::new(),
        }
    }
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::OAuth => self.fetch_oauth(ctx.include_credits).await,
            SourceMode::Cli => self.cli.fetch_usage(ctx.include_credits).await,
            SourceMode::Auto => match self.fetch_oauth(ctx.include_credits).await {
                Ok(result) => Ok(result),
                Err(error) if should_fallback_to_cli(&error) => {
                    tracing::info!(
                        "Codex OAuth credentials were unavailable; trying the local Codex CLI"
                    );
                    self.cli.fetch_usage(ctx.include_credits).await
                }
                Err(error) => Err(error),
            },
            SourceMode::Web => Err(ProviderError::UnsupportedSource(SourceMode::Web)),
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth, SourceMode::Cli]
    }

    fn supports_oauth(&self) -> bool {
        true
    }

    fn supports_cli(&self) -> bool {
        true
    }

    fn detect_version(&self) -> Option<String> {
        detect_codex_version()
    }
}

impl CodexProvider {
    async fn fetch_oauth(
        &self,
        include_credits: bool,
    ) -> Result<ProviderFetchResult, ProviderError> {
        tracing::debug!("Fetching Codex usage via OAuth API");
        let (usage, cost) = self
            .api
            .fetch_usage(include_credits)
            .await
            .map_err(|error| {
                tracing::warn!("Codex OAuth fetch failed: {error}");
                error
            })?;
        let mut result = ProviderFetchResult::new(usage, "oauth");
        if let Some(cost) = cost {
            result = result.with_cost(cost);
        }
        Ok(result)
    }
}

fn should_fallback_to_cli(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::AuthRequired | ProviderError::NotInstalled(_) | ProviderError::OAuth(_)
    )
}

/// Try to find the codex CLI binary
pub(super) fn which_codex() -> Option<std::path::PathBuf> {
    // Check common locations on Windows
    let possible_paths = [
        // In PATH
        which::which("codex").ok(),
        which::which("codex.exe").ok(),
        // Current Codex desktop app
        dirs::data_local_dir().map(|p| {
            p.join("Programs")
                .join("OpenAI")
                .join("Codex")
                .join("bin")
                .join("codex.exe")
        }),
        // npm global install
        dirs::data_dir().map(|p| p.join("npm").join("codex.cmd")),
        // AppData locations
        dirs::data_local_dir().map(|p| p.join("Programs").join("codex").join("codex.exe")),
    ];

    possible_paths.into_iter().flatten().find(|p| p.exists())
}

/// Detect the version of the codex CLI
fn detect_codex_version() -> Option<String> {
    let codex_path = which_codex()?;

    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new(codex_path);
    cmd.args(["--version"]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd.output().ok()?;

    if output.status.success() {
        let version_str = String::from_utf8_lossy(&output.stdout);
        extract_version(&version_str)
    } else {
        None
    }
}

/// Extract version number from a string like "codex 1.2.3"
fn extract_version(s: &str) -> Option<String> {
    let re = regex_lite::Regex::new(r"(\d+(?:\.\d+)+)").ok()?;
    re.find(s).map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_falls_back_only_for_authentication_and_installation_errors() {
        assert!(should_fallback_to_cli(&ProviderError::AuthRequired));
        assert!(should_fallback_to_cli(&ProviderError::OAuth(
            "expired".to_string()
        )));
        assert!(should_fallback_to_cli(&ProviderError::NotInstalled(
            "missing".to_string()
        )));
        assert!(!should_fallback_to_cli(&ProviderError::Timeout));
        assert!(!should_fallback_to_cli(&ProviderError::Parse(
            "changed response".to_string()
        )));
        assert!(!should_fallback_to_cli(&ProviderError::Other(
            "server failed".to_string()
        )));
    }
}
