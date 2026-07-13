import {
  Children,
  createElement,
  type ReactElement,
  type ReactNode,
} from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { MESSAGES } from "../../locales/messages";
import appSource from "../../App.tsx?raw";
import advancedDetailsSource from "../AdvancedDetailsPanel.tsx?raw";
import controllerSource from "../../hooks/useMediaWorkflowController.ts?raw";
import estimateCardSource from "./OutputSizeEstimateCard.tsx?raw";
import resultsOverlaySource from "./EditorResultsOverlay.tsx?raw";
import resultsStackSource from "./ResultsStack.tsx?raw";
import { PreviewUtilityActions } from "./PreviewUtilityActions";

type ButtonProps = {
  children: ReactNode;
  disabled?: boolean;
  "aria-label"?: string;
  onClick: () => void;
};

function optimizerButton(
  props: Parameters<typeof PreviewUtilityActions>[0],
): ReactElement<ButtonProps> {
  const root = PreviewUtilityActions(props) as ReactElement<{
    children: ReactNode;
  }>;
  const children = Children.toArray(root.props.children);
  const primaryStack = children[1] as ReactElement<{ children: ReactNode }>;
  const stackChildren = Children.toArray(primaryStack.props.children);
  return stackChildren[stackChildren.length - 1] as ReactElement<ButtonProps>;
}

function props(
  overrides: Partial<Parameters<typeof PreviewUtilityActions>[0]> = {},
) {
  return {
    activeDockPanel: null,
    copy: MESSAGES.en,
    isStaticImage: false,
    canConvertToPng: false,
    conversionLoading: false,
    planLoading: false,
    searchLoading: false,
    timelineFrameCount: 3,
    supportsDesktopProcessing: true,
    hasSearchResult: false,
    estimateCard: null,
    operationProgress: null,
    operationCancelled: false,
    fallbackWarning: null,
    advancedSettingsPanelId: "settings",
    previewPanelId: "preview",
    resultsPanelId: "results",
    onTogglePreviewCandidates: vi.fn(),
    onToggleAdvancedSettings: vi.fn(),
    onToggleResults: vi.fn(),
    onRunOptimizer: vi.fn(),
    onCancelOptimizer: vi.fn(),
    onConvertToPng: vi.fn(),
    ...overrides,
  } satisfies Parameters<typeof PreviewUtilityActions>[0];
}

describe("PreviewUtilityActions optimizer primary action", () => {
  it("runs optimization while idle", () => {
    const onRunOptimizer = vi.fn();
    const onCancelOptimizer = vi.fn();
    const button = optimizerButton(
      props({ onRunOptimizer, onCancelOptimizer }),
    );

    button.props.onClick();

    expect(onRunOptimizer).toHaveBeenCalledOnce();
    expect(onCancelOptimizer).not.toHaveBeenCalled();
    expect(button.props.disabled).toBe(false);
  });

  it("stays enabled and becomes the localized cancel action while searching", () => {
    const onRunOptimizer = vi.fn();
    const onCancelOptimizer = vi.fn();
    const button = optimizerButton(
      props({ searchLoading: true, onRunOptimizer, onCancelOptimizer }),
    );

    button.props.onClick();

    expect(onCancelOptimizer).toHaveBeenCalledOnce();
    expect(onRunOptimizer).not.toHaveBeenCalled();
    expect(button.props.disabled).toBe(false);
    expect(renderToStaticMarkup(button)).toContain(MESSAGES.en.cancelOptimizer);
    expect(button.props["aria-label"]).toBe(MESSAGES.en.cancelOptimizer);
  });

  it.each([
    ["en", MESSAGES.en.cancelOptimizer],
    ["ko", MESSAGES.ko.cancelOptimizer],
  ] as const)(
    "uses the %s cancel label for visible and accessible copy",
    (_locale, label) => {
      const button = optimizerButton(
        props({
          copy: _locale === "en" ? MESSAGES.en : MESSAGES.ko,
          searchLoading: true,
        }),
      );

      expect(renderToStaticMarkup(button)).toContain(label);
      expect(button.props["aria-label"]).toBe(label);
    },
  );

  it("keeps an active cancel action enabled even if ordinary start prerequisites disappear", () => {
    const onCancelOptimizer = vi.fn();
    const button = optimizerButton(
      props({
        searchLoading: true,
        supportsDesktopProcessing: false,
        timelineFrameCount: 0,
        onCancelOptimizer,
      }),
    );

    expect(button.props.disabled).toBe(false);
    button.props.onClick();
    expect(onCancelOptimizer).toHaveBeenCalledOnce();
  });

  it("renders the estimate immediately before the primary action", () => {
    const markup = renderToStaticMarkup(
      PreviewUtilityActions(
        props({
          estimateCard: createElement(
            "section",
            { "data-estimate-card": "true" },
            "estimate",
          ),
        }),
      ),
    );

    expect(markup.indexOf("data-estimate-card")).toBeGreaterThan(-1);
    expect(markup.indexOf("data-estimate-card")).toBeLessThan(
      markup.indexOf(MESSAGES.en.runOptimizer),
    );
  });

  it("exposes determinate and indeterminate operation progress accessibly", () => {
    const determinate = renderToStaticMarkup(
      PreviewUtilityActions(
        props({
          searchLoading: true,
          operationProgress: {
            operationId: "search-1",
            stage: "encoding",
            completed: 2,
            total: 5,
            messageCode: "media-operation-encoding",
          },
        }),
      ),
    );
    expect(determinate).toContain('role="progressbar"');
    expect(determinate).toContain('aria-valuenow="2"');
    expect(determinate).toContain('aria-valuemax="5"');

    const indeterminate = renderToStaticMarkup(
      PreviewUtilityActions(
        props({
          searchLoading: true,
          operationProgress: {
            operationId: "search-2",
            stage: "queued",
            completed: 0,
            total: null,
            messageCode: "media-operation-queued",
          },
        }),
      ),
    );
    expect(indeterminate).toContain("operationProgressTrackIndeterminate");
    expect(indeterminate).not.toContain("aria-valuenow");
  });

  it("renders cancelled work as a neutral message instead of an alert", () => {
    const markup = renderToStaticMarkup(
      PreviewUtilityActions(props({ operationCancelled: true })),
    );

    expect(markup).toContain(MESSAGES.en.operationCancelled);
    expect(markup).not.toContain('role="alert"');
  });
});

describe("Task 16 UI source contracts", () => {
  it("shows exact probing only for a sampled near-limit estimate", () => {
    expect(estimateCardSource).toContain('estimate?.kind === "range"');
    expect(estimateCardSource).toContain(
      'classifyEstimate(estimate) === "near-limit"',
    );
    expect(estimateCardSource).toContain("copy.checkExactCandidateSize");
    expect(estimateCardSource).toContain("copy.exactProbeNoOutput");
  });

  it("uses the typed fallback reason and never tool detail as warning authority", () => {
    expect(appSource).toContain(
      'inspection?.fallbackReasonCode === "media-foundation-failed"',
    );
    const warningBlock = appSource.slice(
      appSource.indexOf("const fallbackWarning"),
      appSource.indexOf("const primaryEstimateCard"),
    );
    expect(warningBlock).not.toContain("toolDetail");
  });

  it("estimates from the full plan while keeping the display projection bounded", () => {
    expect(controllerSource).not.toContain(
      "candidates: result.candidates.slice(0, advancedPreviewCount)",
    );
    expect(appSource).toContain(
      "candidates: fullPlan.candidates.slice(0, ADVANCED_PREVIEW_COUNT)",
    );
    const estimateHook = appSource.slice(
      appSource.indexOf("const outputSizeEstimate = useOutputSizeEstimate"),
      appSource.indexOf("const currentEstimateState"),
    );
    expect(estimateHook).toContain("plan: fullPlan");
  });

  it("keeps estimate recovery visible in the candidate overlay", () => {
    expect(advancedDetailsSource).toContain("showSharedCandidateEstimateState");
    expect(advancedDetailsSource).toContain('estimateState.status === "error"');
    expect(advancedDetailsSource).toContain(
      "onRetryEstimate={onRetryEstimate}",
    );
    expect(advancedDetailsSource).toContain("estimateState={estimateState}");
  });

  it("keeps one keyed exact-probe action across loading transitions", () => {
    expect(estimateCardSource).toContain('key="exact-probe-action"');
    expect(estimateCardSource).toContain("probeActionCancels");
    expect(estimateCardSource).toContain("showProbeControls");
  });

  it("offers a truthful plan refresh action before estimating changed settings", () => {
    expect(estimateCardSource).toContain("planLoading");
    expect(estimateCardSource).toContain("onRequestPlan");
    expect(estimateCardSource).toContain("copy.estimateWaitingForPlan");
    expect(estimateCardSource).toContain("copy.buildPreview");
    expect(appSource).toContain("handleBuildPreviewCandidates");
  });

  it("renders actual bytes, elapsed time, warnings, and a representative failure", () => {
    for (const source of [resultsOverlaySource, resultsStackSource]) {
      expect(source).toContain("copy.actualOutputSize");
      expect(source).toContain("copy.elapsedTime");
      expect(source).toContain("warnings");
    }
    expect(resultsOverlaySource).toContain("copy.representativeError");
    expect(resultsOverlaySource).toContain("mediaOperationMessage(");
  });
});
