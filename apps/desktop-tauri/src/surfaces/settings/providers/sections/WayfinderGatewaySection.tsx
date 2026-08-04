import type { LocaleKey } from "../../../../i18n/keys";

interface Props {
  draft: string;
  error: string | null;
  busy: boolean;
  disabled: boolean;
  onDraftChange: (draft: string) => void;
  onSave: () => void;
  t: (key: LocaleKey) => string;
}

export function WayfinderGatewaySection({
  draft,
  error,
  busy,
  disabled,
  onDraftChange,
  onSave,
  t,
}: Props) {
  return (
    <section className="provider-detail__section">
      <h3>{t("WayfinderGatewayTitle")}</h3>
      <label>
        <span>{t("WayfinderGatewayLabel")}</span>
        <input
          type="url"
          value={draft}
          disabled={disabled || busy}
          onChange={(event) => onDraftChange(event.target.value)}
          aria-describedby="wayfinder-gateway-help"
        />
      </label>
      <p id="wayfinder-gateway-help">{t("WayfinderGatewayHelp")}</p>
      {error && <p role="alert">{error}</p>}
      <button
        type="button"
        disabled={disabled || busy}
        onClick={onSave}
      >
        {t("Save")}
      </button>
    </section>
  );
}
