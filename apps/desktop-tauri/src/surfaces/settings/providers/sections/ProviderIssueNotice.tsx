import type { ProviderDetail } from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";

interface Props {
  detail: ProviderDetail;
  message: string;
  onCopy: () => void;
  t: (key: LocaleKey) => string;
}

export function ProviderIssueNotice({ detail, message, onCopy, t }: Props) {
  const cleaned = message.replace(/^last fetch failed:\s*/i, "").trim();
  const lower = cleaned.toLowerCase();
  const needsLogin =
    lower.includes("auth.json not found") ||
    lower.includes("not signed in") ||
    lower.includes("credentials not found") ||
    lower.includes("oauth credentials not found") ||
    lower.includes("run `") ||
    lower.includes("run codex") ||
    lower.includes("run claude");
  const title = needsLogin
    ? `${detail.displayName} ${t("ProviderIssueNeedsSignIn")}`
    : t("ProviderIssueFetchNeedsAttention");
  const displayMessage = localizeProviderIssue(cleaned, t);

  return (
    <div className="provider-detail-error" role="status">
      <div className="provider-detail-error__header">
        <strong>{title}</strong>
        <button
          type="button"
          className="provider-detail-error__copy"
          onClick={onCopy}
        >
          {t("ProviderIssueCopy")}
        </button>
      </div>
      <p>{displayMessage}</p>
    </div>
  );
}

function localizeProviderIssue(
  message: string,
  t: (key: LocaleKey) => string,
): string {
  const unsupported = message.match(
    /^Source mode `?([^`']+)`? not supported for this provider$/i,
  );
  if (unsupported) {
    return `${t("ProviderIssueUnsupportedSourceModePrefix")} (${unsupported[1]})`;
  }
  return message;
}
