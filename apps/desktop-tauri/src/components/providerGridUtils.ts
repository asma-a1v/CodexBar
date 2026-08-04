import type { ProviderUsageSnapshot } from "../types/bridge";

export function prioritizeProviders(
  providers: ProviderUsageSnapshot[],
  selectedProviderId: string | null,
): ProviderUsageSnapshot[] {
  if (!selectedProviderId) return providers;
  const selectedIndex = providers.findIndex((provider) => provider.providerId === selectedProviderId);
  if (selectedIndex < 0 || selectedIndex < 18) return providers;
  const selected = providers[selectedIndex];
  return [selected, ...providers.slice(0, selectedIndex), ...providers.slice(selectedIndex + 1)];
}
