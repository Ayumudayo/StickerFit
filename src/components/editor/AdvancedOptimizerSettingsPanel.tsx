import { useId } from "react";

import type { MessagesForLocale } from "../../locales/messages";
import type {
  OptimizerGoal,
  OptimizerSearchDepth,
} from "../../types/workflow";

import { ChevronDownIcon, SettingsIcon } from "../AppIcons";

type AdvancedOptimizerSettingsPanelProps = {
  copy: MessagesForLocale;
  layout?: "floating" | "dock";
  optimizerGoal: OptimizerGoal;
  qualityFrameDropInterval: number;
  optimizerSearchDepth: OptimizerSearchDepth;
  onOptimizerGoalChange: (value: OptimizerGoal) => void;
  onQualityFrameDropIntervalChange: (value: number) => void;
  onOptimizerSearchDepthChange: (value: OptimizerSearchDepth) => void;
};

export function AdvancedOptimizerSettingsPanel({
  copy,
  layout = "floating",
  optimizerGoal,
  qualityFrameDropInterval,
  optimizerSearchDepth,
  onOptimizerGoalChange,
  onQualityFrameDropIntervalChange,
  onOptimizerSearchDepthChange,
}: AdvancedOptimizerSettingsPanelProps) {
  const idPrefix = useId();
  const headingId = `${idPrefix}-heading`;
  const optimizerGoalId = `${idPrefix}-goal`;
  const frameDropId = `${idPrefix}-frame-drop`;
  const searchDepthId = `${idPrefix}-search-depth`;
  const panelClassName =
    layout === "dock"
      ? "appCard settingsCard advancedOptimizerPanel advancedOptimizerPanelInline"
      : "appCard settingsCard advancedOptimizerPanel";

  return (
    <section className={panelClassName} aria-labelledby={headingId}>
      <div className="cardHeading">
        <h3 id={headingId}>
          <SettingsIcon size={16} className="cardHeadingIcon" />
          {copy.advancedSettings}
        </h3>
      </div>

      <div className="advancedOptimizerGrid">
        <label className="field" htmlFor={optimizerGoalId}>
          <span className="metaLabel">{copy.optimizerGoal}</span>
          <div className="selectShell">
            <select
              id={optimizerGoalId}
              className="fieldSelect"
              value={optimizerGoal}
              onChange={(event) =>
                onOptimizerGoalChange(event.target.value as OptimizerGoal)
              }
            >
              <option value="balanced">{copy.optimizerGoalBalanced}</option>
              <option value="motion">{copy.optimizerGoalMotion}</option>
              <option value="quality">{copy.optimizerGoalQuality}</option>
            </select>
            <ChevronDownIcon size={16} className="selectChevron" />
          </div>
        </label>

        {optimizerGoal === "quality" ? (
          <label className="field" htmlFor={frameDropId}>
            <span className="metaLabel">{copy.qualityFrameDropInterval}</span>
            <div className="selectShell">
              <select
                id={frameDropId}
                className="fieldSelect"
                value={qualityFrameDropInterval}
                onChange={(event) =>
                  onQualityFrameDropIntervalChange(Number(event.target.value))
                }
              >
                <option value={0}>{copy.frameDropDisabled}</option>
                <option value={2}>{copy.frameDropEvery(2)}</option>
                <option value={3}>{copy.frameDropEvery(3)}</option>
                <option value={4}>{copy.frameDropEvery(4)}</option>
                <option value={5}>{copy.frameDropEvery(5)}</option>
              </select>
              <ChevronDownIcon size={16} className="selectChevron" />
            </div>
          </label>
        ) : null}

        <label className="field" htmlFor={searchDepthId}>
          <span className="metaLabel">{copy.advancedSearchDepth}</span>
          <div className="selectShell">
            <select
              id={searchDepthId}
              className="fieldSelect"
              value={optimizerSearchDepth}
              onChange={(event) =>
                onOptimizerSearchDepthChange(event.target.value as OptimizerSearchDepth)
              }
            >
              <option value="standard">{copy.searchDepthStandard}</option>
              <option value="thorough">{copy.searchDepthThorough}</option>
            </select>
            <ChevronDownIcon size={16} className="selectChevron" />
          </div>
        </label>
      </div>
      <p className="detailText estimateSettingsHint">{copy.estimateSettingsHint}</p>
    </section>
  );
}
