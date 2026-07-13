# Pastey product site

Static Astro + TypeScript + Tailwind CSS product website for Pastey. It is deployable to Cloudflare Pages without a server or runtime environment variables.

The website presents Pastey as an ephemeral local handoff tool with a format-agnostic binary transfer path, LAN-native speed, no cloud relay, no remote storage, and no storage buildup by design. Managed Bridge session state remains local and is cleared when the session is burned; Inbox-saved output remains user-owned.

## Landing experience

The desktop landing page is an eight-slide horizontal product presentation with a fixed header, wheel/trackpad cooldown, keyboard navigation, direct navbar/hash and progress-dot navigation, staged slide-entry reveals, and reduced-motion support.

At 760px and below, the landing page uses native vertical stacking to avoid cramped slides and horizontal overflow. `/download` and `/architecture` remain normal pages.

## Local development

```bash
cd site
pnpm install
pnpm dev
```

If pnpm is unavailable, use the equivalent npm commands:

```bash
cd site
npm install
npm run dev
```

## Production build

```bash
pnpm build
```

Alternative:

```bash
npm run build
```

The fully static output is written to `dist/`.

Preview the production build with `pnpm preview` or `npm run preview`.

## Localized routes

- English: `/`, `/download`, `/architecture`
- Simplified Chinese: `/zh-CN/`, `/zh-CN/download`, `/zh-CN/architecture`

Canonical links, hreflang metadata, and the language switcher preserve the corresponding route.

## Downloads

- Latest release: <https://github.com/xpan0988/Pastey/releases/latest>
- All releases: <https://github.com/xpan0988/Pastey/releases>

## Cloudflare Pages

- Root directory: `site`
- Build command: `pnpm build` when pnpm is available; alternative `npm run build`
- Output directory: `dist`
- Environment variables: none

## Public claims

- Pastey uses a format-agnostic binary-v1 transfer path for file-like payloads; JSON/base64 remains a compatibility fallback.
- Pastey avoids cloud relay and remote storage.
- Pastey is built for LAN-native speed and to use most of the available LAN.
- Current validation has observed roughly 75–80% of a practical LAN ceiling. This is an observed range, not a guarantee.
- Actual throughput depends on Wi-Fi/Ethernet quality, device I/O, CPU, system load, and runtime conditions.

See [`../docs/layers/layer-1-transfer.md`](../docs/layers/layer-1-transfer.md) for current implementation boundaries and [`../docs/development.md`](../docs/development.md) for validation layers.

## Design QA

Current landing interaction and responsive validation notes live in [`design-qa.md`](design-qa.md).
