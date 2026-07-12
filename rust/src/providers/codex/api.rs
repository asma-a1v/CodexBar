//! Codex API client for fetching usage information
//!
//! Uses OAuth tokens stored by the Codex CLI in ~/.codex/auth.json

use crate::core::{CostSnapshot, NamedRateWindow, ProviderError, RateWindow, UsageSnapshot};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const USAGE_PATH: &str = "/wham/usage";
const RESET_CREDITS_PATH: &str = "/wham/rate-limit-reset-credits";
const TOKEN_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
const TOKEN_REFRESH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_REFRESH_AGE_DAYS: i64 = 8;
const CREDENTIAL_CACHE_TTL: Duration = Duration::from_secs(5);

static CREDENTIAL_CACHE: OnceLock<Mutex<Option<CachedCodexCredentials>>> = OnceLock::new();

/// Codex API client
pub struct CodexApi {
    client: reqwest::Client,
    home_dir: PathBuf,
}

impl CodexApi {
    pub fn new() -> Self {
        // Build client with proper TLS settings
        let client = crate::core::credentialed_http_client_builder()
            .use_rustls_tls()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            home_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    /// Fetch usage information from Codex API
    /// Returns (UsageSnapshot, optional CostSnapshot)
    pub async fn fetch_usage(
        &self,
        include_credits: bool,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        // Load credentials
        let loaded = self.load_credentials()?;
        let creds = self.refresh_if_needed(loaded).await?;

        // Build request URL
        let base_url = self.resolve_base_url();
        let url = format!("{}{}", base_url, USAGE_PATH);

        // Build request
        let mut request = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("User-Agent", "CodexBar")
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(30));

        if let Some(account_id) = &creds.account_id
            && !account_id.is_empty()
        {
            request = request.header("ChatGPT-Account-Id", account_id);
        }

        let response = request.send().await?;

        if response.status() == 401 || response.status() == 403 {
            return Err(ProviderError::AuthRequired);
        }

        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Codex API returned {}",
                response.status()
            )));
        }

        // Parse as raw JSON first for flexibility
        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let (mut usage, cost) = self.build_result_from_json(&json)?;
        if include_credits
            && let Ok(reset_credits) = self.fetch_rate_limit_reset_credits(&creds, &base_url).await
            && reset_credits.available_count > 0
        {
            let mut window = RateWindow::new(0.0);
            window.reset_description = Some(format!(
                "{} reset credit{} available",
                reset_credits.available_count,
                if reset_credits.available_count == 1 {
                    ""
                } else {
                    "s"
                }
            ));
            usage = usage.with_extra_rate_window("reset-credits", "Reset credits", window);
        }
        Ok((usage, include_credits.then_some(cost).flatten()))
    }

    async fn fetch_rate_limit_reset_credits(
        &self,
        creds: &CodexCredentials,
        base_url: &str,
    ) -> Result<ResetCredits, ProviderError> {
        let mut request = self
            .client
            .get(format!("{}{}", base_url, RESET_CREDITS_PATH))
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("User-Agent", "CodexBar")
            .header("Accept", "application/json");
        if let Some(account_id) = &creds.account_id
            && !account_id.is_empty()
        {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "Codex reset credits returned {}",
                response.status()
            )));
        }
        decode_reset_credits(&response.bytes().await?)
    }

    fn load_credentials(&self) -> Result<LoadedCodexCredentials, ProviderError> {
        let auth_path = self.get_auth_path();

        if !auth_path.exists() {
            return Err(ProviderError::NotInstalled(
                "Codex auth.json not found. Run `codex login` in a terminal to sign in."
                    .to_string(),
            ));
        }

        let modified = std::fs::metadata(&auth_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(cached) = Self::cached_credentials(&auth_path, modified) {
            return Ok(LoadedCodexCredentials {
                credentials: cached,
                path: auth_path,
                modified,
            });
        }

        let content = std::fs::read_to_string(&auth_path).map_err(|e| {
            ProviderError::Other(format!("Failed to read Codex credentials: {}", e))
        })?;

        let credentials = Self::parse_credentials_json(&content)?;
        Self::store_cached_credentials(auth_path.clone(), modified, credentials.clone());
        Ok(LoadedCodexCredentials {
            credentials,
            path: auth_path,
            modified,
        })
    }

    fn parse_credentials_json(content: &str) -> Result<CodexCredentials, ProviderError> {
        let json: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| ProviderError::Parse(format!("Invalid Codex credentials JSON: {}", e)))?;

        // Check for OPENAI_API_KEY first
        if let Some(api_key) = json.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
            let trimmed = api_key.trim();
            if !trimmed.is_empty() {
                return Ok(CodexCredentials {
                    access_token: trimmed.to_string(),
                    refresh_token: None,
                    id_token: None,
                    account_id: None,
                    last_refresh: None,
                });
            }
        }

        // Otherwise, look for tokens object
        let tokens = json.get("tokens").ok_or_else(|| {
            ProviderError::OAuth(
                "Codex auth.json exists but contains no OAuth tokens. Run `codex login`."
                    .to_string(),
            )
        })?;

        let access_token =
            string_value(tokens, "access_token", "accessToken").ok_or_else(|| {
                ProviderError::OAuth(
                    "Codex OAuth access token is missing. Run `codex login`.".to_string(),
                )
            })?;
        let refresh_token = string_value(tokens, "refresh_token", "refreshToken");
        let id_token = string_value(tokens, "id_token", "idToken");
        let account_id = string_value(tokens, "account_id", "accountId");
        let last_refresh = json
            .get("last_refresh")
            .and_then(|value| value.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));

        Ok(CodexCredentials {
            access_token,
            refresh_token,
            id_token,
            account_id,
            last_refresh,
        })
    }

    fn credential_cache() -> &'static Mutex<Option<CachedCodexCredentials>> {
        CREDENTIAL_CACHE.get_or_init(|| Mutex::new(None))
    }

    fn cached_credentials(
        path: &std::path::Path,
        modified: Option<SystemTime>,
    ) -> Option<CodexCredentials> {
        let guard = Self::credential_cache().lock().ok()?;
        let cached = guard.as_ref()?;
        if cached.path == path
            && cached.modified == modified
            && cached.loaded_at.elapsed() <= CREDENTIAL_CACHE_TTL
        {
            return Some(cached.credentials.clone());
        }
        None
    }

    fn store_cached_credentials(
        path: PathBuf,
        modified: Option<SystemTime>,
        credentials: CodexCredentials,
    ) {
        if let Ok(mut guard) = Self::credential_cache().lock() {
            *guard = Some(CachedCodexCredentials {
                path,
                modified,
                loaded_at: Instant::now(),
                credentials,
            });
        }
    }

    fn clear_cached_credentials() {
        if let Ok(mut guard) = Self::credential_cache().lock() {
            *guard = None;
        }
    }

    async fn refresh_if_needed(
        &self,
        loaded: LoadedCodexCredentials,
    ) -> Result<CodexCredentials, ProviderError> {
        if !loaded.credentials.needs_refresh() {
            return Ok(loaded.credentials);
        }

        let refreshed = self.refresh_token(&loaded.credentials).await?;
        if self.persist_refreshed_credentials(&loaded, &refreshed)? {
            return Ok(refreshed);
        }

        // The Codex CLI updated auth.json while the refresh request was in flight.
        // Prefer the CLI's new credentials instead of overwriting a rotated token.
        tracing::info!("Codex credentials changed during refresh; using the newer auth.json");
        Self::clear_cached_credentials();
        Ok(self.load_credentials()?.credentials)
    }

    async fn refresh_token(
        &self,
        current: &CodexCredentials,
    ) -> Result<CodexCredentials, ProviderError> {
        let refresh_token = current.refresh_token.as_deref().ok_or_else(|| {
            ProviderError::OAuth(
                "Codex OAuth refresh token is missing. Run `codex login`.".to_string(),
            )
        })?;
        let response = self
            .client
            .post(TOKEN_REFRESH_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "client_id": TOKEN_REFRESH_CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "scope": "openid profile email"
            }))
            .send()
            .await?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            let code = refresh_error_code(&body);
            let message = match code.as_deref() {
                Some("refresh_token_expired") => "Codex refresh token expired. Run `codex login`.",
                Some("refresh_token_reused") => {
                    "Codex refresh token was already used. Run `codex login`."
                }
                Some("invalid_grant") | Some("refresh_token_invalidated") => {
                    "Codex refresh token was revoked. Run `codex login`."
                }
                _ if status.as_u16() == 401 => {
                    "Codex refresh token is no longer valid. Run `codex login`."
                }
                _ => {
                    return Err(ProviderError::Other(format!(
                        "Codex token refresh returned {status}"
                    )));
                }
            };
            return Err(ProviderError::OAuth(message.to_string()));
        }

        let payload: RefreshTokenResponse = serde_json::from_slice(&body).map_err(|error| {
            ProviderError::Parse(format!("Invalid Codex token refresh response: {error}"))
        })?;
        Ok(CodexCredentials {
            access_token: payload
                .access_token
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| current.access_token.clone()),
            refresh_token: payload
                .refresh_token
                .filter(|value| !value.is_empty())
                .or_else(|| current.refresh_token.clone()),
            id_token: payload
                .id_token
                .filter(|value| !value.is_empty())
                .or_else(|| current.id_token.clone()),
            account_id: current.account_id.clone(),
            last_refresh: Some(Utc::now()),
        })
    }

    fn persist_refreshed_credentials(
        &self,
        loaded: &LoadedCodexCredentials,
        refreshed: &CodexCredentials,
    ) -> Result<bool, ProviderError> {
        let current_modified = std::fs::metadata(&loaded.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if current_modified != loaded.modified {
            return Ok(false);
        }

        let content = std::fs::read_to_string(&loaded.path).map_err(|error| {
            ProviderError::OAuth(format!("Failed to reread Codex auth.json: {error}"))
        })?;
        let current = Self::parse_credentials_json(&content)?;
        if current.access_token != loaded.credentials.access_token
            || current.refresh_token != loaded.credentials.refresh_token
        {
            return Ok(false);
        }

        let mut root: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
            ProviderError::Parse(format!("Invalid Codex credentials JSON: {error}"))
        })?;
        apply_refreshed_credentials_json(&mut root, refreshed)?;
        let serialized = serde_json::to_vec_pretty(&root).map_err(|error| {
            ProviderError::Parse(format!("Failed to serialize Codex credentials: {error}"))
        })?;
        let parent = loaded.path.parent().ok_or_else(|| {
            ProviderError::OAuth("Codex auth.json has no parent directory.".to_string())
        })?;
        let temporary = parent.join(format!(".auth.json.codexbar-tmp-{}", uuid::Uuid::new_v4()));
        std::fs::write(&temporary, serialized).map_err(|error| {
            ProviderError::OAuth(format!("Failed to stage Codex credentials: {error}"))
        })?;
        restrict_private_permissions(&temporary)?;
        if let Err(error) = std::fs::rename(&temporary, &loaded.path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(ProviderError::OAuth(format!(
                "Failed to replace Codex auth.json: {error}"
            )));
        }

        let modified = std::fs::metadata(&loaded.path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        Self::store_cached_credentials(loaded.path.clone(), modified, refreshed.clone());
        Ok(true)
    }

    fn get_auth_path(&self) -> PathBuf {
        // Check CODEX_HOME env var
        if let Ok(codex_home) = std::env::var("CODEX_HOME") {
            let trimmed = codex_home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("auth.json");
            }
        }

        self.home_dir.join(".codex").join("auth.json")
    }

    fn resolve_base_url(&self) -> String {
        // Check CODEX_HOME for config.toml
        let config_path = if let Ok(codex_home) = std::env::var("CODEX_HOME") {
            let trimmed = codex_home.trim();
            if !trimmed.is_empty() {
                PathBuf::from(trimmed).join("config.toml")
            } else {
                self.home_dir.join(".codex").join("config.toml")
            }
        } else {
            self.home_dir.join(".codex").join("config.toml")
        };

        if let Ok(content) = std::fs::read_to_string(&config_path)
            && let Some(base_url) = parse_chatgpt_base_url(&content)
        {
            let normalized = normalize_base_url(&base_url);
            // Only allow HTTPS URLs for custom base URLs to prevent token exfiltration
            if normalized.starts_with("https://")
                || normalized.starts_with("http://127.0.0.1")
                || normalized.starts_with("http://localhost")
            {
                return normalized;
            }
            tracing::warn!(
                "Ignoring insecure custom chatgpt_base_url (must be HTTPS): {}",
                normalized
            );
        }

        DEFAULT_BASE_URL.to_string()
    }

    fn build_result_from_json(
        &self,
        json: &serde_json::Value,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        // Extract plan type
        let plan_type = json
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Extract rate limit info - handle multiple possible structures
        let (primary, secondary, code_review) = self.extract_rate_limits(json);

        // Build login method string
        let login_method = plan_type.as_ref().map(|pt| match pt.as_str() {
            "guest" => "Guest".to_string(),
            "free" => "ChatGPT Free".to_string(),
            "go" => "Codex Go".to_string(),
            "plus" => "ChatGPT Plus".to_string(),
            "pro" | "pro_lite" | "prolite" | "pro-lite" => {
                if pt == "pro" {
                    "ChatGPT Pro".to_string()
                } else {
                    "Pro Lite".to_string()
                }
            }
            "team" => "ChatGPT Team".to_string(),
            "business" => "ChatGPT Business".to_string(),
            "enterprise" => "ChatGPT Enterprise".to_string(),
            "education" | "edu" => "ChatGPT Education".to_string(),
            "free_workspace" | "freeWorkspace" => "Free Workspace".to_string(),
            "quorum" => "Codex Quorum".to_string(),
            "k12" => "Codex K12".to_string(),
            other => format!("ChatGPT {}", capitalize(other)),
        });

        let mut usage = UsageSnapshot::new(primary);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        if let Some(cr) = code_review {
            usage = usage.with_model_specific(cr);
        }
        for extra in self.extract_additional_rate_limits(json) {
            usage.extra_rate_windows.push(extra);
        }
        if let Some(method) = login_method {
            usage = usage.with_login_method(method);
        }

        // Extract credits if present
        let cost = self.extract_credits(json);

        Ok((usage, cost))
    }

    fn extract_rate_limits(
        &self,
        json: &serde_json::Value,
    ) -> (RateWindow, Option<RateWindow>, Option<RateWindow>) {
        // Try rate_limit object
        if let Some(rate_limit) = json.get("rate_limit") {
            let primary_opt = rate_limit
                .get("primary_window")
                .map(|w| self.parse_window(w));

            let secondary_opt = rate_limit
                .get("secondary_window")
                .map(|w| self.parse_window(w));

            let code_review = rate_limit
                .get("code_review_window")
                .map(|w| self.parse_window(w));

            // If primary is missing, promote secondary to primary (weekly-only plans)
            let (primary, secondary) = match (primary_opt, secondary_opt) {
                (Some(p), s) => (p, s),
                (None, Some(s)) => (s, None),
                (None, None) => (RateWindow::new(0.0), None),
            };

            return (primary, secondary, code_review);
        }

        // Try rate_limits array
        if let Some(rate_limits) = json.get("rate_limits").and_then(|v| v.as_array())
            && let Some(first) = rate_limits.first()
        {
            let primary = self.parse_window(first);
            let secondary = rate_limits.get(1).map(|w| self.parse_window(w));
            let code_review = rate_limits.get(2).map(|w| self.parse_window(w));
            return (primary, secondary, code_review);
        }

        // Try direct fields
        let used_percent = json
            .get("used_percent")
            .or_else(|| json.get("usage_percent"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        (RateWindow::new(used_percent), None, None)
    }

    fn parse_window(&self, window: &serde_json::Value) -> RateWindow {
        let used_percent = window
            .get("used_percent")
            .or_else(|| window.get("usage_percent"))
            .and_then(json_f64)
            .unwrap_or(0.0);

        let window_minutes = window
            .get("limit_window_seconds")
            .and_then(|v| v.as_i64())
            .map(|s| (s / 60) as u32);

        let reset_at = window
            .get("reset_at")
            .and_then(|v| v.as_i64())
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single());

        RateWindow::with_details(
            used_percent,
            window_minutes,
            reset_at,
            format_reset_countdown(reset_at),
        )
    }

    fn extract_additional_rate_limits(&self, json: &serde_json::Value) -> Vec<NamedRateWindow> {
        json.get("additional_rate_limits")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| self.parse_additional_rate_limit(entry))
            .collect()
    }

    fn parse_additional_rate_limit(&self, entry: &serde_json::Value) -> Option<NamedRateWindow> {
        let metered_feature = entry
            .get("metered_feature")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let limit_name = entry
            .get("limit_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty());

        let rate_limit = entry.get("rate_limit").unwrap_or(entry);
        let primary = rate_limit.get("primary_window");
        let secondary = rate_limit.get("secondary_window");
        let window = primary.or(secondary)?;
        if is_placeholder_window(window) {
            return None;
        }

        let parsed = self.parse_window(window);
        let feature = metered_feature.unwrap_or_default();
        let limit = limit_name.unwrap_or_default();
        let is_spark = feature.eq_ignore_ascii_case("codex_spark")
            || feature.eq_ignore_ascii_case("spark")
            || limit.to_ascii_lowercase().contains("spark");

        if is_spark {
            let is_weekly = secondary.is_some() && primary.is_none()
                || parsed
                    .window_minutes
                    .is_some_and(|mins| mins >= 7 * 24 * 60);
            let (id, title) = if is_weekly {
                ("codex-spark-weekly", "Codex Spark Weekly")
            } else {
                ("codex-spark", "Codex Spark 5-hour")
            };
            return Some(NamedRateWindow::new(id, title, parsed));
        }

        let label = limit_name.or(metered_feature)?;
        let slug = slugify(label);
        if slug.is_empty() {
            return None;
        }

        Some(NamedRateWindow::new(
            format!("codex-{slug}"),
            titleize_limit_label(label),
            parsed,
        ))
    }

    fn extract_credits(&self, json: &serde_json::Value) -> Option<CostSnapshot> {
        let credits = json.get("credits")?;

        let has_credits = credits
            .get("has_credits")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !has_credits {
            return None;
        }

        let unlimited = credits
            .get("unlimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if unlimited {
            return None;
        }

        let balance = credits
            .get("balance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        Some(CostSnapshot::new(balance, "USD", "Credits"))
    }

    fn build_result(
        &self,
        response: UsageResponse,
    ) -> Result<(UsageSnapshot, Option<CostSnapshot>), ProviderError> {
        // Extract primary rate window
        let primary = if let Some(ref rate_limit) = response.rate_limit {
            if let Some(ref primary_window) = rate_limit.primary_window {
                let reset_at = timestamp_to_datetime(primary_window.reset_at);
                RateWindow::with_details(
                    primary_window.used_percent as f64,
                    primary_window.limit_window_seconds.map(|s| (s / 60) as u32),
                    reset_at,
                    format_reset_countdown(reset_at),
                )
            } else {
                RateWindow::new(0.0)
            }
        } else {
            RateWindow::new(0.0)
        };

        // Extract secondary rate window
        let secondary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.secondary_window.as_ref())
            .map(|window| {
                let reset_at = timestamp_to_datetime(window.reset_at);
                RateWindow::with_details(
                    window.used_percent as f64,
                    window.limit_window_seconds.map(|s| (s / 60) as u32),
                    reset_at,
                    format_reset_countdown(reset_at),
                )
            });

        // Extract code review rate window
        let code_review = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.code_review_window.as_ref())
            .map(|window| {
                let reset_at = timestamp_to_datetime(window.reset_at);
                RateWindow::with_details(
                    window.used_percent as f64,
                    window.limit_window_seconds.map(|s| (s / 60) as u32),
                    reset_at,
                    format_reset_countdown(reset_at),
                )
            });

        // Build usage snapshot
        let login_method = response.plan_type.as_ref().map(|pt| match pt.as_str() {
            "guest" => "Guest".to_string(),
            "free" => "ChatGPT Free".to_string(),
            "go" => "ChatGPT Go".to_string(),
            "plus" => "ChatGPT Plus".to_string(),
            "pro" => "ChatGPT Pro".to_string(),
            "team" => "ChatGPT Team".to_string(),
            "business" => "ChatGPT Business".to_string(),
            "enterprise" => "ChatGPT Enterprise".to_string(),
            "education" | "edu" => "ChatGPT Education".to_string(),
            other => format!("ChatGPT {}", capitalize(other)),
        });

        let mut usage = UsageSnapshot::new(primary);
        if let Some(sec) = secondary {
            usage = usage.with_secondary(sec);
        }
        if let Some(cr) = code_review {
            usage = usage.with_model_specific(cr);
        }
        if let Some(method) = login_method {
            usage = usage.with_login_method(method);
        }

        // Build cost snapshot if credits are present
        let credit_limit = response.individual_limit.as_ref().or_else(|| {
            response
                .rate_limit
                .as_ref()
                .and_then(|rate_limit| rate_limit.individual_limit.as_ref())
        });
        let cost = response.credits.as_ref().and_then(|credits| {
            if credits.has_credits() {
                let balance = credits.balance.unwrap_or(0.0);
                if credits.unlimited() {
                    None // Unlimited credits, no need to show
                } else if let Some(limit) =
                    credit_limit.and_then(|limit| limit.to_cost_snapshot(balance))
                {
                    Some(limit)
                } else {
                    Some(CostSnapshot::new(balance, "USD", "Credits"))
                }
            } else {
                None
            }
        });

        Ok((usage, cost))
    }
}

impl Default for CodexApi {
    fn default() -> Self {
        Self::new()
    }
}

// --- Data structures ---

#[derive(Clone)]
struct CodexCredentials {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
    last_refresh: Option<DateTime<Utc>>,
}

impl CodexCredentials {
    fn needs_refresh(&self) -> bool {
        if self.refresh_token.as_deref().is_none_or(str::is_empty) {
            return false;
        }
        let Some(last_refresh) = self.last_refresh else {
            return true;
        };
        Utc::now() - last_refresh >= chrono::Duration::days(TOKEN_REFRESH_AGE_DAYS)
    }
}

struct LoadedCodexCredentials {
    credentials: CodexCredentials,
    path: PathBuf,
    modified: Option<SystemTime>,
}

struct CachedCodexCredentials {
    path: PathBuf,
    modified: Option<SystemTime>,
    loaded_at: Instant,
    credentials: CodexCredentials,
}

#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDetails>,
    credits: Option<CreditDetails>,
    #[serde(default, alias = "individualLimit")]
    individual_limit: Option<SpendControlLimitSnapshot>,
}

#[derive(Debug, Deserialize)]
struct RateLimitDetails {
    primary_window: Option<WindowSnapshot>,
    secondary_window: Option<WindowSnapshot>,
    code_review_window: Option<WindowSnapshot>,
    #[serde(default, alias = "individualLimit")]
    individual_limit: Option<SpendControlLimitSnapshot>,
}

#[derive(Debug, Deserialize)]
struct WindowSnapshot {
    used_percent: i32,
    reset_at: Option<i64>,
    limit_window_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreditDetails {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    balance: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct SpendControlLimitSnapshot {
    limit: Option<f64>,
    used: Option<f64>,
    #[serde(default, alias = "remainingPercent")]
    remaining_percent: Option<f64>,
    #[serde(default, alias = "resetsAt")]
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResetCredits {
    #[serde(default)]
    credits: Vec<serde_json::Value>,
    #[serde(default)]
    available_count: u32,
}

fn decode_reset_credits(data: &[u8]) -> Result<ResetCredits, ProviderError> {
    serde_json::from_slice(data)
        .map_err(|e| ProviderError::Parse(format!("Failed to parse Codex reset credits: {e}")))
}

impl CreditDetails {
    // Helper to safely check has_credits
    fn has_credits(&self) -> bool {
        self.has_credits.unwrap_or(false)
    }

    fn unlimited(&self) -> bool {
        self.unlimited.unwrap_or(false)
    }
}

impl SpendControlLimitSnapshot {
    fn to_cost_snapshot(&self, balance: f64) -> Option<CostSnapshot> {
        let limit = self
            .limit
            .filter(|limit| limit.is_finite() && *limit >= 0.0)?;
        let used = self
            .used
            .filter(|used| used.is_finite() && *used >= 0.0)
            .or_else(|| {
                self.remaining_percent
                    .filter(|pct| pct.is_finite() && *pct >= 0.0)
                    .map(|remaining| limit * (1.0 - (remaining / 100.0)))
            })
            .unwrap_or_else(|| (limit - balance).max(0.0));
        let mut cost =
            CostSnapshot::new(used.clamp(0.0, limit), "USD", "Monthly credits").with_limit(limit);
        if let Some(resets_at) = timestamp_to_datetime(self.resets_at) {
            cost = cost.with_resets_at(resets_at);
        }
        Some(cost)
    }
}

// --- Helper functions ---

fn string_value(value: &serde_json::Value, snake_case: &str, camel_case: &str) -> Option<String> {
    value
        .get(snake_case)
        .or_else(|| value.get(camel_case))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn refresh_error_code(data: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(data).ok()?;
    let code = value
        .get("error")
        .and_then(|error| {
            error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .or_else(|| error.as_str())
        })
        .or_else(|| value.get("code").and_then(serde_json::Value::as_str))?;
    Some(code.to_ascii_lowercase())
}

fn apply_refreshed_credentials_json(
    root: &mut serde_json::Value,
    credentials: &CodexCredentials,
) -> Result<(), ProviderError> {
    let tokens = root
        .get_mut("tokens")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            ProviderError::OAuth("Codex auth.json is missing its tokens object.".to_string())
        })?;
    tokens.insert(
        "access_token".to_string(),
        serde_json::Value::String(credentials.access_token.clone()),
    );
    if let Some(refresh_token) = &credentials.refresh_token {
        tokens.insert(
            "refresh_token".to_string(),
            serde_json::Value::String(refresh_token.clone()),
        );
    }
    if let Some(id_token) = &credentials.id_token {
        tokens.insert(
            "id_token".to_string(),
            serde_json::Value::String(id_token.clone()),
        );
    }
    if let Some(account_id) = &credentials.account_id {
        tokens.insert(
            "account_id".to_string(),
            serde_json::Value::String(account_id.clone()),
        );
    }
    root["last_refresh"] = serde_json::Value::String(
        credentials
            .last_refresh
            .unwrap_or_else(Utc::now)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    );
    Ok(())
}

#[cfg(unix)]
fn restrict_private_permissions(path: &std::path::Path) -> Result<(), ProviderError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| {
            ProviderError::OAuth(format!("Failed to inspect staged credentials: {error}"))
        })?
        .permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(|error| {
        ProviderError::OAuth(format!("Failed to protect staged credentials: {error}"))
    })
}

#[cfg(not(unix))]
fn restrict_private_permissions(_path: &std::path::Path) -> Result<(), ProviderError> {
    Ok(())
}

fn timestamp_to_datetime(timestamp: Option<i64>) -> Option<DateTime<Utc>> {
    timestamp.and_then(|ts| Utc.timestamp_opt(ts, 0).single())
}

fn json_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
}

fn is_placeholder_window(window: &serde_json::Value) -> bool {
    let has_usage = window
        .get("used_percent")
        .or_else(|| window.get("usage_percent"))
        .and_then(json_f64)
        .is_some();
    let has_duration = window
        .get("limit_window_seconds")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse::<i64>().ok()))
        .is_some();
    let has_reset = window.get("reset_at").is_some();

    !has_usage && !has_duration && !has_reset
}

fn slugify(label: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn titleize_limit_label(label: &str) -> String {
    label
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

fn format_reset_countdown(reset_at: Option<DateTime<Utc>>) -> Option<String> {
    let dt = reset_at?;
    let now = Utc::now();
    if dt <= now {
        return Some("now".to_string());
    }
    let diff = dt - now;
    let total_mins = diff.num_minutes();
    let hours = diff.num_hours();
    let mins = total_mins % 60;
    if hours >= 24 {
        let days = hours / 24;
        let rem_h = hours % 24;
        if rem_h == 0 {
            Some(format!("{}d", days))
        } else {
            Some(format!("{}d {}h", days, rem_h))
        }
    } else if hours > 0 {
        if mins == 0 {
            Some(format!("{}h", hours))
        } else {
            Some(format!("{}h {}m", hours, mins))
        }
    } else {
        Some(format!("{}m", mins))
    }
}

fn parse_chatgpt_base_url(config_content: &str) -> Option<String> {
    for line in config_content.lines() {
        // Skip comments
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        // Look for chatgpt_base_url = "..."
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key == "chatgpt_base_url" {
                let mut value = value.trim();
                // Remove quotes
                if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    value = &value[1..value.len() - 1];
                }
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn normalize_base_url(url: &str) -> String {
    let mut trimmed = url.trim().to_string();
    if trimmed.is_empty() {
        return DEFAULT_BASE_URL.to_string();
    }

    // Remove trailing slashes
    while trimmed.ends_with('/') {
        trimmed.pop();
    }

    // Add /backend-api if needed
    if (trimmed.starts_with("https://chatgpt.com")
        || trimmed.starts_with("https://chat.openai.com"))
        && !trimmed.contains("/backend-api")
    {
        trimmed.push_str("/backend-api");
    }

    trimmed
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_codex_credentials_for_refresh() {
        let credentials = CodexApi::parse_credentials_json(
            r#"{
                "last_refresh": "2026-07-01T12:00:00Z",
                "tokens": {
                    "access_token": "access",
                    "refresh_token": "refresh",
                    "id_token": "identity",
                    "account_id": "acct_123"
                }
            }"#,
        )
        .expect("credentials");

        assert_eq!(credentials.access_token, "access");
        assert_eq!(credentials.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(credentials.id_token.as_deref(), Some("identity"));
        assert_eq!(credentials.account_id.as_deref(), Some("acct_123"));
        assert!(credentials.last_refresh.is_some());
    }

    #[test]
    fn refresh_merge_preserves_unrelated_auth_fields() {
        let mut root = json!({
            "OPENAI_API_KEY": null,
            "custom": { "keep": true },
            "tokens": {
                "access_token": "old-access",
                "refresh_token": "old-refresh",
                "id_token": "old-id",
                "account_id": "acct_123",
                "extra": "keep"
            }
        });
        let refreshed = CodexCredentials {
            access_token: "new-access".to_string(),
            refresh_token: Some("new-refresh".to_string()),
            id_token: Some("new-id".to_string()),
            account_id: Some("acct_123".to_string()),
            last_refresh: Some(Utc.timestamp_opt(2_000_000_000, 0).unwrap()),
        };

        apply_refreshed_credentials_json(&mut root, &refreshed).expect("merge");

        assert_eq!(root["custom"]["keep"], true);
        assert_eq!(root["tokens"]["extra"], "keep");
        assert_eq!(root["tokens"]["access_token"], "new-access");
        assert_eq!(root["tokens"]["refresh_token"], "new-refresh");
        assert_eq!(root["tokens"]["id_token"], "new-id");
        assert_eq!(root["tokens"]["account_id"], "acct_123");
        assert_eq!(root["last_refresh"], "2033-05-18T03:33:20.000Z");
    }

    #[test]
    fn api_key_credentials_do_not_attempt_oauth_refresh() {
        let credentials = CodexApi::parse_credentials_json(r#"{ "OPENAI_API_KEY": "sk-test" }"#)
            .expect("credentials");

        assert!(!credentials.needs_refresh());
        assert!(credentials.refresh_token.is_none());
    }

    #[test]
    fn recognizes_refresh_error_codes_without_exposing_response_text() {
        assert_eq!(
            refresh_error_code(br#"{"error":{"code":"refresh_token_reused","message":"secret"}}"#)
                .as_deref(),
            Some("refresh_token_reused")
        );
    }

    #[test]
    fn persists_refreshed_tokens_atomically_on_windows() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("auth.json");
        let original = r#"{
            "custom": { "keep": true },
            "last_refresh": "2026-06-01T00:00:00Z",
            "tokens": {
                "access_token": "old-access",
                "refresh_token": "old-refresh",
                "account_id": "acct_123"
            }
        }"#;
        std::fs::write(&path, original).expect("write auth");
        let credentials = CodexApi::parse_credentials_json(original).expect("credentials");
        let loaded = LoadedCodexCredentials {
            credentials,
            path: path.clone(),
            modified: std::fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
        };
        let refreshed = CodexCredentials {
            access_token: "new-access".to_string(),
            refresh_token: Some("new-refresh".to_string()),
            id_token: Some("new-id".to_string()),
            account_id: Some("acct_123".to_string()),
            last_refresh: Some(Utc.timestamp_opt(2_000_000_000, 0).unwrap()),
        };
        let api = CodexApi::new();

        assert!(
            api.persist_refreshed_credentials(&loaded, &refreshed)
                .expect("persist")
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read persisted auth"))
                .expect("saved json");
        assert_eq!(saved["custom"]["keep"], true);
        assert_eq!(saved["tokens"]["access_token"], "new-access");
        assert_eq!(saved["tokens"]["refresh_token"], "new-refresh");
    }

    #[test]
    fn does_not_overwrite_credentials_changed_by_codex_cli() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("auth.json");
        let original = r#"{
            "tokens": {
                "access_token": "old-access",
                "refresh_token": "old-refresh"
            }
        }"#;
        std::fs::write(&path, original).expect("write auth");
        let loaded = LoadedCodexCredentials {
            credentials: CodexApi::parse_credentials_json(original).expect("credentials"),
            path: path.clone(),
            modified: std::fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok()),
        };
        std::fs::write(
            &path,
            r#"{"tokens":{"access_token":"cli-access","refresh_token":"cli-refresh"}}"#,
        )
        .expect("simulate CLI rotation");
        let refreshed = CodexCredentials {
            access_token: "app-access".to_string(),
            refresh_token: Some("app-refresh".to_string()),
            id_token: None,
            account_id: None,
            last_refresh: Some(Utc::now()),
        };

        assert!(
            !CodexApi::new()
                .persist_refreshed_credentials(&loaded, &refreshed)
                .expect("concurrent update")
        );
        let saved = std::fs::read_to_string(&path).expect("read auth");
        assert!(saved.contains("cli-refresh"));
        assert!(!saved.contains("app-refresh"));
    }

    #[test]
    fn decodes_reset_credits() {
        let credits = decode_reset_credits(br#"{"available_count":2,"credits":[{"id":"a"}]}"#)
            .expect("reset credits");
        assert_eq!(credits.available_count, 2);
        assert_eq!(credits.credits.len(), 1);
    }

    #[test]
    fn maps_codex_spark_additional_rate_limits() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": { "used_percent": 20, "limit_window_seconds": 18000 },
                    "secondary_window": { "used_percent": 40, "limit_window_seconds": 604800 }
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "Codex Spark",
                        "metered_feature": "codex_spark",
                        "rate_limit": {
                            "primary_window": { "used_percent": "17", "limit_window_seconds": 18000 }
                        }
                    },
                    {
                        "limit_name": "Codex Spark Weekly",
                        "metered_feature": "codex_spark",
                        "rate_limit": {
                            "secondary_window": { "used_percent": 62, "limit_window_seconds": 604800 }
                        }
                    }
                ]
            }))
            .expect("codex usage");

        assert_eq!(usage.extra_rate_windows.len(), 2);
        assert_eq!(usage.extra_rate_windows[0].id, "codex-spark");
        assert_eq!(usage.extra_rate_windows[0].title, "Codex Spark 5-hour");
        assert_eq!(usage.extra_rate_windows[0].window.used_percent, 17.0);
        assert_eq!(usage.extra_rate_windows[1].id, "codex-spark-weekly");
        assert_eq!(usage.extra_rate_windows[1].title, "Codex Spark Weekly");
        assert_eq!(usage.extra_rate_windows[1].window.used_percent, 62.0);
    }

    #[test]
    fn ignores_placeholder_additional_rate_limits() {
        let api = CodexApi::new();
        let (usage, _) = api
            .build_result_from_json(&json!({
                "rate_limit": {
                    "primary_window": { "used_percent": 0, "limit_window_seconds": 18000 }
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "placeholder",
                        "metered_feature": "placeholder",
                        "rate_limit": { "primary_window": {} }
                    }
                ]
            }))
            .expect("codex usage");

        assert!(usage.extra_rate_windows.is_empty());
    }

    #[test]
    fn maps_top_level_individual_credit_limit_to_cost_snapshot() {
        let api = CodexApi::new();
        let (_, cost) = api
            .build_result(UsageResponse {
                plan_type: None,
                rate_limit: None,
                credits: Some(CreditDetails {
                    has_credits: Some(true),
                    unlimited: Some(false),
                    balance: Some(7.5),
                }),
                individual_limit: Some(SpendControlLimitSnapshot {
                    limit: Some(20.0),
                    used: Some(12.5),
                    remaining_percent: None,
                    resets_at: Some(1783036800),
                }),
            })
            .expect("codex result");
        let cost = cost.expect("cost");
        assert_eq!(cost.used, 12.5);
        assert_eq!(cost.limit, Some(20.0));
        assert!(cost.resets_at.is_some());
    }

    #[test]
    fn maps_nested_individual_credit_limit_to_cost_snapshot() {
        let api = CodexApi::new();
        let (_, cost) = api
            .build_result(UsageResponse {
                plan_type: None,
                rate_limit: Some(RateLimitDetails {
                    primary_window: None,
                    secondary_window: None,
                    code_review_window: None,
                    individual_limit: Some(SpendControlLimitSnapshot {
                        limit: Some(100.0),
                        used: None,
                        remaining_percent: Some(60.0),
                        resets_at: None,
                    }),
                }),
                credits: Some(CreditDetails {
                    has_credits: Some(true),
                    unlimited: Some(false),
                    balance: Some(60.0),
                }),
                individual_limit: None,
            })
            .expect("codex result");
        let cost = cost.expect("cost");
        assert_eq!(cost.used, 40.0);
        assert_eq!(cost.limit, Some(100.0));
    }
}
