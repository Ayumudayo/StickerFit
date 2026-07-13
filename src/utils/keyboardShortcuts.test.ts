import { describe, expect, it } from "vitest";

import {
  shouldHandleEditorShortcut,
  type EditorShortcutContext,
} from "./keyboardShortcuts";

const editorBackgroundContext: EditorShortcutContext = {
  defaultPrevented: false,
  isComposing: false,
  hasModifier: false,
  dialogOpen: false,
  insideEditorSurface: true,
  insideInteractiveSurface: false,
};

describe("shouldHandleEditorShortcut", () => {
  it("handles an unmodified shortcut from the editor background", () => {
    expect(shouldHandleEditorShortcut(editorBackgroundContext)).toBe(true);
  });

  it.each([
    ["a previously handled event", { defaultPrevented: true }],
    ["IME composition", { isComposing: true }],
    ["a Ctrl, Alt, or Meta modifier", { hasModifier: true }],
    ["an open dialog", { dialogOpen: true }],
    ["a target outside the editor", { insideEditorSurface: false }],
    ["an interactive target", { insideInteractiveSurface: true }],
  ] satisfies [string, Partial<EditorShortcutContext>][])(
    "ignores %s",
    (_description, override) => {
      expect(
        shouldHandleEditorShortcut({
          ...editorBackgroundContext,
          ...override,
        }),
      ).toBe(false);
    },
  );

  it("keeps every gate closed even when another gate would otherwise allow handling", () => {
    expect(
      shouldHandleEditorShortcut({
        defaultPrevented: true,
        isComposing: true,
        hasModifier: true,
        dialogOpen: true,
        insideEditorSurface: false,
        insideInteractiveSurface: true,
      }),
    ).toBe(false);
  });
});
