export type KeyboardActivationEvent = {
  key: string;
  preventDefault: () => void;
};

/**
 * Give custom interactive rows the same Enter/Space activation contract as a
 * native button. Callers still need role="button" and tabIndex={0} so the row
 * is exposed to assistive technology and keyboard focus.
 */
export function activateOnKeyboard(
  event: KeyboardActivationEvent,
  action: () => void,
): void {
  if (event.key !== "Enter" && event.key !== " ") return;
  event.preventDefault();
  action();
}
