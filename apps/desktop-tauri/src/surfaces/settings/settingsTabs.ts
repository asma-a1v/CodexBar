import type { BootstrapState, SettingsTabId, SettingsUpdate } from "../../types/bridge";
import type { LocaleKey } from "../../i18n/keys";

export const TAB_META: { id: SettingsTabId; labelKey: LocaleKey }[] = [
  { id: "general", labelKey: "TabGeneral" },
  { id: "providers", labelKey: "TabProviders" },
  { id: "notifications", labelKey: "TabNotifications" },
  { id: "menuBar", labelKey: "TabMenuBar" },
  { id: "menu", labelKey: "TabMenu" },
  { id: "usageSpend", labelKey: "TabUsageSpend" },
  { id: "advanced", labelKey: "TabAdvanced" },
  { id: "about", labelKey: "TabAbout" },
];

export interface TabProps {
  settings: BootstrapState["settings"];
  set: (p: SettingsUpdate) => void;
  saving: boolean;
}

export function isSettingsTab(value: string): value is SettingsTabId {
  return TAB_META.some((t) => t.id === value);
}
