# Release Notes Format

Win-CodexBar release notes follow the format established in `CHANGELOG.md` (Keep a Changelog style). Use this template for every release.

## Template

```markdown
## [Windows] X.Y.Z - YYYY-MM-DD

Windows port of upstream CodexBar **old → new**.

### Added
- Area: description (#PR).

### Fixed
- Area: description (#PR).

### Changed
- Area: description (#PR).

> Release artifacts are unsigned (SignPath onboarding pending); verify them against the attached `.sha256` sidecar files.
```

## Rules

1. **Header**: `## [Windows] X.Y.Z - YYYY-MM-DD` — ISO date, same as the tag date.
2. **Intro line**: `Windows port of upstream CodexBar **old → new**.` with the version range ported in bold.
3. **Three sections**: `### Added`, `### Fixed`, `### Changed` — in that order. Omit a section if it has no entries.
4. **Bullets**: `Area: description (#PR).` — `Area` is a category prefix (Providers, Cost, Settings, Tray, Charts, CLI, Serve, i18n, CI, Docs). Link every PR.
5. **Footer**: Blockquote about unsigned artifacts — keep until SignPath is production-ready.
6. **Separator**: `---` between entries in CHANGELOG.md.
7. **CHANGELOG.md**: Add the entry at the top (after `# Changelog` header) before tagging the release.
8. **GitHub release**: Use the same body as the CHANGELOG.md entry. Title is `Win-CodexBar X.Y.Z` (not `vX.Y.Z`).
