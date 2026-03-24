import type { CSSProperties, ComponentProps, RefObject } from "react";

import { MediaSelectionPreview } from "../MediaSelectionPreview";
import type { MediaInspection } from "../../types/workflow";
import { EditorOverlayPanel } from "./EditorOverlayPanel";
import { FrameRail } from "./FrameRail";
import { PreviewControlBar } from "./PreviewControlBar";
import { PreviewInfoBar } from "./PreviewInfoBar";
import { PreviewUtilityActions } from "./PreviewUtilityActions";
import { ResultsStack } from "./ResultsStack";

type EditorWorkspaceProps = {
  editorWorkspaceRef: RefObject<HTMLElement | null>;
  editorWorkspaceStyle?: CSSProperties;
  inspection: MediaInspection;
  previewKey: string;
  isWebPreviewMode: boolean;
  webPreviewNotice: string;
  plannerError: string | null;
  frameRailProps: ComponentProps<typeof FrameRail>;
  previewInfoBarProps: ComponentProps<typeof PreviewInfoBar>;
  previewProps: ComponentProps<typeof MediaSelectionPreview>;
  overlayPanelProps: ComponentProps<typeof EditorOverlayPanel>;
  previewControlBarProps: ComponentProps<typeof PreviewControlBar>;
  previewUtilityActionsProps: ComponentProps<typeof PreviewUtilityActions>;
  staticImageResultsProps: ComponentProps<typeof ResultsStack> | null;
};

export function EditorWorkspace({
  editorWorkspaceRef,
  editorWorkspaceStyle,
  inspection,
  previewKey,
  isWebPreviewMode,
  webPreviewNotice,
  plannerError,
  frameRailProps,
  previewInfoBarProps,
  previewProps,
  overlayPanelProps,
  previewControlBarProps,
  previewUtilityActionsProps,
  staticImageResultsProps,
}: EditorWorkspaceProps) {
  return (
    <>
      <section
        ref={editorWorkspaceRef}
        className={inspection.isStaticImage ? "editorWorkspace editorWorkspaceStatic" : "editorWorkspace"}
        style={editorWorkspaceStyle}
      >
        {!inspection.isStaticImage ? <FrameRail {...frameRailProps} /> : null}

        <div className="stageColumn">
          <div className="previewWorkspaceColumn">
            <PreviewInfoBar {...previewInfoBarProps} />

            <MediaSelectionPreview
              key={previewKey}
              {...previewProps}
            />

            <EditorOverlayPanel {...overlayPanelProps} />
          </div>

          <div className="previewBottomDock">
            <PreviewControlBar {...previewControlBarProps} />
            <PreviewUtilityActions {...previewUtilityActionsProps} />

            {isWebPreviewMode ? (
              <section className="noticeCard previewInlineMessage" role="status" aria-live="polite">
                <p>{webPreviewNotice}</p>
              </section>
            ) : null}

            {plannerError ? (
              <section className="errorCard compactState previewInlineMessage" role="alert" aria-live="assertive">
                <p>{plannerError}</p>
              </section>
            ) : null}
          </div>
        </div>
      </section>

      {inspection.isStaticImage && staticImageResultsProps ? (
        <ResultsStack {...staticImageResultsProps} />
      ) : null}
    </>
  );
}
