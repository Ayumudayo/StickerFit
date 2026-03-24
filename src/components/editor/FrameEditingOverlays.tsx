import type { ComponentProps } from "react";

import { FrameContextMenu } from "./FrameContextMenu";
import { FrameDialogs } from "./FrameDialogs";

type FrameEditingOverlaysProps = {
  frameContextMenuProps: ComponentProps<typeof FrameContextMenu> | null;
  frameDialogsProps: ComponentProps<typeof FrameDialogs>;
};

export function FrameEditingOverlays({
  frameContextMenuProps,
  frameDialogsProps,
}: FrameEditingOverlaysProps) {
  return (
    <>
      {frameContextMenuProps ? <FrameContextMenu {...frameContextMenuProps} /> : null}
      <FrameDialogs {...frameDialogsProps} />
    </>
  );
}
