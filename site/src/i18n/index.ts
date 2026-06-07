import { en } from "./en";
import { zhCN } from "./zh-CN";
import type { Locale } from "./config";

export type Dictionary = typeof en | typeof zhCN;

export function getDictionary(locale: Locale): Dictionary {
  return locale === "zh-CN" ? zhCN : en;
}
