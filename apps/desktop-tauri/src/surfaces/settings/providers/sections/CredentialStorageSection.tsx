import type { CredentialStorageStatus } from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";

interface Props {
  status: CredentialStorageStatus | null;
  busy: boolean;
  onRevoke: () => void;
  t: (key: LocaleKey) => string;
}

export function CredentialStorageSection({
  status,
  busy,
  onRevoke,
  t,
}: Props) {
  if (!status) return null;

  return (
    <section className="provider-detail-section provider-detail-credential-storage">
      <div className="provider-detail-section__header">
        <h4>{t("CredentialStorageTitle")}</h4>
        <button
          type="button"
          className="credential-btn credential-btn--danger"
          disabled={busy}
          onClick={onRevoke}
        >
          {t("CredentialRevokeStored")}
        </button>
      </div>
      <dl className="provider-detail-grid provider-detail-grid--storage">
        <dt>{t("CredentialApiKeys")}</dt>
        <dd>{storageLabel(status.apiKeys, t)}</dd>
        <dt>{t("CredentialManualCookies")}</dt>
        <dd>{storageLabel(status.manualCookies, t)}</dd>
        <dt>{t("CredentialTokenAccounts")}</dt>
        <dd>{storageLabel(status.tokenAccounts, t)}</dd>
      </dl>
    </section>
  );
}

function storageLabel(value: string, t: (key: LocaleKey) => string): string {
  if (value.startsWith("protected:")) {
    return `${t("CredentialProtectedPrefix")} (${value.slice("protected:".length)})`;
  }
  switch (value) {
    case "missing":
      return t("CredentialStatusNotCreated");
    case "plaintext":
      return t("CredentialStatusPlaintext");
    case "unavailable":
      return t("CredentialStatusUnavailable");
    case "unreadable":
      return t("CredentialStatusUnreadable");
    default:
      return value;
  }
}
