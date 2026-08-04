import { useEffect, useState } from "react";
import { useLocale } from "./useLocale";

export type ResetTimeFormatMode = "reset" | "expires";

const absoluteResetFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});

/**
 * Format a provider's reset timestamp for display.
 *
 * When `relative` is true, returns a live countdown string. `mode` selects
 * reset wording ("Resets in …") vs expiry wording ("Next expires in …").
 *
 * When `relative` is false, returns the absolute reset time converted to
 * the user's local timezone via `Intl.DateTimeFormat`.
 *
 * Falls back to `fallback` (typically the backend's `resetDescription`) when
 * `resetsAt` is absent or unparseable.
 */
export function useFormattedResetTime(
  resetsAt: string | null,
  fallback: string | null,
  relative: boolean,
  mode: ResetTimeFormatMode = "reset",
): string | null {
  const { t } = useLocale();
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!resetsAt) return;
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, [resetsAt, relative]);

  if (!resetsAt) {
    return fallback;
  }
  const target = Date.parse(resetsAt);
  if (Number.isNaN(target)) {
    return fallback;
  }

  if (relative) {
    const diffMs = target - now;
    const dueNowKey = mode === "expires" ? "NextExpiresDueNow" : "TrayResetsDueNow";
    if (diffMs <= 0) return t(dueNowKey);
    const totalMinutes = Math.floor(diffMs / 60_000);
    const days = Math.floor(totalMinutes / 1440);
    const hours = Math.floor((totalMinutes % 1440) / 60);
    const minutes = totalMinutes % 60;
    if (days > 0) {
      const key = mode === "expires" ? "NextExpiresInDaysHours" : "ResetsInDaysHours";
      return t(key)
        .replace("{}", String(days))
        .replace("{}", String(hours));
    }
    if (hours === 0) {
      const key = mode === "expires" ? "NextExpiresInMinutes" : "ResetsInMinutes";
      return t(key).replace("{}", String(minutes));
    }
    const key = mode === "expires" ? "NextExpiresInHoursMinutes" : "ResetsInHoursMinutes";
    return t(key)
      .replace("{}", String(hours))
      .replace("{}", String(minutes));
  }

  try {
    return absoluteResetFormatter.format(new Date(target));
  } catch {
    return fallback;
  }
}
