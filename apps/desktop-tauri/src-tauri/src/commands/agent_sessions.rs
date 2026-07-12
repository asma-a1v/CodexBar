use codexbar::agent_sessions::{
    AgentSession, AgentSessionDiscovery, AgentSessionDiscoveryMode, AgentSessionDiscoveryResult,
    SessionFocusResult, focus_session,
};
use codexbar::settings::Settings;

#[tauri::command]
pub async fn list_agent_sessions() -> AgentSessionDiscoveryResult {
    let settings = Settings::load();
    let mode = if settings.agent_sessions_enabled {
        AgentSessionDiscoveryMode::Enabled {
            ssh_hosts: settings.agent_session_ssh_hosts,
        }
    } else {
        AgentSessionDiscoveryMode::Disabled
    };
    AgentSessionDiscovery::default().scan(mode).await
}

#[tauri::command]
pub fn focus_agent_session(session: AgentSession) -> SessionFocusResult {
    focus_session(&session)
}
