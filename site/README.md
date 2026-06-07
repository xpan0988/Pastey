# Pastey product site

Static Astro product website for Pastey.

## Local development

```bash
cd site
pnpm install
pnpm dev
```

## Production build

```bash
pnpm build
```

The fully static output is written to `dist/`.

## Localized routes

- English: `/`, `/download`, `/architecture`
- Simplified Chinese: `/zh-CN/`, `/zh-CN/download`, `/zh-CN/architecture`

## Cloudflare Pages

- Root directory: `site`
- Build command: `pnpm build`
- Output directory: `dist`
- Environment variables: none
