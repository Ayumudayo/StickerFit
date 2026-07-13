import {
  Children,
  createElement,
  createRef,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { editorText } from "../../locales/editorText";
import type { Locale } from "../../locales/messages";
import type { TimelineFrameView } from "../../types/editor";
import type { FramePreviewLoadState } from "../../types/workflow";
import { FramePreviewRetryLayer, FrameRail } from "./FrameRail";

const frames: TimelineFrameView[] = [
  {
    instanceId: "frame-1",
    sourceFrameId: 7,
    displayNumber: 1,
    durationUs: 100_000,
    durationSeconds: 0.1,
    startTimeUs: 0,
    startTimeSeconds: 0,
    sourceStartTimeSeconds: 0,
  },
  {
    instanceId: "frame-2",
    sourceFrameId: 8,
    displayNumber: 2,
    durationUs: 100_000,
    durationSeconds: 0.1,
    startTimeUs: 100_000,
    startTimeSeconds: 0.1,
    sourceStartTimeSeconds: 0.1,
  },
];

const previewEntries = new Map<number, FramePreviewLoadState>([
  [7, { status: "error", message: "preview failed" }],
  [8, { status: "error", message: "preview failed" }],
]);

function railProps(locale: Locale) {
  return {
    ui: editorText(locale),
    locale,
    timelineFrameViews: frames,
    selection: {
      selectedInstanceIdSet: new Set(["frame-1"]),
      selectedVisibleCount: 1,
      hasSelectedFrames: true,
    },
    hasClipboardFrames: false,
    frameDropTarget: null,
    frameReorderState: null,
    frameTableBodyRef: createRef<HTMLDivElement>(),
    activeInstanceId: "frame-1",
    showFramePreviews: true,
    previewEntries,
    onVisibleRange: vi.fn(),
    onRetryFramePreview: vi.fn(),
    onFramePointerDown: vi.fn(),
    onFrameContextMenu: vi.fn(),
    onFrameKeyDown: vi.fn(),
    onFrameFocus: vi.fn(),
    onPasteFramesBelow: vi.fn(),
  } satisfies Parameters<typeof FrameRail>[0];
}

describe("FrameRail preview retry accessibility", () => {
  it("keeps retry controls outside the listbox and names every frame", () => {
    const markup = renderToStaticMarkup(
      createElement(FrameRail, railProps("en")),
    );
    const listboxStart = markup.indexOf('role="listbox"');
    const retryLayerStart = markup.indexOf(
      '<div class="framePreviewRetryLayer">',
    );

    expect(listboxStart).toBeGreaterThan(-1);
    expect(retryLayerStart).toBeGreaterThan(listboxStart);
    expect(markup.slice(listboxStart, retryLayerStart)).not.toContain(
      "framePreviewRetry",
    );
    expect(markup.slice(listboxStart, retryLayerStart)).toContain(
      'role="option"',
    );
    expect(markup.slice(retryLayerStart)).toContain(
      'aria-label="Retry preview for frame 1"',
    );
    expect(markup.slice(retryLayerStart)).toContain(
      'aria-label="Retry preview for frame 2"',
    );

    const koreanMarkup = renderToStaticMarkup(
      createElement(FrameRail, railProps("ko")),
    );
    expect(koreanMarkup).toContain(
      'aria-label="1번 프레임 미리보기 다시 시도"',
    );
  });

  it("uses a native button and forwards retry activation for its source frame", () => {
    const onRetryFramePreview = vi.fn();
    const onRestoreFrameFocus = vi.fn();
    const layer = FramePreviewRetryLayer({
      ui: editorText("en"),
      visibleFrames: frames,
      previewEntries,
      virtualOffsetTop: 0,
      scrollTop: 0,
      viewportHeight: 384,
      onRetryFramePreview,
      onRestoreFrameFocus,
    }) as ReactElement<{ children: ReactNode }>;
    const button = Children.toArray(layer.props.children)[0] as ReactElement<{
      type: "button";
      "aria-label": string;
      onClick: (event: MouseEvent<HTMLButtonElement>) => void;
    }>;
    const stopPropagation = vi.fn();

    expect(button.props.type).toBe("button");
    expect(button.props["aria-label"]).toBe("Retry preview for frame 1");
    button.props.onClick({
      stopPropagation,
    } as unknown as MouseEvent<HTMLButtonElement>);

    expect(stopPropagation).toHaveBeenCalledOnce();
    expect(onRetryFramePreview).toHaveBeenCalledExactlyOnceWith(7);
    expect(onRestoreFrameFocus).toHaveBeenCalledExactlyOnceWith("frame-1");
  });

  it("does not expose overscan retry controls outside the visible viewport", () => {
    const markup = renderToStaticMarkup(
      createElement(FramePreviewRetryLayer, {
        ui: editorText("en"),
        visibleFrames: frames,
        previewEntries,
        virtualOffsetTop: 0,
        scrollTop: 48,
        viewportHeight: 48,
        onRetryFramePreview: vi.fn(),
        onRestoreFrameFocus: vi.fn(),
      }),
    );

    expect(markup).not.toContain("Retry preview for frame 1");
    expect(markup).toContain("Retry preview for frame 2");
  });
});
