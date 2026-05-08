import type { MessagesForLocale } from "../../locales/messages";
import type {
  OptimizerGoal,
  OptimizerSearchDepth,
} from "../../types/workflow";

import { ChevronDownIcon, SettingsIcon } from "../AppIcons";

type AdvancedOptimizerSettingsPanelProps = {
  panelId: string;
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
  panelId,
  copy,
  layout = "floating",
  optimizerGoal,
  qualityFrameDropInterval,
  optimizerSearchDepth,
  onOptimizerGoalChange,
  onQualityFrameDropIntervalChange,
  onOptimizerSearchDepthChange,
}: AdvancedOptimizerSettingsPanelProps) {
  const panelClassName =
    layout === "dock"
      ? "appCard settingsCard advancedOptimizerPanel advancedOptimizerPanelInline"
      : "appCard settingsCard advancedOptimizerPanel";

  return (
    <section id={panelId} className={panelClassName}>
      <div className="cardHeading">
        <h3>
          <SettingsIcon size={16} className="cardHeadingIcon" />
          {copy.advancedSettings}
        </h3>
      </div>

      <div className="advancedOptimizerGrid">
        <label className="field">
          <span className="metaLabel">{copy.optimizerGoal}</span>
          <div className="selectShell">
            <select
              className="fitModeSelect"
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
          <label className="field">
            <span className="metaLabel">{copy.qualityFrameDropInterval}</span>
            <div className="selectShell">
              <select
                className="fitModeSelect"
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

        <label className="field">
          <span className="metaLabel">{copy.advancedSearchDepth}</span>
          <div className="selectShell">
            <select
              className="fitModeSelect"
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
    </section>
  );
}
