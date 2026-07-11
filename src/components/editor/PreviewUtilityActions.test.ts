import { Children, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { MESSAGES } from "../../locales/messages";
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
  return children[1] as ReactElement<ButtonProps>;
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
    const button = optimizerButton(props({ onRunOptimizer, onCancelOptimizer }));

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
  ] as const)("uses the %s cancel label for visible and accessible copy", (_locale, label) => {
    const button = optimizerButton(
      props({
        copy: _locale === "en" ? MESSAGES.en : MESSAGES.ko,
        searchLoading: true,
      }),
    );

    expect(renderToStaticMarkup(button)).toContain(label);
    expect(button.props["aria-label"]).toBe(label);
  });

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
});
