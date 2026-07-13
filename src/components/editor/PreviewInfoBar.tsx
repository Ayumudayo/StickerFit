import type { CropAspectRatioPreset } from "../../components/MediaSelectionPreview";
import type { MessagesForLocale } from "../../locales/messages";

import { ChevronDownIcon, RefreshIcon } from "../AppIcons";

type PreviewInfoBarProps = {
  copy: MessagesForLocale;
  quickResolution: string;
  quickFps: string | null;
  previewDurationLabel: string | null;
  selectionLabel: string;
  cropAspectRatioPreset: CropAspectRatioPreset;
  isCropSelectionReset: boolean;
  onCropAspectRatioPresetChange: (value: CropAspectRatioPreset) => void;
  onResetSelection: () => void;
};

export function PreviewInfoBar({
  copy,
  quickResolution,
  quickFps,
  previewDurationLabel,
  selectionLabel,
  cropAspectRatioPreset,
  isCropSelectionReset,
  onCropAspectRatioPresetChange,
  onResetSelection,
}: PreviewInfoBarProps) {
  return (
    <section className="previewInfoBar">
      <div className="previewInfoMetrics">
        <span className="previewMetricItem">
          <span className="metaLabel">{copy.resolution}</span>
          <strong>{quickResolution}</strong>
        </span>
        {quickFps ? (
          <span className="previewMetricItem">
            <span className="metaLabel">{copy.frameRate}</span>
            <strong>
              {quickFps}
              <span className="statSuffix">fps</span>
            </strong>
          </span>
        ) : null}
        {previewDurationLabel ? (
          <span className="previewMetricItem">
            <span className="metaLabel">{copy.duration}</span>
            <strong>{previewDurationLabel}</strong>
          </span>
        ) : null}
        <span className="previewMetricItem previewMetricSelection">
          <span className="metaLabel">{copy.selection}</span>
          <strong>{selectionLabel}</strong>
        </span>
      </div>

      <div className="previewInfoSettings">
        <label
          className="previewInfoField previewInfoFieldCrop"
          htmlFor="preview-crop-aspect-ratio-select"
        >
          <span className="metaLabel">{copy.cropAspectRatio}</span>
          <div className="previewInlineSelectShell">
            <select
              id="preview-crop-aspect-ratio-select"
              className="previewInlineSelect"
              value={cropAspectRatioPreset}
              onChange={(event) =>
                onCropAspectRatioPresetChange(
                  event.target.value as CropAspectRatioPreset,
                )
              }
            >
              <option value="free">{copy.cropAspectRatioFree}</option>
              <option value="1:1">1:1</option>
              <option value="4:3">4:3</option>
              <option value="3:4">3:4</option>
              <option value="16:9">16:9</option>
              <option value="9:16">9:16</option>
            </select>
            <ChevronDownIcon size={14} className="previewInlineChevron" />
          </div>
        </label>

        <button
          className="secondaryAction compactAction previewInfoAction previewInfoResetAction"
          type="button"
          onClick={onResetSelection}
          disabled={isCropSelectionReset}
        >
          <RefreshIcon size={14} />
          <span>{copy.resetSelection}</span>
        </button>
      </div>
    </section>
  );
}
