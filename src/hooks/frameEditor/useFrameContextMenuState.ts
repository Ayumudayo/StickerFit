import { type MouseEvent as ReactMouseEvent, useCallback, useLayoutEffect, useRef, useState } from "react";

import type { FrameContextMenuState } from "../../types/editor";

type UseFrameContextMenuStateParams = {
  selectedInstanceIdSet: ReadonlySet<string>;
  setSelectedInstanceIds: (value: string[]) => void;
};

export function useFrameContextMenuState({
  selectedInstanceIdSet,
  setSelectedInstanceIds,
}: UseFrameContextMenuStateParams) {
  const [frameContextMenu, setFrameContextMenu] = useState<FrameContextMenuState | null>(null);
  const frameContextMenuRef = useRef<HTMLElement | null>(null);

  const closeFrameContextMenu = useCallback(() => {
    setFrameContextMenu(null);
  }, []);

  const handleFrameContextMenu = useCallback(
    (instanceId: string, event: ReactMouseEvent<HTMLButtonElement>) => {
      event.preventDefault();

      if (!selectedInstanceIdSet.has(instanceId)) {
        setSelectedInstanceIds([instanceId]);
      }

      setFrameContextMenu({
        x: event.clientX,
        y: event.clientY,
        anchorInstanceId: instanceId,
      });
    },
    [selectedInstanceIdSet, setSelectedInstanceIds],
  );

  useLayoutEffect(() => {
    if (!frameContextMenu || !frameContextMenuRef.current) {
      return;
    }

    const margin = 12;
    const rect = frameContextMenuRef.current.getBoundingClientRect();
    let nextX = frameContextMenu.x;
    let nextY = frameContextMenu.y;

    if (rect.right > window.innerWidth - margin) {
      nextX -= rect.right - (window.innerWidth - margin);
    }
    if (rect.bottom > window.innerHeight - margin) {
      nextY -= rect.bottom - (window.innerHeight - margin);
    }

    nextX = Math.max(margin, nextX);
    nextY = Math.max(margin, nextY);

    if (nextX !== frameContextMenu.x || nextY !== frameContextMenu.y) {
      setFrameContextMenu((current) =>
        current
          ? {
              ...current,
              x: nextX,
              y: nextY,
            }
          : current,
      );
    }
  }, [frameContextMenu]);

  return {
    frameContextMenu,
    frameContextMenuRef,
    setFrameContextMenu,
    closeFrameContextMenu,
    handleFrameContextMenu,
  };
}
