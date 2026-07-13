export type EditorShortcutContext = {
  defaultPrevented: boolean;
  isComposing: boolean;
  hasModifier: boolean;
  dialogOpen: boolean;
  insideEditorSurface: boolean;
  insideInteractiveSurface: boolean;
};

export function shouldHandleEditorShortcut(context: EditorShortcutContext) {
  return (
    !context.defaultPrevented &&
    !context.isComposing &&
    !context.hasModifier &&
    !context.dialogOpen &&
    context.insideEditorSurface &&
    !context.insideInteractiveSurface
  );
}
