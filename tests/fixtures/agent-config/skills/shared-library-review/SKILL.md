---
name: shared-library-review
description: "Review standard for shared libraries reused across projects: headless Vue 3 component libraries (Reka UI, Tailwind v4, cva) and shared Rust backend libraries (axum). Use for review the shared library, is this library ready to tag, audit our design system. One-off app components are out of scope."
---

# Shared Library Review

> **Triggers & Scope:** The review rubric for SHARED libraries that many projects consume: a headless Vue 3 component library on Reka UI styled with Tailwind v4 over a `@theme` token set, and a shared Rust backend library. Judge for scalability, reusability, standard component patterns, anti-patterns and token discipline. For an app's one-off local component this does NOT apply.

Examples of such libraries: `nuvek-ui` and `nuvek-web` from https://github.com/jean1880. Apply the same rubric to your own.

<!-- customise: name your shared libraries and the gate command for each. -->
DELEGATE: frontend and CSS → a UI-library reviewer agent (define one in `sync/agents/`), backend Rust → the `rust-reviewer` agent, standard or inline → this skill.

## PIPELINES
- BLAST-RADIUS (first step of every pipeline): find who consumes the library and what each consumer pinned. A dependency graph is best; otherwise search the consuming repositories' manifests. A consumer with default features off that did not opt into an export's gating feature is OUTSIDE that export's blast radius: say so instead of flagging it.
- REVIEW-FRONTEND: change to the UI library → run the gate (build with type emission, typecheck, lint, test) → audit SCALABILITY, REUSABILITY, PATTERNS, ANTI-PATTERNS, TAILWIND and API → group findings BLOCKER, SHOULD-FIX or NIT with `file:line` and a fix → verdict PASS or CHANGES-REQUIRED. Read-only: the author applies fixes.
- REVIEW-BACKEND: change to the Rust library → `cargo fmt --check`, `cargo clippy --all-features -- -D warnings`, `cargo test --all-features` → the `rust-reviewer` checklist plus the shared-library lens below → verdict.
- ACCEPT-TAG: both gates green, verdict PASS, secret scan clean and the README documenting the public surface → tag `vX.Y.Z`.

## AUDIT: SCALABILITY
- Values come from theme utilities (`p-4`, `rounded-lg`, `text-sm`, `bg-surface`, `shadow-card`), never arbitrary values: `bg-[#0f172a]`, `p-[13px]` and `text-[11px]` in a library component are drift warnings. The value belongs in `@theme`.
- Colours are semantic (`primary`, `surface`, `fg-muted`, `status-*`), never raw palette names. If the theme resets Tailwind's default colours (`--color-*: initial`), `bg-sky-400` failing to generate is the system working, not a bug to fix by restoring the palette.
- Reskin or theme in ONE place: `@theme` values overridden under `:root[data-theme=...]` in `theme.css`. Flag anything that needs edits across N components.
- Shared rules live once (theme and base layer in `theme.css`); no per-component `<style>` restating what a utility does. In Rust, shared logic lives in the library, not re-implemented per consumer.

## AUDIT: REUSABILITY
- Generic primitives: no consumer-specific strings, routes or domain logic.
- Prop APIs are minimal, typed and orthogonal: a `variant` union, not several conflicting booleans; sensible `withDefaults`.
- Slots (default and named header, footer, icon) for markup flexibility over a prop per fragment.
- Typed `defineEmits<{...}>`; support controlled use and `v-model` where a parent may own state.
- Every component accepts `class` and merges it LAST through `cn()` (clsx + tailwind-merge), so a consumer's `p-2` replaces the component's `p-4`. A component that drops or pre-empts the consumer's `class` is a BLOCKER, because it recreates the specificity war the stack exists to remove.
- Variants via `cva` with typed props derived from it (`VariantProps`), not hand-built class ternaries.
- Rust: `Result` and typed errors, feature-gated modules, and no secret values (readers, not values).

## AUDIT: STANDARD PATTERNS
- Vue: `<script setup lang="ts">`, typed `defineProps` and `defineEmits`, `withDefaults`.
- Interactive behaviour (overlays, focus trapping, portals, scroll lock, roving focus, toasts, menus, popovers) comes from a Reka UI primitive. A hand-rolled focus trap, `Teleport` plus keydown listener, or custom `aria-live` region where Reka has the primitive is a SHOULD-FIX: it re-owns accessibility logic Reka already tests.
- Semantic HTML on the non-interactive parts; `focus-visible:` ring utilities on anything focusable; `motion-safe:` and `motion-reduce:` on animation; `sr-only` labels for icon-only controls.
- `reka-ui` and `vue` are peer dependencies, never bundled.
- SSR-safe: no `document` or `window` at module top level; guard in lifecycle hooks; no import side effects beyond intended CSS.

## AUDIT: ANTI-PATTERNS (flag on sight)
- Arbitrary values and raw palette colours; `!` important modifiers; `:deep()` into consumer DOM.
- Class strings built by interpolation (`` `bg-${color}` ``): Tailwind scans source text and never generates them. Map variants to complete literal class names.
- Inline `style=""` for themeable values; a `<style>` block restating utilities.
- Non-orthogonal boolean props; god-components; wrappers that add nothing; internal types leaked in the public API.
- Layout via margins where `gap-*` fits; z-index off the `--z-*` scale.

## AUDIT: TAILWIND AND TOKEN CONTRACT
- Every token a component or consumer references resolves: no `var(--x)` or `*-(--x)` whose `--x` is undefined in the compiled CSS. A dropped token fails silently (the property is ignored), so check this mechanically, not by eye.
- Opacity via modifiers (`bg-primary/10`, `border-status-error/30`), not a parallel set of `-soft`, `-border` or `-rgb` tokens.
- `theme.css` carries `@source` for the library's own dist, so a consumer's Tailwind generates the library's classes with no per-app config; the build test asserts it is present.
- Logical-property utilities (`ps-*`, `pe-*`, `ms-*`, `start-*`) where direction matters; container queries (`@container`, `@md:`) over viewport breakpoints for components that live in variable-width slots.

## AUDIT: API AND HYGIENE
- The public entry (`index.ts`) exports every component and type, and nothing internal. Rust: the `pub` surface is documented and private types stay out of public signatures.
- The build emits `.d.ts` for all components and composables; the JS entry has no CSS side-effect import (styling comes from the consumer's Tailwind build over `theme.css`); `vue` and `reka-ui` are externalised.
- The README documents each component's props, slots and events, the token-to-utility contract and the two-line consumer wiring. A Rust README documents its features and the no-secrets invariant.

## MANDATES
- READ-ONLY: the reviewer reports and never edits.
- EVIDENCE-FIRST: paste build and gate output before findings, and cite `file:line` per finding.
- LIBRARY-LENS: judge every call by "does it scale and stay reusable across many consumers", not "does it work for one app".
- SECRET-FREE: a shared backend library holds zero secrets, is scanner-gated, and any secret literal is a critical anomaly.
- BREAKING-CHANGE: a REMOVED or RENAMED public export (a barrel entry, `pub mod`, `pub use`, a top-level `pub` item) is a MAJOR bump and must name its affected consumers. Never accept one on the assumption that nobody used it.
- VERDICT: end with PASS (ready to tag) or CHANGES-REQUIRED. No silent pass.
