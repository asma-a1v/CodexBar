# Privacy policy

This is the privacy policy for **Win-CodexBar**, a Windows tray/desktop app that
displays AI provider usage quotas on your own machine. It applies to the
installer and portable builds published on
[GitHub Releases](https://github.com/nesszer/Win-CodexBar/releases).

Last updated: 2026-08-06.

## Summary

- Win-CodexBar sends **nothing** to project-controlled servers. There are none.
- **No analytics, no telemetry, no crash reporting, no advertising SDKs.**
- Everything the app reads or stores stays on your device unless you
  explicitly configure a provider, in which case the app talks only to that
  provider's API using the credentials you supplied.
- Update checks contact GitHub only (details below), with no identifiers or
  usage data attached.

## What the app collects

**Nothing.** The project operates no servers, accounts, or data pipelines.
No usage data, device identifiers, provider metrics, credentials, or content
leave your machine for any project-controlled destination, because no such
destination exists.

## What stays on your disk

The app stores configuration and caches locally, under your Windows user
profile. Resolve the live locations with `codexbar config path` or see
[CONFIGURATION.md](CONFIGURATION.md).

### Settings and secrets — `%APPDATA%\CodexBar\`

| Store | Typical file | Contents |
|-------|--------------|----------|
| Settings | `settings.json` | Enabled providers, source choice per provider, UI/theme/refresh preferences, update settings |
| Manual cookies | `manual_cookies.json` | Cookie headers you paste in Settings for cookie-backed providers |
| API keys | `api_keys.json` | API keys you enter for key-based providers |
| Token accounts | `token-accounts.json` | Named API-token accounts you configure |
| Hooks | `hooks.json` | Optional local executable hooks you enable |

Secret material (API keys, manual cookies, token accounts) is read and written
through the app's secure-file layer, which applies user-scoped **Windows DPAPI**
protection where available; some credential types additionally use Windows
Credential Manager through keyring helpers. Secrets are never written to logs.

### Usage caches — `%LOCALAPPDATA%\CodexBar\`

| Store | Contents |
|-------|----------|
| `cost-usage\` | Local token-cost scan cache parsed from your own provider CLI logs |
| `*-cookie.json` | Short-lived per-provider cookie snapshots |
| `local-usage\codex-workspaces-v1.sqlite` | Codex Workspaces snapshot sidecar |
| `openai-dashboard.json`, widget/tray snapshots | Last-rendered usage display state |

Startup diagnostics go to `%TEMP%\codexbar_launch_<pid>.log` on launch failure
and contain no credentials.

All of the above can be deleted at any time: uninstall the app (or quit the
portable build) and remove `%APPDATA%\CodexBar\` and `%LOCALAPPDATA%\CodexBar\`.
Deleting these folders removes every trace the app wrote, including secrets.

### Browser cookies

If — and only if — you opt in per provider, the app reads cookies for that
provider's domain from your local Chrome, Edge, Brave, or Firefox profile
(Chromium cookies are decrypted locally with your user DPAPI key; Firefox
cookies are read from its SQLite store). Cookies are used only to call that
provider's own web endpoints for usage data. See [COOKIES.md](COOKIES.md).

## Network endpoints the app contacts

The app makes outbound connections only for the following purposes:

1. **Provider APIs you enable and configure.** Each enabled provider's usage
   quota is fetched from that provider's own API or web dashboard, using the
   credential type you chose (API key, OAuth token, browser/manual cookies, or
   the provider's local CLI). The set of providers and their strategies is
   documented in [PROVIDERS.md](PROVIDERS.md). Your credentials are sent only
   to the corresponding provider, subject to that provider's own privacy
   policy. Optionally, provider *status pages* may be polled for incident
   status where that toggle is enabled.
2. **Release/update checks against GitHub.** When you choose
   "Check for Updates…" from the About tab — or if you explicitly enable
   automatic update downloads in Settings (off by default) — the app sends a
   plain GET request to the GitHub Releases API
   (`https://api.github.com/repos/.../Win-CodexBar/releases`) to learn the
   latest version. With your approval it then downloads the installer asset
   from `github.com`. The downloaded installer is applied only after its
   SHA-256 digest is re-verified against the digest GitHub computed for the
   release asset. Update requests contain no identifiers, no usage data, and
   no telemetry — they are ordinary GitHub API requests; GitHub's own privacy
   statement applies to them.

There is no other update or telemetry channel, and no request is made before
you enable a provider or trigger an update check.

## Third-party data processors

**None.** No analytics vendor, crash-reporting service, or other third party
receives data from Win-CodexBar. The only external parties that ever see a
request are (a) the AI providers you deliberately configure and (b) GitHub as
the release host you contact for updates.

## Retention

- **Remote:** not applicable — the project holds no data about you, so there
  is nothing to retain or delete remotely.
- **Local:** settings and secrets persist until you change or delete them in
  the app; caches persist until refreshed or removed. You can erase everything
  at any time by deleting the `%APPDATA%\CodexBar\` and
  `%LOCALAPPDATA%\CodexBar\` folders described above.

## Diagnostics

If you generate a diagnostics bundle or report an issue, the app's
diagnostics expose provider/source/status metadata only — never raw cookies,
API keys, bearer tokens, or OAuth values. Review anything you paste into a
public issue regardless.

## Changes to this policy

Changes ship with the source tree in this file and are visible in git history;
material changes will also be noted in release notes.

## Contact

Questions or data concerns: open an issue at
<https://github.com/nesszer/Win-CodexBar/issues>.
