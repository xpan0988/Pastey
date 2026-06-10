# Design QA

- Source visual tone: `/Users/xiyuanpan/Downloads/已生成图像 3.png`
- Implementation: `http://127.0.0.1:4321/`
- Compared widths: 1440px, 1280px, 864px, and 390px
- State: English and Simplified Chinese routes, dark theme

## Landing presentation

The landing page is an eight-slide horizontal product presentation on desktop. Each slide is one viewport wide and fills the space below the fixed header. The existing dark technical palette, compact navbar, explanatory product visuals, typography, buttons, cards, and restrained motion remain intact.

Desktop wheel input maps vertical movement into the horizontal deck. Native horizontal trackpad movement, arrow keys, navbar anchors, and the progress dots also move through the slides. The visible slide drives the active progress dot and relevant navbar state.

Vertical wheel navigation uses a 40px delta threshold, a 500ms slide-change cooldown, and a 320ms quiet-period gesture latch. A continuous inertial tail is treated as one gesture instead of repeatedly advancing the deck. Keyboard navigation ignores held-key repeats and uses a 325ms cooldown. Navbar/hash and progress-dot navigation remain direct.

At 760px and below, the presentation becomes a vertical stack. This avoids cramped horizontal slides and preserves native mobile scrolling without body-level horizontal overflow.

## Staged reveal

Each slide separates its important content into grouped reveal layers: eyebrow, heading, copy, actions, visual, and inspection cards. Inactive layers reset to zero opacity and a restrained 24–28px vertical offset; visual panels also reset to `scale(.985)`.

Active layers enter over 700ms with `cubic-bezier(.22,1,.36,1)`. Delays progress from the 60ms eyebrow through the 140ms heading, 220ms copy, 300ms actions, and 360ms visual. Inspection cards finish the sequence from 360–520ms. Leaving a slide resets these layers, so returning to it replays the reveal.

## Visual logic

- Nearby Echo pulses originate from both device cards; the center confirms the connection.
- The nearby-discovery icon and rings share one geometric center.
- The binary visual shows sender and receiver devices, encrypted chunk movement, and returning ACK/control traffic.
- LAN utilization copy presents the observed 75–80% range as validation evidence rather than a guarantee.
- Inactive desktop slides pause their concept animations.
- `prefers-reduced-motion: reduce` removes reveal transforms and transitions, keeps all reveal content visible, and reduces existing concept animation.

## Copy

Simplified Chinese public copy uses `近距离即焚式传输`, `云端中转`, `格式无关`, `二进制传输路径`, `局域网原生`, `验证记录`, `当前限制`, and `非侵入式调度诊断` consistently where applicable.

## Verification

- `npm run build`: used because pnpm was unavailable; Astro reported 0 errors, 0 warnings, and generated all 6 routes.
- Routes checked: `/`, `/download`, `/architecture`, `/zh-CN/`, `/zh-CN/download`, `/zh-CN/architecture`.
- Canonical links, hreflang pairs, and language-switch route preservation: passed.
- Desktop slide dimensions and no body overflow at 1440px, 1280px, and 864px: passed.
- Mobile vertical fallback and no body overflow at 390px: passed.
- Rapid wheel burst: advanced one slide only.
- Simulated inertial wheel tail extending beyond 500ms: advanced one slide only; a later quiet/new gesture advanced once.
- Rapid arrow presses: advanced one slide only; a later press after the 325ms cooldown advanced once.
- Navbar/hash navigation, progress-dot navigation, active progress state, and fixed header: passed.
- Staged reveal entry, inactive reset, and replay after returning to a slide: passed.
- Reduced-motion rules: source-verified to keep content immediately visible with transforms and transitions disabled.
- Browser console: no errors.

## Documentation synchronization

Repository documentation points to `site/` as the static Astro + TypeScript + Tailwind product website, records the Cloudflare Pages build settings, uses canonical latest/all GitHub Release links, and keeps public product claims aligned with the current transfer architecture and validation caveats.

## Known limitation

The 390px mobile layout intentionally uses vertical stacking instead of horizontal slide swiping.

final result: passed
