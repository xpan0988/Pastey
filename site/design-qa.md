# Design QA

- Source visual tone: `/Users/xiyuanpan/Downloads/已生成图像 3.png`
- Implementation: `http://127.0.0.1:4321/`
- Compared widths: 1440px, 1280px, 864px, and 390px
- State: English and Simplified Chinese routes, dark theme

## Landing presentation

The landing page is an eight-slide horizontal product presentation on desktop. Each slide is one viewport wide and fills the space below the fixed header. The existing dark technical palette, compact navbar, explanatory product visuals, typography, buttons, cards, and restrained motion remain intact.

Desktop wheel input maps vertical movement into the horizontal deck. Native horizontal trackpad movement, arrow keys, navbar anchors, and the progress dots also move through the slides. The visible slide drives the active progress dot and relevant navbar state.

At 760px and below, the presentation becomes a vertical stack. This avoids cramped horizontal slides and preserves native mobile scrolling without body-level horizontal overflow.

## Visual logic

- Nearby Echo pulses originate from both device cards; the center confirms the connection.
- The nearby-discovery icon and rings share one geometric center.
- The binary visual shows sender and receiver devices, encrypted chunk movement, and returning ACK/control traffic.
- LAN utilization copy presents the observed 75–80% range as validation evidence rather than a guarantee.
- Inactive desktop slides pause their concept animations; `prefers-reduced-motion` reduces all animation and smooth scrolling.

## Copy

Simplified Chinese public copy uses `近距离即焚式传输`, `云端中转`, `格式无关`, `二进制传输路径`, `局域网原生`, `验证记录`, `当前限制`, and `非侵入式调度诊断` consistently where applicable.

## Verification

- `pnpm build`: passed through the equivalent local `npm run build` script; Astro reported 0 errors, 0 warnings, and generated all 6 routes.
- Routes checked: `/`, `/download`, `/architecture`, `/zh-CN/`, `/zh-CN/download`, `/zh-CN/architecture`.
- Canonical links, hreflang pairs, and language-switch route preservation: passed.
- Desktop slide dimensions and no body overflow at 1440px, 1280px, and 864px: passed.
- Mobile vertical fallback and no body overflow at 390px: passed.
- Keyboard navigation, wheel-driven horizontal movement, hash targets, progress state, and fixed header: passed.
- Browser console: no errors.

## Known limitation

The 390px mobile layout intentionally uses vertical stacking instead of horizontal slide swiping.

final result: passed
