# CLAUDE.md

This file provides guidance to Claude Code when working with code in this repository.

## Build & Dev Commands

```bash
pnpm dev              # Start Tauri dev mode (frontend + backend hot-reload)
pnpm build            # Production build

# Rust-only checks (workspace root):
cargo check --workspace
cargo test --workspace
```

## Architecture

**Dictto** is a voice-to-text Windows desktop app (Tauri v2 + React + Rust). The user holds a hotkey, speaks, releases, and cleaned text is pasted at their cursor.

### Two-Window Design

1. **Recording Bar** (`"recording-bar"` label) — Transparent floating overlay at screen bottom. Shows waveform while recording.
2. **Settings** (`"main"` label) — Hidden by default, toggled via system tray. Has tabs: General, Audio, API.

### Recording Pipeline

Triggered by `Ctrl+Win` (push-to-talk):
1. Key press -> starts CPAL audio capture
2. Key release -> stops recording, returns WAV bytes (16kHz mono)
3. Transcribe via OpenAI Whisper API
4. Clean text via OpenAI GPT
5. Inject text via clipboard (Ctrl+V)

### Workspace Structure

```
crates/
├── dictto-core/src/          # Platform-neutral logic, no OS or UI dependency
│   ├── runtime.rs            # Runtime: settings, start/shutdown, events through a sink
│   ├── platform.rs           # Traits the OS implements: Recorder, TextInjector, Sounds, Devices, Services
│   ├── host.rs               # AppHandle/Emitter/async_runtime (event sink, multi-state)
│   ├── dictation/            # Push-to-talk state machine (fed by key_event)
│   ├── pipeline.rs           # Recording -> transcribe -> clean -> paste
│   ├── transcription/, llm/  # TranscriptionEngine, LLMProcessor + OpenAI impls
│   ├── audio/, keys.rs, db/  # Device monitor state, chords, SQLite + keyring
│   └── speech/               # Feature `speech` (native only): TTS, cache, mic route
└── dictto-windows/src/       # Windows impls: recorder, injector, sounds, keyboard hook,
                              # device monitor, autostart, instance guard, speech/ (WinMM, VB-CABLE)
apps/
├── desktop/src-tauri/src/    # Tauri variant (no speech): lib.rs setup, tray, commands/, cursor_passthrough
└── native/src/               # Win32 variant: ui*.rs, overlay.rs, main.rs
```

### Key Traits

- `Platform` — OS services the core calls (`dictto_windows::platform()`); a new OS supplies its own
- `TranscriptionEngine` — async trait for speech-to-text engines
- `LLMProcessor` — async trait for text cleanup processors

Both use `async-trait`. Import the trait explicitly when using in commands.

### Frontend Patterns

- **State**: Zustand store (`stores/appStore.ts`)
- **Events**: Tauri events (`recording-state-changed`, `waveform-data`)
- **IPC**: `invoke` from `@tauri-apps/api/core`
- **Styling**: TailwindCSS v4

## Conventions

- **Package manager**: pnpm (monorepo workspaces)
- **Error handling**: `thiserror` derive macros with `Serialize` for Tauri IPC
- **Async**: `tokio` runtime, `async-trait` for trait definitions
- **State**: `dictto_core::Runtime` (managed Tauri state), accessed via `State<Runtime>` in commands

## Tauri v2 Notes

- Hotkey uses `WH_KEYBOARD_LL` via Microsoft `windows` crate (`dictto-windows`)
- Clipboard permissions: `clipboard-manager:allow-read-text` / `allow-write-text`
- Commands are thin wrappers over core functions, registered in `generate_handler![]` in `lib.rs`
- Use `@tauri-apps/api/core` for `invoke` (not `@tauri-apps/api/tauri`)
