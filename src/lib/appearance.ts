/** The renderer's light/dark preference. A per-device convenience kept in
 * this device's renderer storage; it never reaches the Host or a peer. */
export type Appearance = "system" | "light" | "dark";

const KEY = "pastey:appearance-v1";

export function readAppearance(): Appearance {
  try {
    const value = window.localStorage.getItem(KEY);
    return value === "light" || value === "dark" ? value : "system";
  } catch { return "system"; }
}

export function applyAppearance(value: Appearance = readAppearance()) {
  const root = document.documentElement;
  if (value === "system") delete root.dataset.theme;
  else root.dataset.theme = value;
}

export function saveAppearance(value: Appearance) {
  try { window.localStorage.setItem(KEY, value); } catch { /* still applied for this window */ }
  applyAppearance(value);
}
