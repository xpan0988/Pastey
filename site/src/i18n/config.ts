export const locales = ["en", "zh-CN"] as const;
export type Locale = (typeof locales)[number];

export const defaultLocale: Locale = "en";

export function localePath(locale: Locale, path = "/") {
  const normalized = path === "/" ? "" : `/${path.replace(/^\/|\/$/g, "")}`;
  return locale === defaultLocale ? normalized || "/" : `/zh-CN${normalized || "/"}`;
}

function normalizePath(pathname: string) {
  return pathname === "/" ? pathname : pathname.replace(/\/+$/, "");
}

export function alternateLocalePath(locale: Locale, pathname: string) {
  const normalizedPath = normalizePath(pathname);

  if (locale === "zh-CN") {
    return normalizePath(normalizedPath.replace(/^\/zh-CN(?=\/|$)/, "") || "/");
  }
  return normalizedPath === "/" ? "/zh-CN/" : `/zh-CN${normalizedPath}`;
}
