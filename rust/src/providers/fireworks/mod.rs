//! Fireworks AI provider implementation.
//!
//! Fetches 30-day rated billing spend from the Fireworks billing API:
//! `GET https://api.fireworks.ai/v1/accounts/{slug}/billing/summary?startTime=&endTime=`
//!
//! Fireworks is prepaid with no quota windows and exposes no credit-balance
//! API, so rated spend is the only usable usage signal (upstream 0.49.0
//! #2687). Ported from steipete/CodexBar `FireworksUsageFetcher`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::core::{
    CostSnapshot, FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId,
    ProviderMetadata, RateWindow, SourceMode, UsageSnapshot,
};

const BILLING_SUMMARY_URL: &str = "https://api.fireworks.ai/v1/accounts";
const CREDENTIAL_TARGET: &str = "codexbar-fireworks";
const ENV_KEYS: &[&str] = &["FIREWORKS_API_KEY"];
const SLUG_ENV_KEYS: &[&str] = &["FIREWORKS_ACCOUNT_SLUG"];
const LOOKBACK_DAYS: i64 = 30;
/// Characters permitted in a Fireworks account slug. Slugs are simple
/// lower-case ASCII path segments; restricting to this explicit ASCII set
/// means a misconfigured slug can never widen the request path or inject a
/// query (upstream `accountSlugAllowedCharacters`).
const SLUG_ALLOWED: fn(char) -> bool =
    |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingSummaryResponse {
    #[serde(default)]
    line_items: Vec<LineItem>,
    #[serde(default)]
    #[allow(dead_code)]
    usage_buckets: Vec<UsageBucket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineItem {
    #[serde(default)]
    #[allow(dead_code)]
    category: Option<String>,
    #[serde(default)]
    total_cost: Option<Money>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Money {
    currency_code: Option<String>,
    nanos: Option<i64>,
    /// Google-style money `units` serialized as a string.
    units: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageBucket {
    #[serde(default)]
    #[allow(dead_code)]
    bucket_start_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct FireworksSummary {
    last_30_days_spend: Option<f64>,
    currency_code: Option<String>,
}

impl FireworksSummary {
    fn from_response(response: &BillingSummaryResponse) -> Self {
        // Rated line items arrive grouped by category/model; the newest-rated
        // currency decides the display currency and only rows in that
        // currency are summed (upstream `parseSummary`).
        let mut currency: Option<String> = None;
        let mut total = 0.0_f64;
        for item in &response.line_items {
            let Some(cost) = item.total_cost.as_ref() else {
                continue;
            };
            let Some(units) = cost
                .units
                .as_deref()
                .and_then(|units| units.parse::<f64>().ok())
            else {
                continue;
            };
            let Some(code) = cost
                .currency_code
                .as_deref()
                .map(str::trim)
                .filter(|code| !code.is_empty())
            else {
                continue;
            };
            if currency.is_none() {
                currency = Some(code.to_string());
            }
            if currency.as_deref() != Some(code) {
                continue;
            }
            total += units + cost.nanos.unwrap_or(0) as f64 / 1_000_000_000.0;
        }

        Self {
            last_30_days_spend: currency.as_ref().map(|_| total),
            currency_code: currency,
        }
    }

    fn to_usage_snapshot(&self) -> UsageSnapshot {
        // Fireworks is prepaid with no quota windows, so no RateWindows are
        // synthesized; the spend text rides the primary description (upstream
        // emits a cost-only snapshot).
        let spend_text = self
            .last_30_days_spend
            .zip(self.currency_code.as_deref())
            .map(|(spend, _)| format_money(spend));
        let mut primary = RateWindow::new(0.0);
        primary.reset_description = spend_text.clone();
        let mut snapshot = UsageSnapshot::new(primary);
        if let Some(text) = spend_text {
            snapshot = snapshot.with_login_method(text);
        }
        snapshot
    }

    fn to_cost_snapshot(&self) -> Option<CostSnapshot> {
        let spend = self.last_30_days_spend?;
        let currency = self.currency_code.as_deref().unwrap_or("USD");
        Some(CostSnapshot::new(spend, currency, "Last 30 days"))
    }
}

fn format_money(value: f64) -> String {
    format!("${value:.2}")
}

pub struct FireworksProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl FireworksProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::Fireworks,
                display_name: "Fireworks",
                session_label: "Spend",
                weekly_label: "Spend",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://app.fireworks.ai"),
                status_page_url: None,
            },
            client: crate::core::credentialed_http_client_builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn resolve_api_key(api_key: Option<&str>) -> Result<String, ProviderError> {
        let raw = crate::providers::resolve_api_key(api_key, CREDENTIAL_TARGET, ENV_KEYS)?;
        let cleaned = raw.trim().to_string();
        if cleaned.is_empty() {
            return Err(ProviderError::NotInstalled(
                "Missing Fireworks API key. Add one in Settings or set FIREWORKS_API_KEY."
                    .to_string(),
            ));
        }
        Ok(cleaned)
    }

    /// Account slug from settings (provider workspace slot) or
    /// `FIREWORKS_ACCOUNT_SLUG`. Validated against the upstream slug charset
    /// so a bad slug surfaces as a config error, not a widened request path.
    fn resolve_account_slug(ctx: &FetchContext) -> Result<String, ProviderError> {
        let from_env = SLUG_ENV_KEYS.iter().find_map(|key| std::env::var(key).ok());
        let raw = from_env
            .or_else(|| ctx.workspace_id.as_deref().map(str::to_string))
            .unwrap_or_default();
        let slug = raw.trim().to_string();
        if slug.is_empty() {
            return Err(ProviderError::NotInstalled(
                "Fireworks needs the account slug from app.fireworks.ai/accounts/<slug>. Set FIREWORKS_ACCOUNT_SLUG or the slug field in Settings."
                    .to_string(),
            ));
        }
        if !slug.chars().all(SLUG_ALLOWED) {
            return Err(ProviderError::Other(format!(
                "Invalid Fireworks account slug '{slug}'. Please double-check the account slug in Settings."
            )));
        }
        Ok(slug)
    }

    fn summary_url(slug: &str, now: DateTime<Utc>) -> String {
        let start = now - chrono::Duration::days(LOOKBACK_DAYS);
        format!(
            "{BILLING_SUMMARY_URL}/{slug}/billing/summary?startTime={}&endTime={}",
            start.to_rfc3339(),
            now.to_rfc3339()
        )
    }

    async fn fetch_usage_api(
        &self,
        ctx: &FetchContext,
    ) -> Result<ProviderFetchResult, ProviderError> {
        let api_key = Self::resolve_api_key(ctx.api_key.as_deref())?;
        let slug = Self::resolve_account_slug(ctx)?;
        let url = Self::summary_url(&slug, Utc::now());

        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::Other(
                "Fireworks rejected the API key. Create a new key at app.fireworks.ai and update Settings."
                    .to_string(),
            ));
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ProviderError::Other(
                "Fireworks rate limit exceeded. Usage will refresh on the next cycle.".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "Fireworks billing API returned HTTP {status}."
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Parse(format!("Could not read Fireworks usage: {e}")))?;
        let summary = parse_summary_for_testing(&body)?;

        let mut result = ProviderFetchResult::new(summary.to_usage_snapshot(), "api");
        if let Some(cost) = summary.to_cost_snapshot() {
            result = result.with_cost(cost);
        }
        Ok(result)
    }
}

impl Default for FireworksProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_summary_for_testing(body: &str) -> Result<FireworksSummary, ProviderError> {
    let response: BillingSummaryResponse = serde_json::from_str(body)
        .map_err(|e| ProviderError::Parse(format!("Could not parse Fireworks usage: {e}")))?;
    Ok(FireworksSummary::from_response(&response))
}

#[async_trait]
impl Provider for FireworksProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Fireworks
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::OAuth => self.fetch_usage_api(ctx).await,
            SourceMode::Web | SourceMode::Cli => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::OAuth]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_rated_line_items_in_first_currency() {
        let summary = parse_summary_for_testing(
            r#"{
              "lineItems": [
                {"category": "inference", "totalCost": {"currencyCode": "USD", "units": "12", "nanos": 500000000}},
                {"category": "fine-tuning", "totalCost": {"currencyCode": "USD", "units": "3", "nanos": 250000000}},
                {"category": "training", "totalCost": {"currencyCode": "EUR", "units": "1", "nanos": 0}},
                {"category": "unrated"}
              ],
              "usageBuckets": []
            }"#,
        )
        .unwrap();

        assert!((summary.last_30_days_spend.unwrap() - 15.75).abs() < 1e-9);
        assert_eq!(summary.currency_code.as_deref(), Some("USD"));

        let cost = summary.to_cost_snapshot().unwrap();
        assert!((cost.used - 15.75).abs() < 1e-9);
        assert_eq!(cost.currency_code, "USD");
        assert_eq!(cost.period, "Last 30 days");

        let usage = summary.to_usage_snapshot();
        assert_eq!(usage.primary.used_percent, 0.0);
        assert_eq!(usage.primary.reset_description.as_deref(), Some("$15.75"));
    }

    #[test]
    fn unrated_summary_yields_no_spend() {
        let summary = parse_summary_for_testing(
            r#"{"lineItems": [{"category": "pending"}], "usageBuckets": []}"#,
        )
        .unwrap();

        assert!(summary.last_30_days_spend.is_none());
        assert!(summary.currency_code.is_none());
        assert!(summary.to_cost_snapshot().is_none());
    }

    #[test]
    fn slug_validation_rejects_path_and_query_injection() {
        let ctx = |slug: &str| FetchContext {
            source_mode: SourceMode::OAuth,
            workspace_id: Some(slug.to_string()),
            ..FetchContext::default()
        };

        let err = FireworksProvider::resolve_account_slug(&ctx(" ")).unwrap_err();
        assert!(err.to_string().contains("account slug"), "{err}");

        assert!(FireworksProvider::resolve_account_slug(&ctx("acme_corp.1-2")).is_ok());
        assert!(FireworksProvider::resolve_account_slug(&ctx("../etc")).is_err());
        assert!(FireworksProvider::resolve_account_slug(&ctx("a?x=1")).is_err());

        let url = FireworksProvider::summary_url(
            "acme",
            DateTime::parse_from_rfc3339("2026-08-17T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert!(
            url.starts_with("https://api.fireworks.ai/v1/accounts/acme/billing/summary?startTime=")
        );
        assert!(url.contains("&endTime=2026-08-17T00:00:00"));
    }

    #[test]
    fn metadata_matches_upstream_descriptor() {
        let provider = FireworksProvider::new();
        assert_eq!(provider.id(), ProviderId::Fireworks);
        assert_eq!(provider.metadata().display_name, "Fireworks");
        assert_eq!(
            provider.metadata().dashboard_url,
            Some("https://app.fireworks.ai")
        );
        assert_eq!(provider.metadata().status_page_url, None);
        assert!(!provider.metadata().supports_credits);
        assert!(!provider.metadata().default_enabled);
    }
}
