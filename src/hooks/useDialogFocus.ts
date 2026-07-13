import { type RefObject, useLayoutEffect, useRef } from "react";

const FOCUSABLE_SELECTOR = [
  "a[href]:not([tabindex='-1'])",
  "button:not(:disabled):not([tabindex='-1'])",
  "input:not(:disabled):not([type='hidden']):not([tabindex='-1'])",
  "select:not(:disabled):not([tabindex='-1'])",
  "textarea:not(:disabled):not([tabindex='-1'])",
  "[contenteditable='true']:not([tabindex='-1'])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

type UseDialogFocusOptions<T extends HTMLElement> = {
  isOpen: boolean;
  dialogRef: RefObject<T | null>;
  onClose: () => void;
  initialFocusRef?: RefObject<HTMLElement | null>;
};

function focusableElements(container: HTMLElement) {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) =>
      !element.hidden &&
      !element.closest("[hidden], [aria-hidden='true'], [inert]"),
  );
}

export function useDialogFocus<T extends HTMLElement>({
  isOpen,
  dialogRef,
  onClose,
  initialFocusRef,
}: UseDialogFocusOptions<T>) {
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    if (!isOpen) {
      return;
    }

    const dialog = dialogRef.current;
    if (!dialog) {
      return;
    }

    const activeElement = document.activeElement;
    const opener =
      activeElement instanceof HTMLElement && activeElement !== document.body
        ? activeElement
        : null;
    const initialFocus =
      initialFocusRef?.current ??
      dialog.querySelector<HTMLElement>("[data-dialog-initial-focus]") ??
      focusableElements(dialog)[0] ??
      dialog;

    initialFocus.focus({ preventScroll: true });

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.isComposing) {
        return;
      }

      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }

      if (event.key !== "Tab") {
        return;
      }

      const focusable = focusableElements(dialog);
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const current = document.activeElement;
      const currentIndex = current instanceof HTMLElement
        ? focusable.indexOf(current)
        : -1;

      if (event.shiftKey && currentIndex <= 0) {
        event.preventDefault();
        last.focus({ preventScroll: true });
      } else if (!event.shiftKey && (currentIndex === -1 || current === last)) {
        event.preventDefault();
        first.focus({ preventScroll: true });
      }
    };

    dialog.addEventListener("keydown", handleKeyDown);
    return () => {
      dialog.removeEventListener("keydown", handleKeyDown);
      if (opener?.isConnected) {
        opener.focus({ preventScroll: true });
      }
    };
  }, [dialogRef, initialFocusRef, isOpen]);
}
