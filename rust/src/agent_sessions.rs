use crate::host::{CommandOptions, CommandRunner};
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, Utc};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::future::Future;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionProvider {
    Codex,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentSessionSource {
    Cli,
    DesktopApp,
    Ide,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionState {
    Active,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionWorkspace {
    pub cwd: Option<String>,
    pub project_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionActivity {
    pub started_at: Option<DateTime<Utc>>,
    pub last_activity_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AgentSessionFocusTarget {
    Process { pid: u32 },
    Transcript { transcript_path: String },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub provider: AgentSessionProvider,
    pub source: AgentSessionSource,
    pub state: AgentSessionState,
    pub pid: Option<u32>,
    pub transcript_path: Option<String>,
    pub host: String,
    pub workspace: AgentSessionWorkspace,
    pub activity: AgentSessionActivity,
    pub focus_target: AgentSessionFocusTarget,
}

impl AgentSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        provider: AgentSessionProvider,
        source: AgentSessionSource,
        state: AgentSessionState,
        pid: Option<u32>,
        transcript_path: Option<String>,
        host: impl Into<String>,
        workspace: AgentSessionWorkspace,
        activity: AgentSessionActivity,
        focus_target: AgentSessionFocusTarget,
    ) -> Self {
        Self {
            id: id.into(),
            provider,
            source,
            state,
            pid,
            transcript_path,
            host: host.into(),
            workspace,
            activity,
            focus_target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionHostResult {
    pub host: String,
    pub sessions: Vec<AgentSession>,
    pub error: Option<String>,
}

impl AgentSessionHostResult {
    pub fn success(host: impl Into<String>, sessions: Vec<AgentSession>) -> Self {
        Self {
            host: host.into(),
            sessions,
            error: None,
        }
    }

    pub fn failed(host: impl Into<String>, message: impl std::fmt::Display) -> Self {
        Self {
            host: host.into(),
            sessions: Vec::new(),
            error: Some(crate::logging::safe_error_message(message)),
        }
    }

    pub fn from_json(body: &str) -> Result<Self, String> {
        RemoteSessionFetcher::decode_host_result(body)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionScanConfig {
    pub active_window: Duration,
    pub file_only_window: Duration,
}

impl Default for SessionScanConfig {
    fn default() -> Self {
        Self {
            active_window: Duration::from_secs(120),
            file_only_window: Duration::from_secs(30 * 60),
        }
    }
}

impl SessionScanConfig {
    pub fn state(
        &self,
        last_activity_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        has_live_process: bool,
    ) -> AgentSessionState {
        match last_activity_at {
            Some(last_activity_at) => {
                let age = now.signed_duration_since(last_activity_at);
                let active_window = ChronoDuration::from_std(self.active_window)
                    .unwrap_or_else(|_| ChronoDuration::seconds(120));
                if age <= active_window {
                    AgentSessionState::Active
                } else {
                    AgentSessionState::Idle
                }
            }
            None if has_live_process => AgentSessionState::Active,
            None => AgentSessionState::Idle,
        }
    }

    pub fn file_only_session_allowed(
        &self,
        modified_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> bool {
        let age = now.signed_duration_since(modified_at);
        let file_window = ChronoDuration::from_std(self.file_only_window)
            .unwrap_or_else(|_| ChronoDuration::seconds(30 * 60));
        age <= file_window
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentProcessKind {
    Agent,
    Helper,
    AppServer,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProcessRecord {
    pub pid: u32,
    pub ppid: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub provider: Option<AgentSessionProvider>,
    pub source: AgentSessionSource,
    pub executable: String,
    pub kind: AgentProcessKind,
}

impl AgentProcessRecord {
    pub fn is_agent(&self) -> bool {
        self.kind == AgentProcessKind::Agent
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeTranscript {
    pub url: PathBuf,
    pub modified_at: DateTime<Utc>,
}

impl ClaudeTranscript {
    pub fn new(url: PathBuf, modified_at: DateTime<Utc>) -> Self {
        Self { url, modified_at }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexRolloutMetadata {
    pub session_id: String,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub source: Option<String>,
}

impl CodexRolloutMetadata {
    pub fn session_source(&self) -> AgentSessionSource {
        let value = [self.originator.as_deref(), self.source.as_deref()]
            .into_iter()
            .flatten()
            .map(|part| part.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");

        if value.contains("desktop") || value.contains("app-server") {
            AgentSessionSource::DesktopApp
        } else if value.contains("ide")
            || value.contains("vscode")
            || value.contains("cursor")
            || value.contains("zed")
        {
            AgentSessionSource::Ide
        } else if value.contains("codex_exec") || value.contains("exec") || value.contains("cli") {
            AgentSessionSource::Cli
        } else {
            AgentSessionSource::Unknown
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SessionFocusResult {
    Focused,
    Unsupported { message: String },
    Failed { message: String },
}

impl SessionFocusResult {
    pub fn focused() -> Self {
        Self::Focused
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported {
            message: crate::logging::safe_error_message(message.into()),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: crate::logging::safe_error_message(message.into()),
        }
    }
}

pub struct AgentPSOutputParser;
pub struct WindowsProcessOutputParser;
pub struct LSOFCWDOutputParser;
pub struct ClaudeSessionProjectMapper;
pub struct ClaudeTranscriptMetadataParser;
pub struct CodexRolloutFirstLineParser;
pub struct AgentSessionCorrelation;
#[derive(Debug, Clone)]
pub struct RemoteSessionFetcher {
    pub per_host_timeout: Duration,
}
pub struct TailscaleStatusParser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeTranscriptMetadata {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSessionDiscoveryMode {
    Disabled,
    Enabled { ssh_hosts: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSessionDiscoveryResult {
    Disabled,
    Hosts(Vec<AgentSessionHostResult>),
}

#[derive(Debug, Clone)]
pub struct LocalAgentSessionScanner {
    pub config: SessionScanConfig,
    pub command_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct AgentSessionDiscovery {
    local: LocalAgentSessionScanner,
    remote: RemoteSessionFetcher,
}

impl AgentPSOutputParser {
    pub fn parse(output: &str) -> Vec<AgentProcessRecord> {
        let mut seen_pids = HashSet::new();
        output
            .lines()
            .filter_map(|line| Self::parse_line(line, &mut seen_pids))
            .collect()
    }

    pub fn agent_processes(records: &[AgentProcessRecord]) -> Vec<AgentProcessRecord> {
        let mut seen = HashSet::new();
        records
            .iter()
            .filter(|record| record.is_agent())
            .filter_map(|record| {
                if seen.insert(record.pid) {
                    Some(record.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn provider(record: &AgentProcessRecord) -> Option<AgentSessionProvider> {
        record.provider
    }

    pub fn source(record: &AgentProcessRecord) -> AgentSessionSource {
        record.source
    }

    pub fn has_codex_app_server(records: &[AgentProcessRecord]) -> bool {
        records.iter().any(|record| {
            record.kind == AgentProcessKind::AppServer
                && record.provider == Some(AgentSessionProvider::Codex)
        })
    }

    fn parse_line(line: &str, seen_pids: &mut HashSet<u32>) -> Option<AgentProcessRecord> {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse::<u32>().ok()?;
        let ppid = fields.next()?.parse::<u32>().ok()?;
        let weekday = fields.next()?;
        let month = fields.next()?;
        let day = fields.next()?;
        let time = fields.next()?;
        let year = fields.next()?;
        if !seen_pids.insert(pid) {
            return None;
        }

        let started_at = Self::parse_started_at(weekday, month, day, time, year)?;
        let command = fields.collect::<Vec<_>>().join(" ");
        let classification = classify_process_command(&command);
        Some(AgentProcessRecord {
            pid,
            ppid,
            started_at: Some(started_at),
            provider: classification.provider,
            source: classification.source,
            executable: classification.executable,
            kind: classification.kind,
        })
    }

    fn parse_started_at(
        weekday: &str,
        month: &str,
        day: &str,
        time: &str,
        year: &str,
    ) -> Option<DateTime<Utc>> {
        let text = format!("{weekday} {month} {day} {time} {year}");
        chrono::NaiveDateTime::parse_from_str(&text, "%a %b %e %H:%M:%S %Y")
            .ok()
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsProcessMetadata {
    process_id: u32,
    #[serde(default)]
    parent_process_id: u32,
    creation_date: Option<String>,
    name: Option<String>,
    executable_path: Option<String>,
}

impl WindowsProcessOutputParser {
    pub fn parse(output: &str) -> Vec<AgentProcessRecord> {
        let Ok(value) = serde_json::from_str::<Value>(output.trim()) else {
            return Vec::new();
        };
        let values = match value {
            Value::Array(values) => values,
            Value::Object(_) => vec![value],
            _ => return Vec::new(),
        };
        let mut seen = HashSet::new();

        values
            .into_iter()
            .filter_map(|value| serde_json::from_value::<WindowsProcessMetadata>(value).ok())
            .filter(|process| process.process_id > 0 && seen.insert(process.process_id))
            .map(|process| {
                let display_name = process
                    .name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .or(process.executable_path.as_deref())
                    .unwrap_or_default();
                let classification = classify_process_command(display_name);
                AgentProcessRecord {
                    pid: process.process_id,
                    ppid: process.parent_process_id,
                    started_at: process
                        .creation_date
                        .as_deref()
                        .and_then(parse_windows_creation_date),
                    provider: classification.provider,
                    source: classification.source,
                    executable: process.name.unwrap_or(classification.executable),
                    kind: classification.kind,
                }
            })
            .collect()
    }
}

fn parse_windows_creation_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
        .or_else(|| {
            let value = value.strip_prefix("/Date(")?;
            let milliseconds = value
                .trim_end_matches(")/")
                .split(['+', '-'])
                .next()?
                .parse()
                .ok()?;
            DateTime::<Utc>::from_timestamp_millis(milliseconds)
        })
        .or_else(|| {
            let core = value.get(..21)?;
            chrono::NaiveDateTime::parse_from_str(core, "%Y%m%d%H%M%S%.f")
                .ok()
                .map(|date| DateTime::<Utc>::from_naive_utc_and_offset(date, Utc))
        })
}

struct ProcessClassification {
    provider: Option<AgentSessionProvider>,
    source: AgentSessionSource,
    executable: String,
    kind: AgentProcessKind,
}

fn classify_process_command(command: &str) -> ProcessClassification {
    let lower = command.to_ascii_lowercase();
    let executable = executable_basename(command);

    if lower.contains("app-server") && lower.contains("codex") {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Codex),
            source: AgentSessionSource::DesktopApp,
            executable,
            kind: AgentProcessKind::AppServer,
        };
    }

    if lower.contains("codex (renderer)")
        || lower.contains("claude-code-acp")
        || lower.contains("--help")
        || lower.contains("--version")
        || lower.contains("--type=renderer")
        || lower.contains("disclaimer")
        || executable.eq_ignore_ascii_case("disclaimer")
    {
        return ProcessClassification {
            provider: None,
            source: AgentSessionSource::Unknown,
            executable,
            kind: AgentProcessKind::Helper,
        };
    }

    if lower.contains("application support/claude/claude-code/claude")
        || lower.contains("claude.app")
        || lower.contains("claude.exe")
        || executable.eq_ignore_ascii_case("claude")
    {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Claude),
            source: if lower.contains("application support/claude/claude-code")
                || lower.contains("claude.app")
            {
                AgentSessionSource::DesktopApp
            } else {
                AgentSessionSource::Cli
            },
            executable: if executable.eq_ignore_ascii_case("claude") {
                "claude".to_string()
            } else {
                executable
            },
            kind: AgentProcessKind::Agent,
        };
    }

    if lower.contains("codex.exe")
        || lower.contains("codex.app")
        || lower.contains("codex desktop")
        || executable.eq_ignore_ascii_case("codex")
    {
        return ProcessClassification {
            provider: Some(AgentSessionProvider::Codex),
            source: if lower.contains("codex.app") || lower.contains("codex desktop") {
                AgentSessionSource::DesktopApp
            } else {
                AgentSessionSource::Cli
            },
            executable: if executable.eq_ignore_ascii_case("codex") {
                "codex".to_string()
            } else {
                executable
            },
            kind: AgentProcessKind::Agent,
        };
    }

    ProcessClassification {
        provider: None,
        source: AgentSessionSource::Unknown,
        executable,
        kind: AgentProcessKind::Other,
    }
}

fn executable_basename(command: &str) -> String {
    let normalized = command.replace('\\', "/").to_ascii_lowercase();
    for (needle, name) in [
        ("claude-code-acp", "claude-code-acp"),
        ("application support/claude/claude-code/claude", "claude"),
        ("codex app-server", "codex"),
        ("codex (renderer)", "codex"),
        ("codex.app", "codex"),
        ("claude.app", "claude"),
        ("claude.exe", "claude"),
        ("codex.exe", "codex"),
        ("disclaimer", "disclaimer"),
    ] {
        if normalized.contains(needle) {
            return name.to_string();
        }
    }

    let first_token = command.split_whitespace().next().unwrap_or_default();
    if first_token.is_empty() {
        return String::new();
    }

    Path::new(first_token)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(first_token)
        .to_string()
}

impl LSOFCWDOutputParser {
    pub fn parse(output: &str) -> HashMap<u32, String> {
        let mut result = HashMap::new();
        let mut current_pid = None;

        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match line.chars().next() {
                Some('p') => {
                    current_pid = line[1..].trim().parse::<u32>().ok();
                }
                Some('n') => {
                    if let Some(pid) = current_pid {
                        result.insert(pid, line[1..].to_string());
                    }
                }
                _ => {}
            }
        }

        result
    }
}

impl ClaudeSessionProjectMapper {
    pub fn escaped_cwd(cwd: &str) -> String {
        cwd.chars()
            .map(|scalar| {
                if scalar.is_ascii_alphanumeric() {
                    scalar
                } else {
                    '-'
                }
            })
            .collect()
    }

    pub fn project_directories(cwd: &str, home_directory: &Path) -> Vec<PathBuf> {
        if cwd.trim().is_empty() {
            return Vec::new();
        }

        vec![
            home_directory
                .join(".claude")
                .join("projects")
                .join(Self::escaped_cwd(cwd)),
        ]
    }

    pub fn transcripts(cwd: &str, home_directory: &Path) -> Vec<ClaudeTranscript> {
        let mut transcripts = Vec::new();

        for directory in Self::project_directories(cwd, home_directory) {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }

                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let Ok(modified) = metadata.modified() else {
                    continue;
                };

                transcripts.push(ClaudeTranscript::new(path, modified.into()));
            }
        }

        transcripts.sort_by(|lhs, rhs| {
            rhs.modified_at
                .cmp(&lhs.modified_at)
                .then_with(|| rhs.url.cmp(&lhs.url))
        });
        transcripts
    }

    pub fn newest_transcript(cwd: &str, home_directory: &Path) -> Option<ClaudeTranscript> {
        Self::transcripts(cwd, home_directory).into_iter().next()
    }
}

impl ClaudeTranscriptMetadataParser {
    const MAX_LINES: usize = 32;
    const MAX_BYTES: u64 = 64 * 1024;

    pub fn parse(reader: impl Read) -> Option<ClaudeTranscriptMetadata> {
        let mut session_id = None;
        let mut cwd = None;
        let reader = BufReader::new(reader.take(Self::MAX_BYTES));

        for line in reader.lines().take(Self::MAX_LINES).map_while(Result::ok) {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if session_id.is_none() {
                session_id = value
                    .get("sessionId")
                    .or_else(|| value.get("session_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            if cwd.is_none() {
                cwd = value.get("cwd").and_then(Value::as_str).map(str::to_owned);
            }
            if session_id.is_some() && cwd.is_some() {
                break;
            }
        }

        (session_id.is_some() || cwd.is_some())
            .then_some(ClaudeTranscriptMetadata { session_id, cwd })
    }
}

impl CodexRolloutFirstLineParser {
    pub fn parse(line: &str) -> Option<CodexRolloutMetadata> {
        let value: Value = serde_json::from_str(line).ok()?;
        if value.get("type")?.as_str()? != "session_meta" {
            return None;
        }

        let payload = value.get("payload")?.as_object()?;
        let session_id = payload
            .get("session_id")
            .or_else(|| payload.get("id"))?
            .as_str()?;
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return None;
        }

        Some(CodexRolloutMetadata {
            session_id: session_id.to_string(),
            cwd: payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            originator: payload
                .get("originator")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            source: payload
                .get("source")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
        })
    }

    pub fn read_first_line(path: &Path) -> Option<String> {
        let file = File::open(path).ok()?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).ok()?;
        if bytes == 0 {
            return None;
        }
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        Some(line)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn make_session(
        metadata: CodexRolloutMetadata,
        transcript_path: &Path,
        modified_at: DateTime<Utc>,
        pid: Option<u32>,
        started_at: Option<DateTime<Utc>>,
        host: &str,
        config: SessionScanConfig,
        now: DateTime<Utc>,
    ) -> Option<AgentSession> {
        if pid.is_none() && !config.file_only_session_allowed(modified_at, now) {
            return None;
        }

        let session_source = metadata.session_source();
        let workspace = AgentSessionWorkspace {
            cwd: metadata.cwd.clone(),
            project_name: metadata.cwd.as_deref().and_then(project_name_from_cwd),
        };
        let transcript_path = transcript_path.to_string_lossy().to_string();
        let focus_target = match pid {
            Some(pid) => AgentSessionFocusTarget::Process { pid },
            None => AgentSessionFocusTarget::Transcript {
                transcript_path: transcript_path.clone(),
            },
        };

        Some(AgentSession::new(
            metadata.session_id,
            AgentSessionProvider::Codex,
            session_source,
            config.state(Some(modified_at), now, pid.is_some()),
            pid,
            Some(transcript_path),
            host,
            workspace,
            AgentSessionActivity {
                started_at,
                last_activity_at: Some(modified_at),
            },
            focus_target,
        ))
    }
}

impl AgentSessionCorrelation {
    pub fn project_name(cwd: Option<&str>) -> Option<String> {
        cwd.and_then(project_name_from_cwd)
    }
}

pub fn focus_session(session: &AgentSession) -> SessionFocusResult {
    match session.focus_target {
        AgentSessionFocusTarget::Transcript { .. } => {
            SessionFocusResult::unsupported("This file-only session has no focusable Windows window.")
        }
        AgentSessionFocusTarget::None => {
            SessionFocusResult::unsupported("This session has no focus target on Windows.")
        }
        AgentSessionFocusTarget::Process { pid } => {
            if !is_local_host(&session.host) {
                return SessionFocusResult::unsupported(
                    "Remote session focus is not supported from this Windows desktop.",
                );
            }

            focus_process(pid)
        }
    }
}

fn is_local_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host.is_empty()
    {
        return true;
    }

    std::env::var("COMPUTERNAME")
        .map(|name| name.eq_ignore_ascii_case(host))
        .unwrap_or(false)
}

#[cfg(windows)]
fn focus_process(pid: u32) -> SessionFocusResult {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SW_RESTORE, SetForegroundWindow,
        ShowWindow,
    };

    struct Search {
        pid: u32,
        window: Option<HWND>,
    }

    unsafe extern "system" fn find_window(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = &mut *(data.0 as *mut Search);
        let mut window_pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut window_pid));
        if window_pid == search.pid && IsWindowVisible(hwnd).as_bool() {
            search.window = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut search = Search { pid, window: None };
    let result = unsafe { EnumWindows(Some(find_window), LPARAM(&mut search as *mut _ as isize)) };
    if result.is_err() {
        return SessionFocusResult::failed("Windows could not enumerate application windows.");
    }

    let Some(window) = search.window else {
        return SessionFocusResult::failed("No focusable window was found for this session.");
    };
    unsafe {
        let _ = ShowWindow(window, SW_RESTORE);
        if SetForegroundWindow(window).as_bool() {
            SessionFocusResult::focused()
        } else {
            SessionFocusResult::failed("Windows denied the request to focus this session.")
        }
    }
}

#[cfg(not(windows))]
fn focus_process(_pid: u32) -> SessionFocusResult {
    SessionFocusResult::unsupported("Process focus requires the Windows desktop shell.")
}

struct CodexRollout {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    metadata: CodexRolloutMetadata,
}

struct ClaudeTranscriptCandidate {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    metadata: ClaudeTranscriptMetadata,
}

impl Default for LocalAgentSessionScanner {
    fn default() -> Self {
        Self {
            config: SessionScanConfig::default(),
            command_timeout: Duration::from_secs(5),
        }
    }
}

impl LocalAgentSessionScanner {
    pub fn new(config: SessionScanConfig, command_timeout: Duration) -> Self {
        Self {
            config,
            command_timeout,
        }
    }

    pub async fn scan(&self) -> AgentSessionHostResult {
        let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".to_string());
        let options = Self::process_options(self.command_timeout);
        let process_result = CommandRunner::new()
            .run_async("powershell.exe", None, &options)
            .await;
        let (processes, error) = match process_result {
            Ok(result) if result.timed_out => (
                Vec::new(),
                Some("Windows process discovery timed out; file-only sessions may still appear."),
            ),
            Ok(result) if result.exit_code == Some(0) => {
                (WindowsProcessOutputParser::parse(&result.text), None)
            }
            Ok(_) => (
                Vec::new(),
                Some(
                    "Windows process discovery failed; verify PowerShell and CIM access. File-only sessions may still appear.",
                ),
            ),
            Err(_) => (
                Vec::new(),
                Some(
                    "Unable to launch PowerShell for process discovery; file-only sessions may still appear.",
                ),
            ),
        };

        let now = Utc::now();
        let sessions = self.scan_files(
            &host,
            now,
            &Self::codex_sessions_root(),
            &Self::claude_projects_roots(),
            &processes,
        );
        AgentSessionHostResult {
            host,
            sessions,
            error: error.map(crate::logging::safe_error_message),
        }
    }

    fn process_options(timeout: Duration) -> CommandOptions {
        CommandOptions {
            timeout,
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                concat!(
                    "$ErrorActionPreference='Stop';",
                    "$processes=@(Get-CimInstance Win32_Process | ",
                    "Select-Object ProcessId,ParentProcessId,CreationDate,Name,ExecutablePath);",
                    "ConvertTo-Json -Compress -InputObject $processes"
                )
                .to_string(),
            ],
            ..CommandOptions::default()
        }
    }

    fn scan_files(
        &self,
        host: &str,
        now: DateTime<Utc>,
        codex_root: &Path,
        claude_roots: &[PathBuf],
        processes: &[AgentProcessRecord],
    ) -> Vec<AgentSession> {
        let mut agents = AgentPSOutputParser::agent_processes(processes);
        agents.sort_by_key(|process| std::cmp::Reverse(process.started_at));
        let mut rollouts = VecDeque::from(Self::codex_rollouts(
            codex_root,
            now.with_timezone(&Local).date_naive(),
        ));
        let claude_count = agents
            .iter()
            .filter(|process| process.provider == Some(AgentSessionProvider::Claude))
            .count();
        let mut claude_transcripts =
            VecDeque::from(Self::claude_transcripts(claude_roots, claude_count));
        let mut sessions = Vec::new();

        for process in agents {
            match process.provider {
                Some(AgentSessionProvider::Codex) => sessions.push(self.codex_process_session(
                    host,
                    now,
                    process,
                    rollouts.pop_front(),
                )),
                Some(AgentSessionProvider::Claude) => sessions.push(self.claude_process_session(
                    host,
                    now,
                    process,
                    claude_transcripts.pop_front(),
                )),
                None => {}
            }
        }

        sessions.extend(rollouts.into_iter().filter_map(|rollout| {
            CodexRolloutFirstLineParser::make_session(
                rollout.metadata,
                &rollout.path,
                rollout.modified_at,
                None,
                None,
                host,
                self.config,
                now,
            )
        }));
        sessions.sort_by(|lhs, rhs| {
            (rhs.state == AgentSessionState::Active)
                .cmp(&(lhs.state == AgentSessionState::Active))
                .then_with(|| {
                    rhs.activity
                        .last_activity_at
                        .or(rhs.activity.started_at)
                        .cmp(&lhs.activity.last_activity_at.or(lhs.activity.started_at))
                })
        });
        let mut seen = HashSet::new();
        sessions.retain(|session| seen.insert(format!("{}:{}", session.host, session.id)));
        sessions
    }

    fn codex_process_session(
        &self,
        host: &str,
        now: DateTime<Utc>,
        process: AgentProcessRecord,
        rollout: Option<CodexRollout>,
    ) -> AgentSession {
        let cwd = rollout
            .as_ref()
            .and_then(|rollout| rollout.metadata.cwd.clone());
        let source = rollout
            .as_ref()
            .map(|rollout| rollout.metadata.session_source())
            .filter(|source| *source != AgentSessionSource::Unknown)
            .unwrap_or(process.source);
        let modified_at = rollout.as_ref().map(|rollout| rollout.modified_at);
        let transcript_path = rollout
            .as_ref()
            .map(|rollout| rollout.path.to_string_lossy().to_string());
        AgentSession::new(
            rollout
                .as_ref()
                .map(|rollout| rollout.metadata.session_id.clone())
                .unwrap_or_else(|| format!("pid:{}", process.pid)),
            AgentSessionProvider::Codex,
            source,
            self.config.state(modified_at, now, true),
            Some(process.pid),
            transcript_path,
            host,
            AgentSessionWorkspace {
                project_name: cwd.as_deref().and_then(project_name_from_cwd),
                cwd,
            },
            AgentSessionActivity {
                started_at: process.started_at,
                last_activity_at: modified_at,
            },
            AgentSessionFocusTarget::Process { pid: process.pid },
        )
    }

    fn claude_process_session(
        &self,
        host: &str,
        now: DateTime<Utc>,
        process: AgentProcessRecord,
        transcript: Option<ClaudeTranscriptCandidate>,
    ) -> AgentSession {
        let cwd = transcript
            .as_ref()
            .and_then(|transcript| transcript.metadata.cwd.clone());
        let modified_at = transcript.as_ref().map(|transcript| transcript.modified_at);
        let transcript_path = transcript
            .as_ref()
            .map(|transcript| transcript.path.to_string_lossy().to_string());
        let id = transcript
            .as_ref()
            .and_then(|transcript| transcript.metadata.session_id.clone())
            .or_else(|| {
                transcript.as_ref().and_then(|transcript| {
                    transcript
                        .path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                })
            })
            .unwrap_or_else(|| format!("pid:{}", process.pid));
        AgentSession::new(
            id,
            AgentSessionProvider::Claude,
            process.source,
            self.config.state(modified_at, now, true),
            Some(process.pid),
            transcript_path,
            host,
            AgentSessionWorkspace {
                project_name: cwd.as_deref().and_then(project_name_from_cwd),
                cwd,
            },
            AgentSessionActivity {
                started_at: process.started_at,
                last_activity_at: modified_at,
            },
            AgentSessionFocusTarget::Process { pid: process.pid },
        )
    }

    fn codex_sessions_root() -> PathBuf {
        if let Ok(path) = std::env::var("CODEX_HOME")
            && !path.trim().is_empty()
        {
            let path = PathBuf::from(path.trim());
            if path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("sessions"))
            {
                return path;
            }
            return path.join("sessions");
        }
        dirs::home_dir()
            .unwrap_or_default()
            .join(".codex")
            .join("sessions")
    }

    fn claude_projects_roots() -> Vec<PathBuf> {
        if let Ok(path) = std::env::var("CLAUDE_CONFIG_DIR")
            && !path.trim().is_empty()
        {
            return vec![PathBuf::from(path.trim()).join("projects")];
        }
        dirs::home_dir()
            .map(|home| vec![home.join(".claude").join("projects")])
            .unwrap_or_default()
    }

    fn codex_day_directories(root: &Path, today: NaiveDate) -> Vec<PathBuf> {
        [today, today - ChronoDuration::days(1)]
            .into_iter()
            .map(|date| {
                root.join(date.format("%Y").to_string())
                    .join(date.format("%m").to_string())
                    .join(date.format("%d").to_string())
            })
            .collect()
    }

    fn codex_rollouts(root: &Path, today: NaiveDate) -> Vec<CodexRollout> {
        let mut rollouts = Vec::new();
        for directory in Self::codex_day_directories(root, today) {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_rollout = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-"));
                if !is_rollout || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(line) = CodexRolloutFirstLineParser::read_first_line(&path) else {
                    continue;
                };
                let Some(metadata) = CodexRolloutFirstLineParser::parse(&line) else {
                    continue;
                };
                let Some(modified_at) = entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(DateTime::<Utc>::from)
                else {
                    continue;
                };
                rollouts.push(CodexRollout {
                    path,
                    modified_at,
                    metadata,
                });
            }
        }
        rollouts.sort_by_key(|rollout| std::cmp::Reverse(rollout.modified_at));
        rollouts
    }

    fn claude_transcripts(
        roots: &[PathBuf],
        live_process_count: usize,
    ) -> Vec<ClaudeTranscriptCandidate> {
        if live_process_count == 0 {
            return Vec::new();
        }
        let mut files = Vec::new();
        for root in roots {
            let Ok(projects) = fs::read_dir(root) else {
                continue;
            };
            for project in projects.flatten() {
                let Ok(entries) = fs::read_dir(project.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Some(modified_at) = entry
                        .metadata()
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .map(DateTime::<Utc>::from)
                    else {
                        continue;
                    };
                    files.push((path, modified_at));
                }
            }
        }
        files.sort_by(|lhs, rhs| rhs.1.cmp(&lhs.1).then_with(|| rhs.0.cmp(&lhs.0)));
        files
            .into_iter()
            .take(live_process_count)
            .filter_map(|(path, modified_at)| {
                let file = File::open(&path).ok()?;
                let metadata = ClaudeTranscriptMetadataParser::parse(file)?;
                Some(ClaudeTranscriptCandidate {
                    path,
                    modified_at,
                    metadata,
                })
            })
            .collect()
    }
}

impl RemoteSessionFetcher {
    const BUNDLED_CLI_FALLBACK: &'static str =
        "/Applications/CodexBar.app/Contents/Helpers/CodexBarCLI";

    pub fn new(per_host_timeout: Duration) -> Self {
        Self { per_host_timeout }
    }

    pub async fn fetch(&self, hosts: &[String]) -> Vec<AgentSessionHostResult> {
        let valid = Self::sanitized_hosts(hosts);
        let valid_keys = valid
            .iter()
            .map(|host| host.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        let mut invalid = hosts
            .iter()
            .filter(|host| {
                Self::validate_host(host).is_err()
                    && !valid_keys.contains(&host.trim().to_ascii_lowercase())
            })
            .map(|_| {
                AgentSessionHostResult::failed(
                    "<invalid SSH host>",
                    "Invalid SSH host entry; use a host name or user@host without spaces or options.",
                )
            })
            .collect::<Vec<_>>();
        let timeout = self.per_host_timeout;
        let mut results = Self::fetch_hosts_with(&valid, |host| async move {
            Self::fetch_host(host, timeout).await
        })
        .await;
        results.append(&mut invalid);
        results.sort_by(|lhs, rhs| {
            lhs.host
                .to_ascii_lowercase()
                .cmp(&rhs.host.to_ascii_lowercase())
        });
        results
    }

    async fn fetch_host(host: String, timeout: Duration) -> AgentSessionHostResult {
        let options = match Self::ssh_options(&host, timeout) {
            Ok(options) => options,
            Err(error) => return AgentSessionHostResult::failed("<invalid SSH host>", error),
        };
        let result = CommandRunner::new().run_async("ssh", None, &options).await;
        match result {
            Ok(result) if result.timed_out => AgentSessionHostResult::failed(
                host,
                "SSH session discovery timed out; verify the host is reachable and key authentication is configured.",
            ),
            Ok(result) if result.exit_code == Some(0) => {
                Self::decode_remote_sessions(&host, &result.text).unwrap_or_else(|error| {
                    AgentSessionHostResult::failed(
                        host,
                        actionable_message(
                            "Remote session response was not valid JSON; update CodexBar on the remote host",
                            error,
                        ),
                    )
                })
            }
            Ok(result) => AgentSessionHostResult::failed(
                host,
                format!(
                    "SSH session discovery failed{}; verify BatchMode key access and the remote codexbar installation.",
                    result
                        .exit_code
                        .map(|code| format!(" with exit code {code}"))
                        .unwrap_or_default()
                ),
            ),
            Err(error) => AgentSessionHostResult::failed(
                host,
                actionable_message(
                    "Unable to start SSH; install the Windows OpenSSH client and verify PATH",
                    error,
                ),
            ),
        }
    }

    fn ssh_options(host: &str, timeout: Duration) -> Result<CommandOptions, String> {
        let host = Self::validate_host(host)?;
        let connect_timeout = timeout.as_secs().clamp(1, 3);
        let remote_command = format!(
            "codexbar sessions --json || '{}' sessions --json",
            Self::BUNDLED_CLI_FALLBACK
        );
        Ok(CommandOptions {
            timeout,
            initial_delay: Duration::ZERO,
            extra_args: vec![
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                format!("ConnectTimeout={connect_timeout}"),
                "--".to_string(),
                host,
                "sh".to_string(),
                "-lc".to_string(),
                remote_command,
            ],
            ..CommandOptions::default()
        })
    }

    async fn fetch_hosts_with<F, Fut>(hosts: &[String], fetch: F) -> Vec<AgentSessionHostResult>
    where
        F: Fn(String) -> Fut + Clone,
        Fut: Future<Output = AgentSessionHostResult>,
    {
        let mut results = join_all(hosts.iter().cloned().map(|host| fetch.clone()(host))).await;
        results.sort_by(|lhs, rhs| {
            lhs.host
                .to_ascii_lowercase()
                .cmp(&rhs.host.to_ascii_lowercase())
        });
        results
    }

    fn decode_remote_sessions(host: &str, body: &str) -> Result<AgentSessionHostResult, String> {
        if let Ok(mut sessions) = serde_json::from_str::<Vec<AgentSession>>(body) {
            for session in &mut sessions {
                session.host = host.to_string();
            }
            return Ok(AgentSessionHostResult::success(host, sessions));
        }
        let mut result = Self::decode_host_result(body)?;
        result.host = host.to_string();
        for session in &mut result.sessions {
            session.host = host.to_string();
        }
        Ok(result)
    }

    pub fn sanitized_hosts(hosts: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut sanitized = Vec::new();

        for host in hosts {
            let Ok(host) = Self::validate_host(host) else {
                continue;
            };

            let key = host.to_ascii_lowercase();
            if seen.insert(key) {
                sanitized.push(host);
            }
        }

        sanitized
    }

    pub fn validate_host(host: &str) -> Result<String, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err("host must not be empty".to_string());
        }
        if host.starts_with('-') {
            return Err("host must not start with '-'".to_string());
        }
        if host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || !is_safe_host_char(c))
        {
            return Err(
                "host must not contain whitespace, control characters, or unsafe shell characters"
                    .to_string(),
            );
        }

        Ok(host.to_string())
    }

    pub fn decode_host_result(body: &str) -> Result<AgentSessionHostResult, String> {
        let result: AgentSessionHostResult = serde_json::from_str(body)
            .map_err(|err| actionable_message("Unable to decode remote session response", err))?;
        Self::validate_host(&result.host).map_err(|err| {
            actionable_message("Remote session response has an invalid host", err)
        })?;
        Ok(result)
    }

    pub fn failed_result(host: &str, err: impl std::fmt::Display) -> AgentSessionHostResult {
        AgentSessionHostResult::failed(host.to_string(), err)
    }
}

impl Default for RemoteSessionFetcher {
    fn default() -> Self {
        Self {
            per_host_timeout: Duration::from_secs(5),
        }
    }
}

impl AgentSessionDiscovery {
    pub fn new(local: LocalAgentSessionScanner, remote: RemoteSessionFetcher) -> Self {
        Self { local, remote }
    }

    pub async fn scan(&self, mode: AgentSessionDiscoveryMode) -> AgentSessionDiscoveryResult {
        let AgentSessionDiscoveryMode::Enabled { ssh_hosts } = mode else {
            return AgentSessionDiscoveryResult::Disabled;
        };
        let (local, remote) = tokio::join!(self.local.scan(), self.remote.fetch(&ssh_hosts));
        let mut hosts = Vec::with_capacity(remote.len() + 1);
        hosts.push(local);
        hosts.extend(remote);
        AgentSessionDiscoveryResult::Hosts(hosts)
    }
}

impl Default for AgentSessionDiscovery {
    fn default() -> Self {
        Self::new(
            LocalAgentSessionScanner::default(),
            RemoteSessionFetcher::default(),
        )
    }
}

fn actionable_message(label: &str, err: impl std::fmt::Display) -> String {
    crate::logging::safe_error_message(format!("{label}: {err}"))
}

fn is_safe_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_' | '@')
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['\\', '/']);
    let path = Path::new(trimmed);
    let name = path.file_name()?.to_str()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::io;
    use std::sync::Arc;

    #[test]
    fn process_parser_filters_helpers_app_server_duplicates_and_malformed_lines() {
        let output = "\
101   1 Mon Jul  6 09:00:00 2026 /Applications/Claude.app/Contents/Resources/disclaimer /Users/test/Library/Application Support/Claude/claude-code/claude --dangerously-skip-permissions
102 101 Mon Jul  6 09:00:01 2026 /Users/test/Library/Application Support/Claude/claude-code/claude --dangerously-skip-permissions
102 101 Mon Jul  6 09:00:01 2026 /Users/test/Library/Application Support/Claude/claude-code/claude --dangerously-skip-permissions
201   1 Mon Jul  6 09:01:00 2026 /opt/homebrew/bin/codex exec --full-auto strange argv here
202   1 Mon Jul  6 09:02:00 2026 /Applications/Codex.app/Contents/Resources/codex app-server --listen stdio
203   1 Mon Jul  6 09:03:00 2026 /usr/local/bin/codex --help
301   1 Mon Jul  6 09:04:00 2026 /Users/test/.local/bin/claude-code-acp --stdio
401   1 Mon Jul  6 09:05:00 2026 /Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer) --type=renderer
bad line
";

        let records = AgentPSOutputParser::parse(output);
        let agents = AgentPSOutputParser::agent_processes(&records);

        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn session_scan_config_cuts_off_active_and_file_only_windows() {
        let config = SessionScanConfig::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap();

        assert_eq!(
            config.state(Some(now - chrono::Duration::seconds(119)), now, true),
            AgentSessionState::Active
        );
        assert_eq!(
            config.state(Some(now - chrono::Duration::seconds(121)), now, true),
            AgentSessionState::Idle
        );
        assert!(config.file_only_session_allowed(now - chrono::Duration::minutes(29), now));
        assert!(!config.file_only_session_allowed(now - chrono::Duration::minutes(31), now));
    }

    #[test]
    fn agent_session_round_trips_json() {
        let session = AgentSession {
            id: "session-1".to_string(),
            provider: AgentSessionProvider::Codex,
            source: AgentSessionSource::DesktopApp,
            state: AgentSessionState::Active,
            pid: Some(1234),
            transcript_path: Some("C:\\sessions\\rollout.jsonl".to_string()),
            host: "devbox".to_string(),
            workspace: AgentSessionWorkspace {
                cwd: Some("C:\\work\\proj".to_string()),
                project_name: Some("proj".to_string()),
            },
            activity: AgentSessionActivity {
                started_at: Some(Utc.with_ymd_and_hms(2026, 7, 12, 0, 0, 0).unwrap()),
                last_activity_at: Some(Utc.with_ymd_and_hms(2026, 7, 12, 0, 1, 0).unwrap()),
            },
            focus_target: AgentSessionFocusTarget::Process { pid: 1234 },
        };

        let json = serde_json::to_string(&session).unwrap();
        let round_tripped: AgentSession = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, session);
        assert!(json.contains("\"focusTarget\""));
    }

    #[test]
    fn focus_result_serializes_safely() {
        let focused = serde_json::to_value(&SessionFocusResult::Focused).unwrap();
        let unsupported = serde_json::to_value(&SessionFocusResult::Unsupported {
            message: "focus unavailable".to_string(),
        })
        .unwrap();
        let failed = serde_json::to_value(&SessionFocusResult::Failed {
            message: "failed to focus".to_string(),
        })
        .unwrap();

        assert!(focused.is_string() || focused.is_object());
        assert_eq!(unsupported["message"], "focus unavailable");
        assert_eq!(failed["message"], "failed to focus");
    }

    #[test]
    fn host_validation_dedupes_and_rejects_unsafe_values() {
        let hosts = RemoteSessionFetcher::sanitized_hosts(&[
            "".to_string(),
            " ".to_string(),
            "-bad".to_string(),
            "good".to_string(),
            "good".to_string(),
            "GOOD".to_string(),
            "bad host".to_string(),
            "bad\tcontrol".to_string(),
        ]);
        assert_eq!(hosts, vec!["good".to_string()]);
    }

    #[test]
    fn codex_rollout_parser_reads_first_line_metadata() {
        let metadata = CodexRolloutFirstLineParser::parse(
            r#"{"type":"session_meta","payload":{"session_id":"abc","cwd":"C:\\work\\proj","originator":"codex_exec","source":"cli"}}"#,
        )
        .unwrap();
        assert_eq!(metadata.session_id, "abc");
        assert_eq!(metadata.cwd.as_deref(), Some("C:\\work\\proj"));
    }

    #[test]
    fn claude_cwd_escape_is_stable() {
        assert_eq!(
            ClaudeSessionProjectMapper::escaped_cwd(r"C:\Users\me\My Project!"),
            "C--Users-me-My-Project-"
        );
    }

    #[test]
    fn remote_session_result_round_trips() {
        let result = AgentSessionHostResult {
            host: "devbox".to_string(),
            sessions: vec![AgentSession {
                id: "session-1".to_string(),
                provider: AgentSessionProvider::Claude,
                source: AgentSessionSource::Cli,
                state: AgentSessionState::Idle,
                pid: None,
                transcript_path: None,
                host: "devbox".to_string(),
                workspace: AgentSessionWorkspace {
                    cwd: None,
                    project_name: None,
                },
                activity: AgentSessionActivity {
                    started_at: None,
                    last_activity_at: None,
                },
                focus_target: AgentSessionFocusTarget::None,
            }],
            error: Some("ssh not found".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        let decoded: AgentSessionHostResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn windows_process_parser_uses_metadata_and_ignores_command_lines() {
        let output = r#"[
            {
                "ProcessId": 101,
                "ParentProcessId": 1,
                "CreationDate": "/Date(1783773458193)/",
                "Name": "claude.exe",
                "ExecutablePath": "C:\\Tools\\claude.exe",
                "CommandLine": "claude.exe --api-key super-secret"
            },
            {
                "ProcessId": 202,
                "ParentProcessId": 1,
                "CreationDate": "2026-07-12T00:02:03Z",
                "Name": "codex.exe",
                "ExecutablePath": "C:\\Tools\\codex.exe"
            }
        ]"#;

        let records = WindowsProcessOutputParser::parse(output);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].provider, Some(AgentSessionProvider::Claude));
        assert_eq!(records[1].provider, Some(AgentSessionProvider::Codex));
        assert!(records[0].started_at.is_some());
        assert_eq!(records[0].executable, "claude.exe");
        assert!(!records[0].executable.contains("super-secret"));
    }

    #[test]
    fn windows_process_query_never_requests_raw_command_lines() {
        let options = LocalAgentSessionScanner::process_options(Duration::from_secs(2));
        let script = options.extra_args.last().unwrap();

        assert!(script.contains("ProcessId"));
        assert!(script.contains("ExecutablePath"));
        assert!(!script.contains("CommandLine"));
    }

    #[test]
    fn codex_discovery_only_builds_today_and_yesterday_directories() {
        let now = Utc.with_ymd_and_hms(2026, 7, 12, 1, 2, 3).unwrap();
        let root = Path::new(r"C:\Users\me\.codex\sessions");

        let directories = LocalAgentSessionScanner::codex_day_directories(root, now.date_naive());

        assert_eq!(
            directories,
            vec![
                root.join("2026").join("07").join("12"),
                root.join("2026").join("07").join("11"),
            ]
        );
    }

    #[test]
    fn claude_metadata_parser_stops_after_session_and_cwd_are_known() {
        struct FirstLineOnly {
            bytes: &'static [u8],
            offset: usize,
        }

        impl Read for FirstLineOnly {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                assert!(
                    self.offset < self.bytes.len(),
                    "parser read past complete metadata"
                );
                buffer[0] = self.bytes[self.offset];
                self.offset += 1;
                Ok(1)
            }
        }

        let input = FirstLineOnly {
            bytes:
                b"{\"type\":\"user\",\"sessionId\":\"session-1\",\"cwd\":\"C:\\\\work\\\\proj\"}\n",
            offset: 0,
        };

        let metadata = ClaudeTranscriptMetadataParser::parse(input).unwrap();

        assert_eq!(metadata.session_id.as_deref(), Some("session-1"));
        assert_eq!(metadata.cwd.as_deref(), Some(r"C:\work\proj"));
    }

    #[test]
    fn focus_rejects_remote_and_file_only_targets_explicitly() {
        let remote = AgentSession {
            id: "remote".to_string(),
            provider: AgentSessionProvider::Codex,
            source: AgentSessionSource::Cli,
            state: AgentSessionState::Active,
            pid: Some(42),
            transcript_path: None,
            host: "other-host".to_string(),
            workspace: AgentSessionWorkspace {
                cwd: None,
                project_name: None,
            },
            activity: AgentSessionActivity {
                started_at: None,
                last_activity_at: None,
            },
            focus_target: AgentSessionFocusTarget::Process { pid: 42 },
        };
        let file_only = AgentSession {
            host: "localhost".to_string(),
            focus_target: AgentSessionFocusTarget::Transcript {
                transcript_path: "session.jsonl".to_string(),
            },
            ..remote.clone()
        };

        assert!(matches!(
            focus_session(&remote),
            SessionFocusResult::Unsupported { .. }
        ));
        assert!(matches!(
            focus_session(&file_only),
            SessionFocusResult::Unsupported { .. }
        ));
    }

    #[test]
    fn ssh_fetch_uses_noninteractive_options_and_a_strict_timeout() {
        let options =
            RemoteSessionFetcher::ssh_options("user@devbox", Duration::from_secs(5)).unwrap();

        assert_eq!(options.timeout, Duration::from_secs(5));
        assert!(
            options
                .extra_args
                .windows(2)
                .any(|pair| pair == ["-o", "BatchMode=yes"])
        );
        assert!(
            options
                .extra_args
                .windows(2)
                .any(|pair| pair == ["-o", "ConnectTimeout=3"])
        );
        assert!(options.extra_args.iter().any(|arg| arg == "user@devbox"));
    }

    #[tokio::test]
    async fn ssh_hosts_are_fetched_in_parallel_and_failures_are_isolated() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let results = tokio::time::timeout(
            Duration::from_secs(1),
            RemoteSessionFetcher::fetch_hosts_with(
                &[
                    "slow-a".to_string(),
                    "slow-b".to_string(),
                    "broken".to_string(),
                ],
                |host| {
                    let barrier = Arc::clone(&barrier);
                    async move {
                        if host == "broken" {
                            return AgentSessionHostResult::failed(host, "unreachable");
                        }
                        barrier.wait().await;
                        AgentSessionHostResult::success(host, Vec::new())
                    }
                },
            ),
        )
        .await
        .expect("hosts were fetched sequentially");

        assert_eq!(results.len(), 3);
        assert_eq!(
            results
                .iter()
                .filter(|result| result.error.is_some())
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| result.error.is_none())
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn disabled_discovery_returns_without_running_scanners() {
        let result = AgentSessionDiscovery::default()
            .scan(AgentSessionDiscoveryMode::Disabled)
            .await;

        assert_eq!(result, AgentSessionDiscoveryResult::Disabled);
    }
}
