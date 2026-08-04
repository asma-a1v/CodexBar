// `get_safe_diagnostics` (the only Tauri command this module exposed) was
// removed as orphaned dead code — the frontend wrappers that invoked it were
// deleted, leaving zero invokers. Its `SafeDiagnostics` payload + the
// `safe_diagnostics_from` builder + the secret-redaction test existed solely
// to support that command and were removed with it.
