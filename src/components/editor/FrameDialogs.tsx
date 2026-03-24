import type { ChangeEvent, RefObject } from "react";

import type { EditorText } from "../../locales/editorText";
import type { FrameDurationDialogMode, FrameDurationDialogState } from "../../types/editor";

type FrameDialogsProps = {
  ui: EditorText;
  frameDurationDialog: FrameDurationDialogState | null;
  frameDurationDialogRef: RefObject<HTMLElement | null>;
  frameDurationMode: FrameDurationDialogMode;
  frameDurationFpsValue: number;
  frameDurationSecondsValue: number;
  hasMixedSelectedDurations: boolean;
  onFrameDurationModeChange: (mode: FrameDurationDialogMode) => void;
  onFrameDurationFpsChange: (value: number) => void;
  onFrameDurationSecondsChange: (value: number) => void;
  onApplyFrameDuration: (durationUs: number) => void;
  onCloseFrameDurationDialog: () => void;
  showNthSelectionDialog: boolean;
  nthSelectionDialogRef: RefObject<HTMLElement | null>;
  nthSelectionStep: number;
  onNthSelectionStepChange: (value: number) => void;
  onApplyNthFrameSelection: () => void;
  onCloseNthSelectionDialog: () => void;
};

export function FrameDialogs({
  ui,
  frameDurationDialog,
  frameDurationDialogRef,
  frameDurationMode,
  frameDurationFpsValue,
  frameDurationSecondsValue,
  hasMixedSelectedDurations,
  onFrameDurationModeChange,
  onFrameDurationFpsChange,
  onFrameDurationSecondsChange,
  onApplyFrameDuration,
  onCloseFrameDurationDialog,
  showNthSelectionDialog,
  nthSelectionDialogRef,
  nthSelectionStep,
  onNthSelectionStepChange,
  onApplyNthFrameSelection,
  onCloseNthSelectionDialog,
}: FrameDialogsProps) {
  return (
    <>
      {frameDurationDialog ? (
        <div className="overlayBackdrop">
          <section
            ref={frameDurationDialogRef}
            className="dialogCard"
            role="dialog"
            aria-modal="true"
          >
            <div className="cardHeading">
              <h3>{ui.frameDurationDialogTitle}</h3>
            </div>
            <div className="durationDialogBody">
              <label className="durationModeRow">
                <input
                  type="radio"
                  checked={frameDurationMode === "fps"}
                  onChange={() => onFrameDurationModeChange("fps")}
                />
                <span>{ui.frameDurationLabel}</span>
                <input
                  className="fieldInput durationInput"
                  type="number"
                  min={1}
                  step={1}
                  value={frameDurationFpsValue}
                  onChange={(event: ChangeEvent<HTMLInputElement>) =>
                    onFrameDurationFpsChange(Number(event.target.value) || 1)
                  }
                  disabled={frameDurationMode !== "fps"}
                />
                <span>FPS</span>
              </label>
              <label className="durationModeRow">
                <input
                  type="radio"
                  checked={frameDurationMode === "seconds"}
                  onChange={() => onFrameDurationModeChange("seconds")}
                />
                <input
                  className="fieldInput durationInput"
                  type="number"
                  min={0.01}
                  step={0.05}
                  value={frameDurationSecondsValue.toFixed(2)}
                  onChange={(event: ChangeEvent<HTMLInputElement>) =>
                    onFrameDurationSecondsChange(Number(event.target.value) || 0.01)
                  }
                  disabled={frameDurationMode !== "seconds"}
                />
                <span>{ui.secondsLabel}</span>
              </label>
              {hasMixedSelectedDurations ? (
                <p className="summaryText">{ui.mixedDurationHint}</p>
              ) : null}
            </div>
            <div className="dialogActionRow">
              <button
                type="button"
                className="primaryAction"
                onClick={() => onApplyFrameDuration(frameDurationDialog.durationUs)}
              >
                {ui.confirm}
              </button>
              <button type="button" className="secondaryAction" onClick={onCloseFrameDurationDialog}>
                {ui.cancel}
              </button>
            </div>
          </section>
        </div>
      ) : null}

      {showNthSelectionDialog ? (
        <div className="overlayBackdrop">
          <section
            ref={nthSelectionDialogRef}
            className="dialogCard"
            role="dialog"
            aria-modal="true"
          >
            <div className="cardHeading">
              <h3>{ui.nthFrameDialogTitle}</h3>
            </div>
            <div className="durationDialogBody">
              <label className="field">
                <span className="metaLabel">{ui.nthFrameLabel}</span>
                <input
                  className="fieldInput"
                  type="number"
                  min={2}
                  step={1}
                  value={nthSelectionStep}
                  onChange={(event: ChangeEvent<HTMLInputElement>) =>
                    onNthSelectionStepChange(Math.max(2, Number(event.target.value) || 2))
                  }
                />
              </label>
            </div>
            <div className="dialogActionRow">
              <button type="button" className="primaryAction" onClick={onApplyNthFrameSelection}>
                {ui.confirm}
              </button>
              <button type="button" className="secondaryAction" onClick={onCloseNthSelectionDialog}>
                {ui.cancel}
              </button>
            </div>
          </section>
        </div>
      ) : null}
    </>
  );
}
