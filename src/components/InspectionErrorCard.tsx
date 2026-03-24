import type { MessagesForLocale } from "../locales/messages";

type InspectionErrorCardProps = {
  copy: MessagesForLocale;
  message: string | null;
};

export function InspectionErrorCard({ copy, message }: InspectionErrorCardProps) {
  return (
    <div className="errorCard" role="alert" aria-live="assertive">
      <p className="panelLabel">{copy.inspectionFailed}</p>
      <h2>{copy.inspectionFailed}</h2>
      <p>{message ?? "-"}</p>
    </div>
  );
}
