# Theme Palette Reference

> **Note:** This document is auto-generated from the actual CSS and should be updated when theme colors change.

## CSS Custom Properties
(Derived from `:root` in `index.css`)

- `--background`: 228 48% 7% → Deep space navy (#0b0f1a)
- `--foreground`: 220 100% 96% → Near-white blue (#ebf0ff)
- `--card`: 230 45% 13% → Dark card surface (#121b30)
- `--card-foreground`: 220 80% 95% → Bright card text
- `--primary`: 220 100% 65% → Activity icon blue (#4c8bff)
- `--secondary`: 228 36% 22% → Muted navy (#24304a)
- `--muted`: 228 30% 19% → Dimmed surface
- `--muted-foreground`: 221 22% 71% → Subdued text
- `--accent`: 262 83% 64% → Cosmic purple (#8b5cf6)
- `--destructive`: 262 83% 58% → Cosmic purple, darker (#7c3aed)
- `--border`: 228 29% 33% → Subtle border
- `--input`: 228 26% 30% → Input field bg
- `--ring`: 220 100% 65% → Focus ring blue (matches primary)

## Semantic Color Roles
- **Background**: deep-space navy with radial gradient overlays
- **Cards**: frosted glass with `bg-slate-900/50` or `bg-background/30`
- **Primary actions**: activity-icon blue (`--primary: 220 100% 65%` / `#4c8bff`) for all standard buttons
- **Accent/interactive**: cosmic purple (toggle switches, outgoing message bubbles)
- **Text**: near-white for headings (`text-slate-100`), muted for descriptions (`text-muted-foreground`)
- **Destructive**: cosmic purple (`--destructive: 262 83% 58%` / `#7c3aed`) for delete/destructive actions

## Key Gradient Palette
(From CSS classes)
- **Hero banner**: blue-to-purple gradient `rgba(21,47,124) → rgba(48,70,180) → rgba(113,68,205)`
- **Outgoing messages**: purple gradient `rgba(74,94,233) → rgba(125,90,222)`
- **Incoming messages**: blue gradient `rgba(41,73,148) → rgba(40,94,162)`
- **Atmosphere**: multi-layer radial gradients with blue/purple/cyan

## Glow/Shadow Palette
- **Cyan glow**: `rgba(108,220,255)` — used on status orbs, thread alerts
- **Purple glow**: `rgba(133,123,255)` / `rgba(139,92,246)` — used on mark shell, toggle switches
- **Blue shadows**: `rgba(10,22,68)` / `rgba(12,30,87)` — depth shadows

## Status Colors
- **Online**: `rgba(76,139,255,0.24)` background with `rgba(120,172,255,0.6)` border
- **Offline**: `rgba(8,12,20,0.84)` background with `rgba(67,79,101,0.8)` border
