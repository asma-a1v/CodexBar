//! JSONL Scanner with Caching
//!
//! Incremental log file parsing for Codex and Claude session logs.
//! Supports file-level caching to avoid re-parsing unchanged files.

#![allow(dead_code)]

use crate::core::{CostUsagePricing, ProviderId};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Cache for scanned file data
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostUsageCache {
    /// Last scan timestamp in milliseconds
    pub last_scan_unix_ms: i64,
    /// Per-file usage data
    pub files: HashMap<String, CostUsageFileUsage>,
    /// Aggregated daily data: day_key -> model -> [input, cached, output]
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
}

/// Per-file usage tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostUsageFileUsage {
    /// File modification time in milliseconds
    pub mtime_unix_ms: i64,
    /// File size in bytes
    pub size: i64,
    /// Daily usage data extracted from this file
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
    /// Bytes parsed so far (for incremental parsing)
    pub parsed_bytes: Option<i64>,
    /// Last model seen (for delta calculations)
    pub last_model: Option<String>,
    /// Last token totals (for delta calculations)
    pub last_totals: Option<CodexTotals>,
}

/// Running totals for Codex token counting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTotals {
    pub input: i32,
    pub cached: i32,
    pub output: i32,
}

/// Result of parsing a Codex file
#[derive(Debug)]
pub struct CodexParseResult {
    /// Daily usage: day_key -> model -> [input, cached, output]
    pub days: HashMap<String, HashMap<String, Vec<i32>>>,
    /// Bytes parsed
    pub parsed_bytes: i64,
    /// Last model seen
    pub last_model: Option<String>,
    /// Last totals seen
    pub last_totals: Option<CodexTotals>,
}

/// Day range for scanning
pub struct CostUsageDayRange {
    pub since_key: String,
    pub until_key: String,
    pub scan_since_key: String,
    pub scan_until_key: String,
}

impl CostUsageDayRange {
    pub fn new(since: NaiveDate, until: NaiveDate) -> Self {
        let since_minus_one = since - chrono::Duration::days(1);
        let until_plus_one = until + chrono::Duration::days(1);

        Self {
            since_key: Self::day_key(since),
            until_key: Self::day_key(until),
            scan_since_key: Self::day_key(since_minus_one),
            scan_until_key: Self::day_key(until_plus_one),
        }
    }

    pub fn day_key(date: NaiveDate) -> String {
        date.format("%Y-%m-%d").to_string()
    }

    pub fn is_in_range(day_key: &str, since: &str, until: &str) -> bool {
        day_key >= since && day_key <= until
    }

    pub fn parse_day_key(key: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(key, "%Y-%m-%d").ok()
    }
}

/// JSONL Scanner for cost/usage logs
pub struct JsonlScanner;

struct CodexParserState {
    current_model: Option<String>,
    previous_totals: Option<CodexTotals>,
    days: HashMap<String, HashMap<String, Vec<i32>>>,
}

impl CodexParserState {
    fn new(initial_model: Option<String>, initial_totals: Option<CodexTotals>) -> Self {
        Self {
            current_model: initial_model,
            previous_totals: initial_totals,
            days: HashMap::new(),
        }
    }

    fn process_line(&mut self, line: &str, range: &CostUsageDayRange) {
        if !is_candidate_codex_line(line) {
            return;
        }

        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(msg_type) = obj.get("type").and_then(|v| v.as_str()) else {
            return;
        };
        let Some(day_key) = codex_line_day_key(&obj, range) else {
            return;
        };

        match msg_type {
            "turn_context" => self.update_current_model(&obj),
            "event_msg" => self.record_token_count(&obj, day_key),
            _ => {}
        }
    }

    fn update_current_model(&mut self, obj: &Value) {
        let Some(payload) = obj.get("payload") else {
            return;
        };

        if let Some(model) = payload
            .get("model")
            .or_else(|| payload.get("info").and_then(|info| info.get("model")))
            .and_then(|v| v.as_str())
        {
            self.current_model = Some(model.to_string());
        }
    }

    fn record_token_count(&mut self, obj: &Value, day_key: String) {
        let Some(payload) = token_count_payload(obj) else {
            return;
        };
        let info = payload.get("info");
        let Some((delta_input, delta_cached, delta_output)) = self.token_deltas(info) else {
            return;
        };
        if delta_input == 0 && delta_cached == 0 && delta_output == 0 {
            return;
        }

        let model = self.token_model(info, payload, obj);
        let norm_model = CostUsagePricing::normalize_codex_model(&model);
        let packed = self
            .days
            .entry(day_key)
            .or_default()
            .entry(norm_model)
            .or_insert_with(|| vec![0, 0, 0]);

        packed[0] += delta_input;
        packed[1] += delta_cached.min(delta_input);
        packed[2] += delta_output;
    }

    fn token_model(&self, info: Option<&Value>, payload: &Value, obj: &Value) -> String {
        info.and_then(|i| i.get("model").or(i.get("model_name")))
            .or_else(|| payload.get("model"))
            .or_else(|| obj.get("model"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| self.current_model.clone())
            .unwrap_or_else(|| "gpt-5".to_string())
    }

    fn token_deltas(&mut self, info: Option<&Value>) -> Option<(i32, i32, i32)> {
        if let Some(total) = info.and_then(|i| i.get("total_token_usage")) {
            return Some(self.total_usage_delta(total));
        }

        info.and_then(|i| i.get("last_token_usage"))
            .map(last_usage_delta)
    }

    fn total_usage_delta(&mut self, total: &Value) -> (i32, i32, i32) {
        let totals = read_token_totals(total);
        let previous = self.previous_totals.as_ref();
        let delta_input = (totals.input - previous.map_or(0, |t| t.input)).max(0);
        let delta_cached = (totals.cached - previous.map_or(0, |t| t.cached)).max(0);
        let delta_output = (totals.output - previous.map_or(0, |t| t.output)).max(0);

        self.previous_totals = Some(totals);
        (delta_input, delta_cached, delta_output)
    }
}

fn is_candidate_codex_line(line: &str) -> bool {
    if !line.contains("\"type\":\"event_msg\"") && !line.contains("\"type\":\"turn_context\"") {
        return false;
    }

    !line.contains("\"type\":\"event_msg\"") || line.contains("\"token_count\"")
}

fn codex_line_day_key(obj: &Value, range: &CostUsageDayRange) -> Option<String> {
    let ts = obj.get("timestamp").and_then(|v| v.as_str())?;
    let day_key = ts.get(..10)?;

    CostUsageDayRange::is_in_range(day_key, &range.scan_since_key, &range.scan_until_key)
        .then(|| day_key.to_string())
}

fn token_count_payload(obj: &Value) -> Option<&Value> {
    let payload = obj.get("payload")?;
    (payload.get("type").and_then(|v| v.as_str()) == Some("token_count")).then_some(payload)
}

fn read_token_totals(value: &Value) -> CodexTotals {
    CodexTotals {
        input: token_i32(value, "input_tokens"),
        cached: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32,
        output: token_i32(value, "output_tokens"),
    }
}

fn token_i32(value: &Value, key: &str) -> i32 {
    value.get(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32
}

fn last_usage_delta(last: &Value) -> (i32, i32, i32) {
    let totals = read_token_totals(last);
    (
        totals.input.max(0),
        totals.cached.max(0),
        totals.output.max(0),
    )
}

impl JsonlScanner {
    /// Get default Codex sessions root directory
    pub fn default_codex_sessions_root() -> Option<PathBuf> {
        // Check CODEX_HOME environment variable
        if let Ok(home) = std::env::var("CODEX_HOME") {
            let home = home.trim();
            if !home.is_empty() {
                return Some(PathBuf::from(home).join("sessions"));
            }
        }

        // Default to ~/.codex/sessions
        dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
    }

    /// Get default Claude projects roots
    pub fn default_claude_projects_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();

        // Check CLAUDE_CONFIG_DIR
        if let Ok(config_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            let path = PathBuf::from(config_dir.trim()).join("projects");
            if path.exists() {
                roots.push(path);
            }
        }

        // Default locations
        if let Some(home) = dirs::home_dir() {
            let default_path = home.join(".claude").join("projects");
            if default_path.exists() && !roots.contains(&default_path) {
                roots.push(default_path);
            }
        }

        roots
    }

    /// List Codex session files in the given date range
    pub fn list_codex_session_files(
        root: &Path,
        scan_since_key: &str,
        scan_until_key: &str,
    ) -> Vec<PathBuf> {
        let mut files = Vec::new();

        let Some(mut date) = CostUsageDayRange::parse_day_key(scan_since_key) else {
            return files;
        };
        let Some(until_date) = CostUsageDayRange::parse_day_key(scan_until_key) else {
            return files;
        };

        while date <= until_date {
            let year = format!("{:04}", date.year());
            let month = format!("{:02}", date.month());
            let day = format!("{:02}", date.day());

            let day_dir = root.join(&year).join(&month).join(&day);

            if let Ok(entries) = fs::read_dir(&day_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
                    {
                        files.push(path);
                    }
                }
            }

            date += chrono::Duration::days(1);
        }

        files
    }

    /// Parse a Codex JSONL file
    pub fn parse_codex_file(
        file_path: &Path,
        range: &CostUsageDayRange,
        start_offset: i64,
        initial_model: Option<String>,
        initial_totals: Option<CodexTotals>,
    ) -> std::io::Result<CodexParseResult> {
        let file = File::open(file_path)?;
        let file_size = file.metadata()?.len() as i64;

        let mut reader = BufReader::new(file);
        if start_offset > 0 {
            reader.seek(SeekFrom::Start(start_offset as u64))?;
        }

        let mut parser = CodexParserState::new(initial_model, initial_totals);
        let mut parsed_bytes = start_offset;

        let mut line = String::new();
        while reader.read_line(&mut line)? > 0 {
            parsed_bytes += line.len() as i64;
            parser.process_line(&line, range);

            line.clear();
        }

        Ok(CodexParseResult {
            days: parser.days,
            parsed_bytes: file_size.max(parsed_bytes),
            last_model: parser.current_model,
            last_totals: parser.previous_totals,
        })
    }

    /// Load cache from disk
    pub fn load_cache(provider: ProviderId, cache_root: Option<&Path>) -> CostUsageCache {
        let cache_path = Self::cache_path(provider, cache_root);

        if let Ok(contents) = fs::read_to_string(&cache_path)
            && let Ok(cache) = serde_json::from_str(&contents)
        {
            return cache;
        }

        CostUsageCache::default()
    }

    /// Save cache to disk
    pub fn save_cache(provider: ProviderId, cache: &CostUsageCache, cache_root: Option<&Path>) {
        let cache_path = Self::cache_path(provider, cache_root);

        if let Some(parent) = cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(json) = serde_json::to_string_pretty(cache) {
            let _ = fs::write(&cache_path, json);
        }
    }

    fn cache_path(provider: ProviderId, cache_root: Option<&Path>) -> PathBuf {
        let root = cache_root
            .map(|p| p.to_path_buf())
            .or_else(|| dirs::cache_dir().map(|d| d.join("CodexBar")))
            .unwrap_or_else(|| PathBuf::from("."));

        root.join(format!("{}_cost_cache.json", provider.cli_name()))
    }
}

use chrono::Datelike;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_day_range() {
        let since = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let until = NaiveDate::from_ymd_opt(2026, 1, 20).unwrap();
        let range = CostUsageDayRange::new(since, until);

        assert_eq!(range.since_key, "2026-01-15");
        assert_eq!(range.until_key, "2026-01-20");
        assert_eq!(range.scan_since_key, "2026-01-14");
        assert_eq!(range.scan_until_key, "2026-01-21");
    }

    #[test]
    fn test_is_in_range() {
        assert!(CostUsageDayRange::is_in_range(
            "2026-01-15",
            "2026-01-10",
            "2026-01-20"
        ));
        assert!(!CostUsageDayRange::is_in_range(
            "2026-01-05",
            "2026-01-10",
            "2026-01-20"
        ));
        assert!(!CostUsageDayRange::is_in_range(
            "2026-01-25",
            "2026-01-10",
            "2026-01-20"
        ));
    }

    #[test]
    fn test_parse_day_key() {
        let date = CostUsageDayRange::parse_day_key("2026-01-15");
        assert!(date.is_some());
        let date = date.unwrap();
        assert_eq!(date.year(), 2026);
        assert_eq!(date.month(), 1);
        assert_eq!(date.day(), 15);
    }
}
