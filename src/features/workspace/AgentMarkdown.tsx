/** An Agent's reply, read as the Markdown it was written in. The text comes
 * from an Agent, often on another device, so it is untrusted:
 * - raw HTML in it is shown as the text it is, never parsed (no rehype-raw);
 * - nothing in it loads anything: an image is a labelled placeholder, never
 *   a request to its URL;
 * - nothing in it navigates this window: a link is shown with where it goes
 *   and copies that address; only http, https and mailto addresses count as
 *   links at all;
 * - code is highlighted only when its fence names a language Pastey knows,
 *   from the text itself, never by guessing.
 * The original text is kept as it came, for Copy and for every protocol use;
 * this is only a way of reading it. */
import { memo, useState, type ReactNode } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { createLowlight } from "lowlight";
import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import markdown from "highlight.js/lib/languages/markdown";
import powershell from "highlight.js/lib/languages/powershell";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { copyTextToClipboard } from "../../lib/tauri";
import { Icon } from "./ui";

/** The languages coding Agents write in most. Each grammar also answers to
 * its usual aliases (ts, tsx, sh, zsh, py, rs, yml, toml, html, ps1, …). */
const lowlight = createLowlight({ bash, c, cpp, css, diff, go, ini, java, javascript, json, kotlin, markdown, powershell, python, rust, shell, sql, swift, typescript, xml, yaml });

/** Longer code is shown unhighlighted: reading it stays instant. */
const HIGHLIGHT_LIMIT = 20_000;

const SAFE_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);

/** The address a link or image may show, or "" when it is not one Pastey
 * shows as a link (javascript:, data:, file:, relative paths, anchors…). */
export function safeUrl(url: string): string {
  try {
    const parsed = new URL(url.trim());
    return SAFE_PROTOCOLS.has(parsed.protocol) ? parsed.href : "";
  } catch { return ""; }
}

/** Copies through the Host when Pastey runs as the app, and through the
 * browser in the browser preview. */
export async function copyText(text: string): Promise<void> {
  if (typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)) return copyTextToClipboard(text);
  await navigator.clipboard.writeText(text);
}

/** Copy, then "Copied" for a moment. */
export function CopyButton({ text, label = "Copy", className = "copy-button" }: { text: string; label?: string; className?: string }) {
  const [copied, setCopied] = useState(false);
  return <button type="button" className={className} aria-label={copied ? "Copied" : label} onClick={() => void copyText(text).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1500); }).catch(() => undefined)}>
    <Icon name={copied ? "check" : "copy"} small /><span>{copied ? "Copied" : label}</span>
  </button>;
}

type HastNode = { type: string; value?: string; tagName?: string; properties?: Record<string, unknown>; children?: HastNode[] };

function hastText(node: HastNode | undefined): string {
  if (!node) return "";
  if (node.type === "text") return node.value ?? "";
  return (node.children ?? []).map(hastText).join("");
}

function codeLanguage(node: HastNode | undefined): string | null {
  const classes = node?.properties?.className;
  const list = Array.isArray(classes) ? classes : typeof classes === "string" ? classes.split(" ") : [];
  const language = list.map(String).find((name) => name.startsWith("language-"));
  return language ? language.slice("language-".length).toLowerCase() : null;
}

function CodeBlock({ code, language }: { code: string; language: string | null }) {
  const known = language !== null && lowlight.registered(language);
  const body: ReactNode = known && code.length <= HIGHLIGHT_LIMIT
    ? toJsxRuntime(lowlight.highlight(language, code), { Fragment, jsx, jsxs })
    : code;
  return <div className="md-code">
    <div className="md-code-head">
      <span>{language ?? "Code"}</span>
      <CopyButton text={code} label="Copy code" />
    </div>
    <pre><code className={known ? `hljs language-${language}` : undefined}>{body}</code></pre>
  </div>;
}

/** A link from untrusted text: it never navigates this window. It shows the
 * address's host when the text says something else, and copies the address. */
function MarkdownLink({ href, node, children }: { href?: string; node?: HastNode; children?: ReactNode }) {
  const [copied, setCopied] = useState(false);
  if (!href) return <span className="md-link-text">{children}</span>;
  const text = hastText(node).trim();
  const host = href.startsWith("mailto:") ? null : new URL(href).host;
  const showsAddress = text === href || text === href.replace(/\/$/, "") || text === href.replace(/^mailto:/, "") || (host !== null && text === host);
  return <button type="button" className="md-link" title={`Copy link: ${href}`} onClick={() => void copyText(href).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1500); }).catch(() => undefined)}>
    {children}{!showsAddress && host ? <span className="md-link-host">({host})</span> : null}{copied ? <span className="md-link-copied" role="status">Link copied</span> : null}
  </button>;
}

const heading = (level: number) => function Heading({ children }: { children?: ReactNode }) {
  // Inside a conversation whose title is an h2: a reply's headings are
  // sections of one message, not of the page.
  return <p className={`md-h md-h${level}`} role="heading" aria-level={Math.min(6, level + 2)}>{children}</p>;
};

const COMPONENTS: Components = {
  a: ({ href, node, children }) => <MarkdownLink href={href} node={node as HastNode}>{children}</MarkdownLink>,
  img: ({ alt, src }) => <span className="md-image" title={src ? `Pastey does not load images from Agent replies: ${src}` : undefined}>[Image{alt ? `: ${alt}` : ""}]</span>,
  pre: ({ node }) => {
    const code = (node as HastNode | undefined)?.children?.find((child) => child.type === "element" && child.tagName === "code");
    return <CodeBlock code={hastText(code).replace(/\n$/, "")} language={codeLanguage(code)} />;
  },
  table: ({ children }) => <div className="md-table"><table>{children}</table></div>,
  h1: heading(1), h2: heading(2), h3: heading(3), h4: heading(4), h5: heading(5), h6: heading(6),
};

function transformUrl(url: string): string {
  return safeUrl(url);
}

/** One Agent reply as Markdown. Memoized: a long reply is parsed once, not
 * on every poll of the task it belongs to. */
export const AgentMarkdown = memo(function AgentMarkdown({ text }: { text: string }) {
  return <div className="md">
    <Markdown remarkPlugins={[remarkGfm]} components={COMPONENTS} urlTransform={transformUrl}>{text}</Markdown>
  </div>;
});
