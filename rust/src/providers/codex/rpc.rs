//! Codex CLI app-server JSON-RPC client.
//!
//! The CLI source reuses the user's existing Codex login and never copies
//! credentials into CodexBar-managed storage.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

use crate::core::{
    CostSnapshot, NamedRateWindow, ProviderError, ProviderFetchResult, RateWindow, UsageSnapshot,
};

use super::which_codex;

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RPC_LINE_BYTES: usize = 1024 * 1024;

pub(super) struct CodexCli;

impl CodexCli {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) async fn fetch_usage(
        &self,
        include_credits: bool,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let binary = which_codex().ok_or_else(|| {
            ProviderError::NotInstalled(
                "Codex CLI not found. Install Codex and run `codex login` first.".to_string(),
            )
        })?;

        let mut rpc = RpcClient::start(binary).await?;
        let result = async {
            rpc.initialize().await?;
            let limits: RpcRateLimitsResponse =
                rpc.request("account/rateLimits/read", None).await?;
            let account = rpc
                .request::<RpcAccountResponse>("account/read", None)
                .await
                .ok();
            map_rpc_snapshot(limits, account, include_credits)
        }
        .await;
        rpc.shutdown().await;
        result
    }
}

struct RpcClient {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl RpcClient {
    async fn start(binary: PathBuf) -> Result<Self, ProviderError> {
        let mut command = codex_command(&binary);
        command
            .args(["-s", "read-only", "-a", "untrusted", "app-server"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn().map_err(|error| {
            ProviderError::NotInstalled(format!("Failed to start Codex CLI: {error}"))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ProviderError::Other("Codex CLI did not expose an RPC input stream.".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ProviderError::Other("Codex CLI did not expose an RPC output stream.".to_string())
        })?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
        })
    }

    async fn initialize(&mut self) -> Result<(), ProviderError> {
        self.request_with_timeout::<Value>(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "codexbar-windows",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            INITIALIZE_TIMEOUT,
        )
        .await?;
        self.send(json!({ "method": "initialized", "params": {} }))
            .await
    }

    async fn request<T: for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T, ProviderError> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout<T: for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        params: Option<Value>,
        duration: Duration,
    ) -> Result<T, ProviderError> {
        match timeout(duration, self.request_inner(method, params)).await {
            Ok(result) => result,
            Err(_) => {
                self.shutdown().await;
                Err(ProviderError::Timeout)
            }
        }
    }

    async fn request_inner<T: for<'de> Deserialize<'de>>(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<T, ProviderError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "id": id,
            "method": method,
            "params": params.unwrap_or_else(|| json!({}))
        }))
        .await?;

        loop {
            let line = self.stdout.next_line().await.map_err(|error| {
                ProviderError::Other(format!("Failed reading Codex RPC output: {error}"))
            })?;
            let Some(line) = line else {
                return Err(ProviderError::Other(
                    "Codex app-server closed its output stream.".to_string(),
                ));
            };
            if line.len() > MAX_RPC_LINE_BYTES {
                return Err(ProviderError::Parse(
                    "Codex RPC response exceeded the 1 MiB safety limit.".to_string(),
                ));
            }

            let message: Value = match serde_json::from_str(&line) {
                Ok(message) => message,
                Err(_) => continue,
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                // Notifications and replies for other request ids are ignored.
                continue;
            }
            if let Some(error) = message.get("error") {
                let summary = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                return Err(ProviderError::Other(format!(
                    "Codex RPC `{method}` failed: {summary}"
                )));
            }
            let result = message.get("result").cloned().ok_or_else(|| {
                ProviderError::Parse(format!("Codex RPC `{method}` response had no result."))
            })?;
            return serde_json::from_value(result).map_err(|error| {
                ProviderError::Parse(format!("Invalid Codex RPC `{method}` result: {error}"))
            });
        }
    }

    async fn send(&mut self, payload: Value) -> Result<(), ProviderError> {
        let mut encoded = serde_json::to_vec(&payload).map_err(|error| {
            ProviderError::Parse(format!("Failed to encode Codex RPC: {error}"))
        })?;
        encoded.push(b'\n');
        self.stdin.write_all(&encoded).await.map_err(|error| {
            ProviderError::Other(format!("Failed writing to Codex RPC: {error}"))
        })?;
        self.stdin
            .flush()
            .await
            .map_err(|error| ProviderError::Other(format!("Failed flushing Codex RPC: {error}")))
    }

    async fn shutdown(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill().await;
        }
        let _ = self.child.wait().await;
    }
}

fn codex_command(binary: &PathBuf) -> Command {
    #[cfg(windows)]
    if binary
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
    {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/s", "/c"]);
        command.arg(binary);
        return command;
    }

    Command::new(binary)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcRateLimitsResponse {
    rate_limits: RpcRateLimitSnapshot,
    #[serde(default, alias = "rate_limits_by_limit_id")]
    rate_limits_by_limit_id: Option<std::collections::HashMap<String, RpcRateLimitSnapshot>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcRateLimitSnapshot {
    #[serde(default, alias = "limit_id")]
    limit_id: Option<String>,
    #[serde(default, alias = "limit_name")]
    limit_name: Option<String>,
    primary: Option<RpcRateLimitWindow>,
    secondary: Option<RpcRateLimitWindow>,
    credits: Option<RpcCreditsSnapshot>,
    #[serde(default, alias = "individual_limit")]
    individual_limit: Option<RpcSpendControlLimit>,
    #[serde(default, alias = "plan_type")]
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcRateLimitWindow {
    used_percent: f64,
    #[serde(default, alias = "window_duration_mins")]
    window_duration_mins: Option<u32>,
    #[serde(default, alias = "resets_at")]
    resets_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcCreditsSnapshot {
    #[serde(default)]
    unlimited: bool,
    balance: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcSpendControlLimit {
    limit: Option<f64>,
    used: Option<f64>,
    #[serde(default, alias = "remaining_percent")]
    remaining_percent: Option<f64>,
    #[serde(default, alias = "resets_at")]
    resets_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RpcAccountResponse {
    account: Option<RpcAccountDetails>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RpcAccountDetails {
    #[serde(rename = "apikey")]
    ApiKey,
    Chatgpt {
        email: Option<String>,
        #[serde(default, rename = "planType", alias = "plan_type")]
        plan_type: Option<String>,
    },
}

fn map_rpc_snapshot(
    response: RpcRateLimitsResponse,
    account: Option<RpcAccountResponse>,
    include_credits: bool,
) -> Result<ProviderFetchResult, ProviderError> {
    let limits = response.rate_limits;
    let primary = limits.primary.as_ref().map(map_window);
    let secondary = limits.secondary.as_ref().map(map_window);
    let (primary, secondary) = match (primary, secondary) {
        (Some(primary), secondary) => (primary, secondary),
        (None, Some(secondary)) => (secondary, None),
        (None, None) => {
            return Err(ProviderError::Parse(
                "Codex CLI returned no rate-limit windows.".to_string(),
            ));
        }
    };

    let mut usage = UsageSnapshot::new(primary);
    if let Some(secondary) = secondary {
        usage = usage.with_secondary(secondary);
    }

    let account_details = account.and_then(|response| response.account);
    if let Some(RpcAccountDetails::Chatgpt {
        email: Some(email), ..
    }) = &account_details
    {
        usage = usage.with_email(email.clone());
    }
    let plan = match account_details {
        Some(RpcAccountDetails::Chatgpt { plan_type, .. }) => plan_type,
        _ => limits.plan_type.clone(),
    };
    if let Some(plan) = plan.filter(|plan| !plan.trim().is_empty()) {
        usage = usage.with_login_method(format_plan(&plan));
    }

    if let Some(additional) = response.rate_limits_by_limit_id {
        for (key, snapshot) in additional {
            let id = snapshot.limit_id.as_deref().unwrap_or(&key);
            let title = snapshot.limit_name.as_deref().unwrap_or(id);
            if id.eq_ignore_ascii_case("codex") || title.eq_ignore_ascii_case("codex") {
                // app-server also includes the primary Codex snapshot in this map.
                // It is already represented by the primary/secondary lanes above.
                continue;
            }
            if let Some(primary) = snapshot.primary.as_ref() {
                usage.extra_rate_windows.push(NamedRateWindow::new(
                    format!("codex-{}", slugify(id)),
                    title,
                    map_window(primary),
                ));
            }
            if let Some(secondary) = snapshot.secondary.as_ref() {
                usage.extra_rate_windows.push(NamedRateWindow::new(
                    format!("codex-{}-weekly", slugify(id)),
                    format!("{title} Weekly"),
                    map_window(secondary),
                ));
            }
        }
    }

    let mut result = ProviderFetchResult::new(usage, "cli");
    if include_credits && let Some(cost) = map_cost(&limits) {
        result = result.with_cost(cost);
    }
    Ok(result)
}

fn map_window(window: &RpcRateLimitWindow) -> RateWindow {
    let resets_at = window
        .resets_at
        .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single());
    RateWindow::with_details(
        window.used_percent,
        window.window_duration_mins,
        resets_at,
        resets_at.map(|reset| format!("resets {}", reset.to_rfc3339())),
    )
}

fn map_cost(limits: &RpcRateLimitSnapshot) -> Option<CostSnapshot> {
    if let Some(limit) = &limits.individual_limit
        && let Some(total) = limit
            .limit
            .filter(|value| value.is_finite() && *value >= 0.0)
    {
        let used = limit
            .used
            .filter(|value| value.is_finite() && *value >= 0.0)
            .or_else(|| {
                limit
                    .remaining_percent
                    .map(|remaining| total * (1.0 - remaining.clamp(0.0, 100.0) / 100.0))
            })
            .unwrap_or(0.0)
            .clamp(0.0, total);
        let mut cost = CostSnapshot::new(used, "USD", "Monthly credits").with_limit(total);
        if let Some(reset) = limit
            .resets_at
            .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
        {
            cost = cost.with_resets_at(reset);
        }
        return Some(cost);
    }

    let credits = limits.credits.as_ref()?;
    if credits.unlimited {
        return None;
    }
    let balance = credits
        .balance
        .as_ref()
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))?;
    Some(CostSnapshot::new(balance, "USD", "Credits"))
}

fn format_plan(plan: &str) -> String {
    let plan = plan.trim();
    if plan.to_ascii_lowercase().starts_with("chatgpt") {
        plan.to_string()
    } else {
        format!("ChatGPT {plan}")
    }
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_cli_rate_limits_identity_and_credits() {
        let response: RpcRateLimitsResponse = serde_json::from_value(json!({
            "rateLimits": {
                "primary": { "usedPercent": 25, "windowDurationMins": 300, "resetsAt": 2_000_000_000 },
                "secondary": { "usedPercent": 60, "windowDurationMins": 10080, "resetsAt": 2_000_100_000 },
                "credits": { "hasCredits": true, "unlimited": false, "balance": "12.5" },
                "planType": "pro"
            }
        }))
        .expect("rate limits");
        let account: RpcAccountResponse = serde_json::from_value(json!({
            "account": { "type": "chatgpt", "email": "user@example.com", "planType": "pro" }
        }))
        .expect("account");

        let result = map_rpc_snapshot(response, Some(account), true).expect("snapshot");

        assert_eq!(result.source_label, "cli");
        assert_eq!(result.usage.primary.used_percent, 25.0);
        assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 60.0);
        assert_eq!(
            result.usage.account_email.as_deref(),
            Some("user@example.com")
        );
        assert_eq!(result.usage.login_method.as_deref(), Some("ChatGPT pro"));
        assert_eq!(result.cost.as_ref().unwrap().used, 12.5);
    }

    #[test]
    fn rejects_cli_response_without_usage_windows() {
        let response: RpcRateLimitsResponse = serde_json::from_value(json!({
            "rateLimits": { "planType": "pro" }
        }))
        .expect("rate limits");

        let error = map_rpc_snapshot(response, None, false).unwrap_err();

        assert!(matches!(error, ProviderError::Parse(_)));
    }

    #[test]
    fn does_not_duplicate_primary_codex_windows_from_limit_map() {
        let response: RpcRateLimitsResponse = serde_json::from_value(json!({
            "rateLimits": {
                "primary": { "usedPercent": 25, "windowDurationMins": 300 },
                "secondary": { "usedPercent": 60, "windowDurationMins": 10080 }
            },
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "limitName": "Codex",
                    "primary": { "usedPercent": 25, "windowDurationMins": 300 },
                    "secondary": { "usedPercent": 60, "windowDurationMins": 10080 }
                },
                "codex-bengalfox": {
                    "limitName": "GPT-5.3-Codex-Spark",
                    "primary": { "usedPercent": 10, "windowDurationMins": 300 }
                }
            }
        }))
        .expect("rate limits");

        let result = map_rpc_snapshot(response, None, false).expect("snapshot");

        assert_eq!(result.usage.extra_rate_windows.len(), 1);
        assert_eq!(
            result.usage.extra_rate_windows[0].title,
            "GPT-5.3-Codex-Spark"
        );
    }
}
