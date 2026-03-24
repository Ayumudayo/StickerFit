import type { Locale, MessagesForLocale } from "../../locales/messages";

import { CheckCircleIcon, FileBadgeIcon, InfoIcon } from "../AppIcons";

type DesktopHeaderProps = {
  copy: MessagesForLocale;
  locale: Locale;
  healthLabel: string;
  isToolReady: boolean;
  onLocaleChange: (locale: Locale) => void;
};

export function DesktopHeader({
  copy,
  locale,
  healthLabel,
  isToolReady,
  onLocaleChange,
}: DesktopHeaderProps) {
  const HeaderStatusIcon = isToolReady ? CheckCircleIcon : InfoIcon;

  return (
    <section className="topBar desktopHeader">
      <div className="topBarCopy desktopHeaderCopy">
        <h1 className="desktopTitle">
          <FileBadgeIcon className="desktopTitleIcon" />
          <span>{copy.title}</span>
        </h1>
        <p className="lede">{copy.lede}</p>
      </div>

      <div className="statusCluster desktopHeaderActions">
        <span
          className={
            isToolReady
              ? "statusBadge statusOk desktopStatus"
              : "statusBadge statusWarn desktopStatus"
          }
          role="status"
          aria-live="polite"
        >
          <HeaderStatusIcon size={16} />
          <span>{healthLabel}</span>
        </span>
        <div className="localeSwitch">
          <button
            className={locale === "en" ? "localeButton active" : "localeButton"}
            type="button"
            onClick={() => onLocaleChange("en")}
          >
            EN
          </button>
          <button
            className={locale === "ko" ? "localeButton active" : "localeButton"}
            type="button"
            onClick={() => onLocaleChange("ko")}
          >
            KO
          </button>
        </div>
      </div>
    </section>
  );
}
