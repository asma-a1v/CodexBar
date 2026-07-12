# Restore Providers Tab

## Goal

Restore provider management as a top-level Settings tab instead of embedding it below General settings.

## Design

- Add **Providers** immediately after **General** in the Settings tab bar.
- General contains only language, system, and automation settings and uses the normal settings-page scroll behavior.
- Providers retains the existing 600px split-pane layout, searchable provider sidebar, provider detail pane, and independent pane scrolling.
- Opening Settings with the `providers` route selects Providers directly.
- Other tabs, persisted settings, and provider behavior remain unchanged.

## Verification

- Add a focused Settings test proving General and Providers render as separate tab panels.
- Run frontend tests, type-check, locale parity, and the desktop build.
- Verify General, Providers, and provider-pane scrolling in the rebuilt app with CUA Driver.
