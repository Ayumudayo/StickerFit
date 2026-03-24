import type { RefObject } from "react";

import type { EditorText } from "../../locales/editorText";
import type { FrameContextMenuState } from "../../types/editor";

type FrameContextMenuProps = {
  ui: EditorText;
  frameContextMenu: FrameContextMenuState;
  frameContextMenuRef: RefObject<HTMLElement | null>;
  hasSingleFrameSelection: boolean;
  canDeleteUnselectedFrames: boolean;
  hasClipboardFrames: boolean;
  onOpenFrameDurationDialog: () => void;
  onSplitCurrentFrame: () => void;
  onSpeedUpFrames: () => void;
  onSlowDownFrames: () => void;
  onCopyFramesToStart: () => void;
  onMoveFramesToStart: () => void;
  onMoveFramesUp: () => void;
  onMoveFramesDown: () => void;
  onCopyFramesToEnd: () => void;
  onMoveFramesToEnd: () => void;
  onReverseFrames: () => void;
  onDeleteUnselectedFrames: () => void;
  onSelectAllFrames: () => void;
  onSelectOddFrames: () => void;
  onSelectEvenFrames: () => void;
  onOpenNthFrameDialog: () => void;
  onClearAllFrames: () => void;
  onInvertSelection: () => void;
  onRenumberFrames: () => void;
  onCopyFrames: () => void;
  onCutFrames: () => void;
  onPasteFramesAbove: () => void;
  onPasteFramesBelow: () => void;
};

export function FrameContextMenu({
  ui,
  frameContextMenu,
  frameContextMenuRef,
  hasSingleFrameSelection,
  canDeleteUnselectedFrames,
  hasClipboardFrames,
  onOpenFrameDurationDialog,
  onSplitCurrentFrame,
  onSpeedUpFrames,
  onSlowDownFrames,
  onCopyFramesToStart,
  onMoveFramesToStart,
  onMoveFramesUp,
  onMoveFramesDown,
  onCopyFramesToEnd,
  onMoveFramesToEnd,
  onReverseFrames,
  onDeleteUnselectedFrames,
  onSelectAllFrames,
  onSelectOddFrames,
  onSelectEvenFrames,
  onOpenNthFrameDialog,
  onClearAllFrames,
  onInvertSelection,
  onRenumberFrames,
  onCopyFrames,
  onCutFrames,
  onPasteFramesAbove,
  onPasteFramesBelow,
}: FrameContextMenuProps) {
  return (
    <div className="overlayBackdrop">
      <section
        ref={frameContextMenuRef}
        className="frameContextMenu"
        role="dialog"
        aria-modal="true"
        style={{ left: frameContextMenu.x, top: frameContextMenu.y }}
      >
        <div className="contextMenuGrid">
          <button type="button" className="contextMenuItem" onClick={onOpenFrameDurationDialog}>
            {ui.setFrameTime}
          </button>
          <button
            type="button"
            className="contextMenuItem"
            onClick={onSplitCurrentFrame}
            disabled={!hasSingleFrameSelection}
          >
            {ui.splitFrame}
          </button>
          <button type="button" className="contextMenuItem" onClick={onSpeedUpFrames}>
            {ui.speedUpFrames}
          </button>
          <button type="button" className="contextMenuItem" onClick={onSlowDownFrames}>
            {ui.slowDownFrames}
          </button>
        </div>
        <div className="contextMenuDivider" />
        <div className="contextMenuGrid">
          <button type="button" className="contextMenuItem" onClick={onMoveFramesUp}>
            {ui.moveFramesUp}
          </button>
          <button type="button" className="contextMenuItem" onClick={onMoveFramesDown}>
            {ui.moveFramesDown}
          </button>
          <button type="button" className="contextMenuItem" onClick={onMoveFramesToStart}>
            {ui.moveFramesToStart}
          </button>
          <button type="button" className="contextMenuItem" onClick={onMoveFramesToEnd}>
            {ui.moveFramesToEnd}
          </button>
          <button type="button" className="contextMenuItem" onClick={onCopyFramesToStart}>
            {ui.copyFramesToStart}
          </button>
          <button type="button" className="contextMenuItem" onClick={onCopyFramesToEnd}>
            {ui.copyFramesToEnd}
          </button>
          <button
            type="button"
            className="contextMenuItem contextMenuItemFull"
            onClick={onReverseFrames}
          >
            {ui.reverseFrames}
          </button>
        </div>
        <div className="contextMenuDivider" />
        <div className="contextMenuGrid">
          <button type="button" className="contextMenuItem" onClick={onSelectAllFrames}>
            {ui.selectAll}
          </button>
          <button type="button" className="contextMenuItem" onClick={onClearAllFrames}>
            {ui.clearAll}
          </button>
          <button type="button" className="contextMenuItem" onClick={onSelectOddFrames}>
            {ui.selectOddFrames}
          </button>
          <button type="button" className="contextMenuItem" onClick={onSelectEvenFrames}>
            {ui.selectEvenFrames}
          </button>
          <button type="button" className="contextMenuItem" onClick={onOpenNthFrameDialog}>
            {ui.selectNthFrames}
          </button>
          <button type="button" className="contextMenuItem" onClick={onInvertSelection}>
            {ui.invertSelection}
          </button>
          <button
            type="button"
            className="contextMenuItem contextMenuItemFull"
            onClick={onRenumberFrames}
          >
            {ui.renumberFrames}
          </button>
        </div>
        <div className="contextMenuDivider" />
        <div className="contextMenuGrid">
          <button type="button" className="contextMenuItem" onClick={onCopyFrames}>
            {ui.copyFrames}
          </button>
          <button type="button" className="contextMenuItem" onClick={onCutFrames}>
            {ui.cutFrames}
          </button>
          <button
            type="button"
            className="contextMenuItem"
            onClick={onPasteFramesAbove}
            disabled={!hasClipboardFrames}
          >
            {ui.pasteFramesAbove}
          </button>
          <button
            type="button"
            className="contextMenuItem"
            onClick={onPasteFramesBelow}
            disabled={!hasClipboardFrames}
          >
            {ui.pasteFramesBelow}
          </button>
        </div>
        <div className="contextMenuDivider" />
        <button
          type="button"
          className="contextMenuItem contextMenuItemFull dangerItem"
          onClick={onDeleteUnselectedFrames}
          disabled={!canDeleteUnselectedFrames}
        >
          {ui.deleteUnselectedFrames}
        </button>
      </section>
    </div>
  );
}
