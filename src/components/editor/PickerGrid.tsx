import type { MessagesForLocale } from "../../locales/messages";
import { compactPathLabel } from "../../utils/pathLabels";

import { FileVideoIcon, FolderDownIcon, FolderOpenIcon } from "../AppIcons";

type PickerGridProps = {
  copy: MessagesForLocale;
  inputLabel: string;
  outputLabel: string;
  hasInputPath: boolean;
  hasOutputDirectory: boolean;
  inspectionLoading: boolean;
  openFolderDisabled: boolean;
  outputActionsDisabled: boolean;
  onPickInputFile: () => void;
  onPickOutputDirectory: () => void;
  onResetOutputDirectory: () => void;
  onOpenOutputFolder: () => void;
};

export function PickerGrid({
  copy,
  inputLabel,
  outputLabel,
  hasInputPath,
  hasOutputDirectory,
  inspectionLoading,
  openFolderDisabled,
  outputActionsDisabled,
  onPickInputFile,
  onPickOutputDirectory,
  onResetOutputDirectory,
  onOpenOutputFolder,
}: PickerGridProps) {
  const compactInputLabel = hasInputPath ? compactPathLabel(inputLabel) : inputLabel;
  const compactOutputLabel = hasOutputDirectory ? compactPathLabel(outputLabel) : outputLabel;

  return (
    <section className="desktopPickerGrid">
      <article className="pickerCard desktopPickerCard">
        <div className="pickerHeaderRow">
          <span className="metaLabel">{copy.sourceFile}</span>
        </div>
        <div className="pickerControlRow">
          <div className={hasInputPath ? "pickerDisplaySurface" : "pickerDisplaySurface is-empty"}>
            <FileVideoIcon size={18} className="pickerSurfaceIcon" />
            <strong className="pathValue" title={inputLabel}>{compactInputLabel}</strong>
          </div>
          <button
            className="secondaryAction desktopSelectButton"
            type="button"
            onClick={onPickInputFile}
            disabled={inspectionLoading}
            aria-label={copy.chooseMedia}
          >
            {copy.chooseMedia}
          </button>
        </div>
      </article>

      <article className="pickerCard desktopPickerCard">
        <div className="pickerHeaderRow">
          <span className="metaLabel">{copy.outputFolder}</span>
        </div>
        <div className="pickerControlRow">
          <div className={hasOutputDirectory ? "pickerDisplaySurface" : "pickerDisplaySurface is-muted"}>
            <FolderDownIcon size={18} className="pickerSurfaceIcon" />
            <strong className="pathValue" title={outputLabel}>{compactOutputLabel}</strong>
          </div>
          <div className="desktopPickerActions">
            <button
              className="secondaryAction iconButtonAction"
              type="button"
              onClick={onPickOutputDirectory}
              title={outputActionsDisabled ? copy.desktopOnlyFeature : copy.chooseFolder}
              aria-label={
                outputActionsDisabled
                  ? `${copy.chooseFolder}. ${copy.desktopOnlyFeature}`
                  : copy.chooseFolder
              }
              disabled={outputActionsDisabled}
            >
              <FolderOpenIcon size={16} />
            </button>
            <button
              className="secondaryAction"
              type="button"
              onClick={onResetOutputDirectory}
              disabled={outputActionsDisabled}
              title={outputActionsDisabled ? copy.desktopOnlyFeature : undefined}
              aria-label={
                outputActionsDisabled
                  ? `${copy.useSourceFolder}. ${copy.desktopOnlyFeature}`
                  : copy.useSourceFolder
              }
            >
              {copy.useSourceFolder}
            </button>
            <button
              className="subtleAction"
              type="button"
              onClick={onOpenOutputFolder}
              disabled={openFolderDisabled || outputActionsDisabled}
              title={
                outputActionsDisabled
                  ? copy.desktopOnlyFeature
                  : openFolderDisabled
                    ? copy.selectSourceFirst
                    : undefined
              }
              aria-label={
                outputActionsDisabled
                  ? `${copy.openFolder}. ${copy.desktopOnlyFeature}`
                  : openFolderDisabled
                    ? `${copy.openFolder}. ${copy.selectSourceFirst}`
                    : copy.openFolder
              }
            >
              {copy.openFolder}
            </button>
          </div>
        </div>
      </article>
    </section>
  );
}
