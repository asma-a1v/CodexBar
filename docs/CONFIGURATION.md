# Configuration (Windows)

Windows rewrite of upstream `docs/configuration.md` and `docs/cli-configuration.md`.
Upstream default paths (`~/.config/codexbar/config.json`, `~/.codexbar/config.json`, macOS Keychain layout) are **not** the primary story here.

## Location

On Windows, config lives under the roaming app data directory:

| Store | Typical path |
|-------|----------------|
| Settings | `%AppData%\Roaming\CodexBar\settings.json` |
| Manual cookies | `%AppData%\Roaming\CodexBar\manual_cookies.json` |
| API keys | `%AppData%\Roaming\CodexBar\api_keys.json` |
| Token accounts | `%AppData%\Roaming\CodexBar\token-accounts.json` |

Resolve at runtime:

```powershell
codexbar config path
```

Implementation: `dirs::config_dir()/CodexBar/...` via `Settings::settings_path()` and related helpers in `rust/src/settings/`. Reads/writes go through `secure_file` (can use Windows DPAPI protection for sensitive material).

Desktop UI and CLI share these stores. Prefer the Settings window for day-to-day toggles; use `codexbar config` for scripts/CI.

## What lives where (conceptual)

Aligned with upstream *ideas*, mapped to this port:

- **Enabled providers, theme, refresh, float bar, UI language, metrics, …** → `settings.json`
- **Manual cookie headers** → `manual_cookies.json` (and/or settings fields depending on provider path)
- **API keys** → `api_keys.json` / keyring helpers where used
- **Token accounts** → `token-accounts.json`
- **Browser auto cookies** → extracted at runtime from Chrome/Edge/Brave/Firefox profiles (see [COOKIES.md](./COOKIES.md)); not a substitute for committing secrets into git

Do not commit real `settings.json` / key files into the repo.

## CLI configuration commands

```powershell
codexbar config providers              # list enablement
codexbar config providers --json --pretty
codexbar config enable -p grok
codexbar config disable -p cursor
codexbar config validate
codexbar config dump
codexbar config path

# API key via stdin (example)
printf '%s' $env:OPENROUTER_API_KEY | codexbar config set-api-key -p openrouter --stdin
```

Notes:

- `enable` / `disable` are **persistent** (same idea as upstream).
- `codexbar usage -p <provider>` is a **one-shot** query override; it is not a full substitute for enable/disable.
- If every provider is disabled, bare `usage` may print nothing useful; pass `-p` explicitly to force a provider for that run.

## Settings UI tabs (desktop)

Canonical tab ids (frontend + proof harness whitelist must match):

`general`, `providers`, `notifications`, `menuBar`, `menu`, `usageSpend`, `advanced`, `about`

Unknown ids fall back to General. Legacy ids `display` / `apiKeys` / `cookies` are **not** settings tabs (content lives under other tabs / provider detail).

Proof / automation example:

```powershell
$env:CODEXBAR_PROOF_MODE = "settings:menu"
# then launch the desktop binary
```

## Source mode

CLI `--source` values on this port (see `codexbar usage --help`): `auto`, `web`, `cli`, `oauth`.

Upstream also documents `api` extensively; treat per-provider support as defined by **this** codebase’s provider modules and help text, not by copying upstream tables blindly.

## Hooks

Upstream documents a rich `hooks` block in JSON config. This port exposes `codexbar hooks` for list/enable/disable/test. Configure trusted local executables only; never point hooks at untrusted paths. Prefer reading `codexbar hooks --help` and Settings UI for the supported surface on the version you run.

## Start at login (Windows)

Desktop start-at-login uses `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `CodexBar` pointing at the desktop executable (managed via settings). CLI also has `codexbar autostart` for boot integration helpers.

## Security

- Do not log cookies, tokens, or API keys (`tracing` only, redacted helpers).
- Manual cookie paste and API keys are secrets — handle like passwords.
- `codexbar serve` on non-loopback without TLS sends bearer tokens in cleartext; require intentional flags/env (see [CLI.md](./CLI.md)).

## Related

- [CLI.md](./CLI.md)
- [COOKIES.md](./COOKIES.md)
- [PROVIDERS.md](./PROVIDERS.md)
- Root [AGENTS.md](../AGENTS.md)
