# Dictto desktop

Plain JavaScript ES modules and native CSS, hosted by Tauri/WebView2. No frontend
framework, CSS runtime or external fonts. Vite only bundles development/release assets.

From the repository root:

- `pnpm dev`: run the desktop app with the Rust backend.
- `pnpm --filter @dictto/desktop dev`: browser preview with sample settings; no real
  microphone access, model requests or API-key storage.
  Add `?theme=light` to inspect the light palette without changing Windows settings.
- `pnpm --filter @dictto/desktop test`: native DOM behavior tests.
- `pnpm --filter @dictto/desktop build`: generate the frontend in `dist/`.
- `pnpm build`: build and package the desktop application.
- `pnpm build:fleet`: build and update the installed `Dictto-Fleet` executable,
  keeping a backup. Ordinary frontend builds do not install the executable.
- `pnpm build:native`: build/install the separate native Windows 11 variant.

The settings and recording overlay load separately. Waveform events update existing
DOM bars directly. System light/dark preferences control CSS theme variables.
Flags are vendored SVGs with their upstream license in `public/flags/LICENSE.txt`.

The Windows 11 native build is a separate proposal in
`../../docs/plans/2026-10-15-windows-native-build.md`.
