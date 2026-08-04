# Win-CodexBar CI and local checks

**PR validation** is hosted on Blacksmith Windows via `.github/workflows/pr-check.yml`
(budget-gated by `vars.CI_BUDGET_MODE`; see `CONTEXT.md`). **Release packaging and
upload stay local.**

## PR checks

Hosted job (when budget mode is not `off`): `cargo fmt --check`, clippy + test on
both `rust/Cargo.toml` and `apps/desktop-tauri/src-tauri/Cargo.toml`, then
`pnpm --dir apps/desktop-tauri test` and `pnpm --dir apps/desktop-tauri run build`.

Mirror locally before opening a PR:

```powershell
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -Format -Clippy
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -All -Version <version>
```

## Release checks

The canonical Windows release path uses the release scripts:

```powershell
powershell.exe -File scripts\release-doctor.ps1 -Version <version>
powershell.exe -File scripts\windows-release-build.ps1 -Ref v<version> -SmokeInstall
```

Use the version/ref being released.

## Release flow

1. Tag the release, for example `vX.Y.Z`.
2. Run `scripts\release-doctor.ps1`.
3. Run `scripts\windows-release-build.ps1 -SmokeInstall`.
4. Upload the verified installer, portable exe, and SHA-256 sidecars to GitHub Releases.
5. Submit the Winget manifest update after the GitHub installer URL is stable.
