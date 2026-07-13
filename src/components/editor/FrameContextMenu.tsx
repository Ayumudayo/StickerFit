import {
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import type { EditorText } from "../../locales/editorText";
import type { FrameContextMenuState } from "../../types/editor";

type FrameContextMenuProps = {
  ui: EditorText;
  frameContextMenu: FrameContextMenuState;
  frameContextMenuRef: RefObject<HTMLElement | null>;
  hasSingleFrameSelection: boolean;
  canDeleteUnselectedFrames: boolean;
  hasClipboardFrames: boolean;
  onClose: () => void;
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

type ContextMenuItem = {
  label: string;
  onSelect: () => void;
  disabled?: boolean;
  fullWidth?: boolean;
  danger?: boolean;
};

function findAnchorFrameElement(instanceId: string) {
  return (
    Array.from(document.querySelectorAll<HTMLElement>("[data-instance-id]")).find(
      (element) => element.dataset.instanceId === instanceId,
    ) ?? null
  );
}

export function FrameContextMenu({
  ui,
  frameContextMenu,
  frameContextMenuRef,
  hasSingleFrameSelection,
  canDeleteUnselectedFrames,
  hasClipboardFrames,
  onClose,
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
  const sections: ContextMenuItem[][] = [
    [
      { label: ui.setFrameTime, onSelect: onOpenFrameDurationDialog },
      {
        label: ui.splitFrame,
        onSelect: onSplitCurrentFrame,
        disabled: !hasSingleFrameSelection,
      },
      { label: ui.speedUpFrames, onSelect: onSpeedUpFrames },
      { label: ui.slowDownFrames, onSelect: onSlowDownFrames },
    ],
    [
      { label: ui.moveFramesUp, onSelect: onMoveFramesUp },
      { label: ui.moveFramesDown, onSelect: onMoveFramesDown },
      { label: ui.moveFramesToStart, onSelect: onMoveFramesToStart },
      { label: ui.moveFramesToEnd, onSelect: onMoveFramesToEnd },
      { label: ui.copyFramesToStart, onSelect: onCopyFramesToStart },
      { label: ui.copyFramesToEnd, onSelect: onCopyFramesToEnd },
      { label: ui.reverseFrames, onSelect: onReverseFrames, fullWidth: true },
    ],
    [
      { label: ui.selectAll, onSelect: onSelectAllFrames },
      { label: ui.clearAll, onSelect: onClearAllFrames },
      { label: ui.selectOddFrames, onSelect: onSelectOddFrames },
      { label: ui.selectEvenFrames, onSelect: onSelectEvenFrames },
      { label: ui.selectNthFrames, onSelect: onOpenNthFrameDialog },
      { label: ui.invertSelection, onSelect: onInvertSelection },
      { label: ui.renumberFrames, onSelect: onRenumberFrames, fullWidth: true },
    ],
    [
      { label: ui.copyFrames, onSelect: onCopyFrames },
      { label: ui.cutFrames, onSelect: onCutFrames },
      {
        label: ui.pasteFramesAbove,
        onSelect: onPasteFramesAbove,
        disabled: !hasClipboardFrames,
      },
      {
        label: ui.pasteFramesBelow,
        onSelect: onPasteFramesBelow,
        disabled: !hasClipboardFrames,
      },
    ],
    [
      {
        label: ui.deleteUnselectedFrames,
        onSelect: onDeleteUnselectedFrames,
        disabled: !canDeleteUnselectedFrames,
        fullWidth: true,
        danger: true,
      },
    ],
  ];
  const items = sections.flat();
  const enabledIndices = items.flatMap((item, index) => (item.disabled ? [] : [index]));
  const enabledIndicesKey = enabledIndices.join(":");
  const [activeIndex, setActiveIndex] = useState(() => enabledIndices[0] ?? -1);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const openerRef = useRef<HTMLElement | null>(null);
  const rovingIndex = enabledIndices.includes(activeIndex)
    ? activeIndex
    : enabledIndices[0] ?? -1;

  useLayoutEffect(() => {
    const menu = frameContextMenuRef.current;
    const activeElement = document.activeElement;
    openerRef.current =
      findAnchorFrameElement(frameContextMenu.anchorInstanceId) ??
      (activeElement instanceof HTMLElement && !menu?.contains(activeElement)
        ? activeElement
        : null);

    return () => {
      const opener = openerRef.current;
      if (opener?.isConnected) {
        opener.focus({ preventScroll: true });
      }
      window.requestAnimationFrame(() => {
        const activeElementAfterCommit = document.activeElement;
        if (
          activeElementAfterCommit instanceof HTMLElement &&
          activeElementAfterCommit !== document.body &&
          activeElementAfterCommit !== document.documentElement
        ) {
          return;
        }

        const fallback =
          findAnchorFrameElement(frameContextMenu.anchorInstanceId) ??
          document.querySelector<HTMLElement>("[role='option'][tabindex='0']") ??
          document.querySelector<HTMLElement>(
            ".frameRailEmptyState button:not(:disabled)",
          ) ??
          document.querySelector<HTMLElement>("[data-editor-shortcut-surface]");
        fallback?.focus({ preventScroll: true });
      });
    };
  }, [frameContextMenu.anchorInstanceId, frameContextMenuRef]);

  useLayoutEffect(() => {
    const firstEnabledIndex = enabledIndices[0] ?? -1;
    setActiveIndex(firstEnabledIndex);
    if (firstEnabledIndex >= 0) {
      itemRefs.current[firstEnabledIndex]?.focus({ preventScroll: true });
    } else {
      frameContextMenuRef.current?.focus({ preventScroll: true });
    }
  }, [enabledIndicesKey, frameContextMenuRef]);

  const focusItem = (index: number) => {
    setActiveIndex(index);
    itemRefs.current[index]?.focus({ preventScroll: true });
  };

  const focusRelativeItem = (direction: -1 | 1) => {
    if (enabledIndices.length === 0) {
      return;
    }
    const currentPosition = Math.max(0, enabledIndices.indexOf(rovingIndex));
    const nextPosition =
      (currentPosition + direction + enabledIndices.length) % enabledIndices.length;
    const nextIndex = enabledIndices[nextPosition];
    if (nextIndex !== undefined) {
      focusItem(nextIndex);
    }
  };

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    switch (event.key) {
      case "ArrowDown":
      case "ArrowRight":
        event.preventDefault();
        focusRelativeItem(1);
        break;
      case "ArrowUp":
      case "ArrowLeft":
        event.preventDefault();
        focusRelativeItem(-1);
        break;
      case "Home":
        event.preventDefault();
        if (enabledIndices[0] !== undefined) {
          focusItem(enabledIndices[0]);
        }
        break;
      case "End": {
        event.preventDefault();
        const lastIndex = enabledIndices[enabledIndices.length - 1];
        if (lastIndex !== undefined) {
          focusItem(lastIndex);
        }
        break;
      }
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        onClose();
        break;
      case "Tab":
        event.preventDefault();
        event.stopPropagation();
        onClose();
        break;
    }
  };

  let sectionOffset = 0;
  return (
    <div
      className="overlayBackdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <section
        ref={frameContextMenuRef}
        className="frameContextMenu"
        role="menu"
        aria-label={ui.frameTitle}
        aria-orientation="vertical"
        tabIndex={-1}
        style={{ left: frameContextMenu.x, top: frameContextMenu.y }}
        onKeyDown={handleMenuKeyDown}
      >
        {sections.map((section, sectionIndex) => {
          const startIndex = sectionOffset;
          sectionOffset += section.length;
          return (
            <div role="presentation" key={sectionIndex}>
              {sectionIndex > 0 ? <div className="contextMenuDivider" role="separator" /> : null}
              <div className="contextMenuGrid" role="presentation">
                {section.map((item, itemIndex) => {
                  const index = startIndex + itemIndex;
                  const className = [
                    "contextMenuItem",
                    item.fullWidth ? "contextMenuItemFull" : "",
                    item.danger ? "dangerItem" : "",
                  ]
                    .filter(Boolean)
                    .join(" ");
                  return (
                    <button
                      key={item.label}
                      ref={(element) => {
                        itemRefs.current[index] = element;
                      }}
                      type="button"
                      role="menuitem"
                      className={className}
                      disabled={item.disabled}
                      tabIndex={index === rovingIndex ? 0 : -1}
                      onFocus={() => setActiveIndex(index)}
                      onClick={() => {
                        item.onSelect();
                        onClose();
                      }}
                    >
                      {item.label}
                    </button>
                  );
                })}
              </div>
            </div>
          );
        })}
      </section>
    </div>
  );
}
