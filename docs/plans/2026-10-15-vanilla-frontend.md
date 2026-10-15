# Plain JavaScript frontend migration

User-authorized scope: remove React and Tailwind, retain dictation/settings behavior,
use matching switches, and propose a separate Windows 11 native build.

Architecture: native ES modules and semantic HTML/CSS, with lazy-loaded settings
and recording views. A platform adapter wraps existing Tauri commands/events and
supports a local browser preview. Keep all existing backend command names.
System fonts avoid remote font loading. Flags become static SVG assets.

- [x] Settings: general/audio/API tabs, consistent switches, API-key editor,
  hotkey capture, searchable language dialog and live microphone selection.
- [x] Overlay: direct DOM waveform updates, state-driven visibility, native
  no-focus click routing, errors and existing command/event compatibility.
- [x] Integration: remove React/Zustand/Tailwind and obsolete framework tests,
  add native DOM behavior tests, build and visually verify both themes.
- [x] Proposal: separate WinUI build with system light/dark theme and shared
  Rust services; define overlay feasibility gate and migration boundaries.

Ruling: work on main as requested by the user's repository instructions.
Ruling: native Windows build is a proposal, not an authorized full second rewrite.

Validation: native DOM integration tests, production bundle inspection, browser
visual check, Rust checks if backend code changes. Compare to the previous
490.84 kB JavaScript / 20.28 kB CSS build without claiming RAM savings.

Results: 16 native DOM tests pass. Production browser checks verified API switches,
700x500 scrolling/layout, both palettes and language search/select/save with focus
restoration. Release Tauri executable builds. Independent review verified command
contracts, hitbox conversion, cleanup and chunk emission. Physical Windows overlay
focus and actual microphone/network requests were not exercised.

JavaScript across all chunks is approximately 46 kB (about 91% smaller); CSS is
approximately 12 kB. Flags are separate local SVG assets. The overlay no longer
loads the settings/language module. These are build sizes, not memory measurements.
