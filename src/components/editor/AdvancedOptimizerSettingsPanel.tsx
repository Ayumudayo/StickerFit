import type { MessagesForLocale } from "../../locales/messages";
import type {
  OptimizerPresetStrategy,
  OptimizerSearchDepth,
} from "../../types/workflow";

import { ChevronDownIcon, SettingsIcon } from "../AppIcons";

type AdvancedOptimizerSettingsPanelProps = {
  panelId: string;
  copy: MessagesForLocale;
  layout?: "floating" | "dock";
  optimizerPresetStrategy: OptimizerPresetStrategy;
  optimizerSearchDepth: OptimizerSearchDepth;
  onOptimizerPresetStrategyChange: (value: OptimizerPresetStrategy) => void;
  onOptimizerSearchDepthChange: (value: OptimizerSearchDepth) => void;
};

export function AdvancedOptimizerSettingsPanel({
  panelId,
  copy,
  layout = "floating",
  optimizerPresetStrategy,
  optimizerSearchDepth,
  onOptimizerPresetStrategyChange,
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
          <span className="metaLabel">{copy.advancedOptimizeFocus}</span>
          <div className="selectShell">
            <select
              className="fitModeSelect"
              value={optimizerPresetStrategy}
              onChange={(event) =>
                onOptimizerPresetStrategyChange(event.target.value as OptimizerPresetStrategy)
              }
            >
              <option value="auto">{copy.optimizeFocusAuto}</option>
              <option value="quality">{copy.optimizeFocusQuality}</option>
              <option value="size">{copy.optimizeFocusSize}</option>
            </select>
            <ChevronDownIcon size={16} className="selectChevron" />
          </div>
        </label>

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
