import { Fragment, useEffect, useState, type CSSProperties } from "react";
import type { BootstrapState, ProviderUsageSnapshot, UsageSpendSummary } from "../types/bridge";
import type { LocaleKey } from "../i18n/keys";
import { getUsageSpendSummary, openProviderDashboard, openProviderStatusPage } from "../lib/tauri";
import { useTrayPanelController } from "../hooks/useTrayPanelController";
import MenuCard from "../components/MenuCard";
import MenuSurface, { MenuEmpty } from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import ProviderGrid from "../components/ProviderGrid";
import AgentSessions from "../components/AgentSessions";

/** Provider IDs that have a dashboard URL in the backend */
const HAS_DASHBOARD = new Set([
  "abacus", "alibaba", "alibabatokenplan", "amp", "augment",
  "azureopenai", "bedrock", "claude", "codex", "codebuff",
  "aiand", "commandcode", "copilot", "crof", "crossmodel", "cursor", "deepgram", "deepinfra", "deepseek", "zenmux", "clinepass", "longcat", "neuralwatt", "zoommate",
  "doubao", "elevenlabs", "factory", "gemini", "grok", "groq",
  "infini", "jetbrains", "kilo", "kimi", "kimik2", "kiro", "manus",
  "mimo", "minimax", "mistral", "nanogpt", "notion", "ollama", "openaiapi",
  "opencode", "opencodego", "openrouter", "perplexity", "qoder", "codebuddy", "sakana", "stepfun",
  "t3chat", "venice", "vertexai", "warp", "windsurf",
  "xai", "zai", "fireworks",
]);
/** Provider IDs that have a status page URL in the backend */
const HAS_STATUS_PAGE = new Set([
  "alibabatokenplan", "amp", "augment", "azureopenai", "bedrock",
  "claude", "codex", "copilot", "deepgram", "deepinfra", "deepseek", "zenmux", "clinepass", "longcat", "neuralwatt", "zoommate", "elevenlabs",
  "gemini", "grok", "groq", "kiro", "mistral", "openaiapi",
  "openrouter", "vertexai", "windsurf", "xai",
]);

/**
 * Tray popover surface — two modes like macOS CodexBar:
 * 1. Overview (default): provider grid + all cards stacked
 * 2. Detail: click a provider in grid → show only that provider's card
 */
export default function TrayPanel({ state }: { state: BootstrapState }) {
  const {
    t,
    settings,
    isRefreshing,
    refreshingProviderIds,
    refresh,
    hasCachedData,
    sorted,
    denseTrayProviders,
    expectsDenseOverview,
    selectedProviderId,
    gridExpanded,
    setGridExpanded,
    visibleProviders,
    requestLayout,
    headerActions,
    footerRows,
    updateState,
    checkNow,
    download,
    apply,
    dismiss,
    openRelease,
    openSettings,
    handleGridClick,
    handleReorder,
    handleGestureStart,
    handleGestureEnd,
    revealClassName,
  } = useTrayPanelController(state);

  const banner = (
    <UpdateBanner
      updateState={updateState}
      onCheck={checkNow}
      onDownload={download}
      onApply={apply}
      onDismiss={dismiss}
      onOpenRelease={openRelease}
    />
  );

  const renderProviderCard = (p: ProviderUsageSnapshot) => {
    const isSelected =
      selectedProviderId !== null && p.providerId === selectedProviderId;
    return (
      <div
        className={`menu-stack__item${isSelected ? " menu-stack__item--selected" : ""}`}
        id={`card-${p.providerId}`}
        key={p.providerId}
      >
        <MenuCard
          provider={p}
          isRefreshing={refreshingProviderIds.has(p.providerId)}
          display={{
            hideEmail: settings.hidePersonalInfo,
            resetTimeRelative: settings.resetTimeRelative,
            showResetWhenExhausted: settings.showResetWhenExhausted,
            showPace: settings.showPace ?? true,
            showAsUsed: settings.showAsUsed,
            compactMetrics: selectedProviderId === null,
            costSummaryDisplayStyle: settings.costSummaryDisplayStyle,
          }}
          accentColor={settings.providerAccentColors[p.providerId]}
          onLayoutChange={requestLayout}
        />
      </div>
    );
  };

  if (sorted.length === 0) {
    return (
      <div className={revealClassName}>
        <MenuSurface
          variant="tray"
          onRefresh={refresh}
          isRefreshing={isRefreshing}
          actions={headerActions}
          banner={banner}
          footerRows={footerRows}
        >
          {settings.agentSessionsEnabled && <AgentSessions />}
          <MenuEmpty
            isLoading={isRefreshing && !hasCachedData}
            onSettings={openSettings}
          />
        </MenuSurface>
      </div>
    );
  }

  return (
    <div className={revealClassName}>
      <MenuSurface
        variant="tray"
        onRefresh={refresh}
        isRefreshing={isRefreshing}
        actions={headerActions}
        banner={banner}
        footerRows={footerRows}
      >
        {settings.agentSessionsEnabled && <AgentSessions />}
        <ProviderGrid
          providers={expectsDenseOverview ? denseTrayProviders : sorted}
          selectedProviderId={selectedProviderId}
          showAsUsed={settings.showAsUsed}
          showProviderIcons={settings.switcherShowsIcons}
          expanded={gridExpanded}
          onExpandedChange={setGridExpanded}
          onSelect={handleGridClick}
          onReorder={handleReorder}
          onGestureStart={handleGestureStart}
          onGestureEnd={handleGestureEnd}
        />
        <div className="provider-grid__divider" />
        {selectedProviderId === null && (
          <OverviewSpendSummary providerIds={sorted.map((provider) => provider.providerId)} t={t} />
        )}
        <div className="menu-stack">
          {visibleProviders.map((p, idx) => (
            <Fragment key={p.providerId}>
              {idx > 0 && <div className="menu-stack__sep" />}
              {renderProviderCard(p)}
            </Fragment>
          ))}
        </div>
        {/* Context actions — detail mode only, matches macOS actionsSection */}
        {selectedProviderId && (HAS_DASHBOARD.has(selectedProviderId) || HAS_STATUS_PAGE.has(selectedProviderId)) && (
          <div className="context-actions">
            <div className="context-actions__divider" />
            {HAS_DASHBOARD.has(selectedProviderId) && (
              <button
                type="button"
                className="context-actions__btn"
                onClick={() => void openProviderDashboard(selectedProviderId)}
              >
                <span className="context-actions__icon" aria-hidden>
                  <svg width="13" height="13" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
                    <rect x="2" y="9" width="2.5" height="5" rx="0.6" fill="currentColor" />
                    <rect x="6.75" y="6" width="2.5" height="8" rx="0.6" fill="currentColor" />
                    <rect x="11.5" y="3" width="2.5" height="11" rx="0.6" fill="currentColor" />
                  </svg>
                </span>
                {t("ActionUsageDashboard")}
              </button>
            )}
            {HAS_STATUS_PAGE.has(selectedProviderId) && (
              <button
                type="button"
                className="context-actions__btn"
                onClick={() => void openProviderStatusPage(selectedProviderId)}
              >
                <span className="context-actions__icon" aria-hidden>
                  <svg width="14" height="13" viewBox="0 0 18 14" fill="none" xmlns="http://www.w3.org/2000/svg">
                    <path d="M1 7H4L5.5 3L8 11L10.5 5L12 7H17" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" fill="none" />
                  </svg>
                </span>
                {t("ActionStatusPage")}
              </button>
            )}
          </div>
        )}
      </MenuSurface>
    </div>
  );
}
function OverviewSpendSummary({ providerIds, t }: { providerIds: string[]; t: (key: LocaleKey) => string }) {
  const [summary, setSummary] = useState<UsageSpendSummary | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getUsageSpendSummary({ historyDays: 30 })
      .then((value) => { if (!cancelled) setSummary(value); })
      .catch(() => { if (!cancelled) setSummary(null); });
    return () => { cancelled = true; };
  }, [providerIds.join("|")]);

  if (!summary) return null;
  // Overview consumes the same backend spend catalog as Usage & Spend. Do not
  // restrict accounting to whichever cards happen to be rendered in this tray.
  const rows = summary.rows.filter((row) => row.includedInOverview !== false);
  const summable = rows.filter((row) => (row.currency || "USD") === "USD");
  const known = summable.filter((row) => row.thirtyDay != null && Number.isFinite(row.thirtyDay));
  if (known.length === 0) return null;
  const total = known.reduce((sum, row) => sum + (row.thirtyDay ?? 0), 0);
  const partial = known.length < rows.length;
  const formatter = new Intl.NumberFormat(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 2 });

  return (
    <div className="provider-detail-section" style={{ margin: "8px 8px 10px", padding: "10px 12px" }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: 12, alignItems: "baseline" }}>
        <strong>{t("OverviewSpendTitle")}</strong>
        <strong>{partial ? "~" : ""}{formatter.format(total)}</strong>
      </div>
      <div className="settings-section__caption" style={{ marginTop: 4 }}>
        {known.length} of {rows.length} {t("OverviewSpendProviderCoverage")} · {t("OverviewSpendEstimate")}
      </div>
    </div>
  );
}
