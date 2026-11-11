# Shared Core Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Dictto's OS-independent logic into `crates/dictto-core`, move the Windows code shared by both Windows variants into `crates/dictto-windows`, and make `apps/native` and `apps/desktop` thin hosts with unchanged behaviour.

**Architecture:** `apps/native/src/host.rs` (the Tauri look-alike shim) and `NativeRuntime` become the core's host and runtime. The core reaches the OS only through a `Platform` bundle of trait objects, supplied by the app at construction. Tauri adapts by giving the core an `EventSink` that forwards to `tauri::Emitter`.

**Tech Stack:** Rust 2021, tokio, async-trait, thiserror, rusqlite (bundled), reqwest, keyring, windows 0.62.

**Spec:** `docs/superpowers/specs/2026-11-11-shared-core-gnome-design.md`

## Global Constraints

- `dictto-core` has no `windows`, `winreg`, `tauri`, `enigo`, `arboard` dependency and no `cfg(windows)` / `cfg(target_os)` in its logic. Exception: `std::os::windows` file-sharing flags in the speech cache move behind a `Platform`-free helper in `dictto-windows` (see Task 6).
- Stored settings strings stay byte-identical: hotkey values such as `"Ctrl+Win"`, `"CTRL+SHIFT+F8"`, setting keys such as `tts_hotkey_read_clone`.
- Data paths unchanged: `%LOCALAPPDATA%\com.dictto.app`, `voice-sample.wav`, `Speech Audio\`.
- The Tauri variant does not gain speech features: `dictto-core` feature `speech` is enabled by `apps/native` only.
- Windows virtual mic baseline is VB-CABLE (`apps/native/src/speech/communications.rs::cable_endpoint`). `drivers/dictto-mic` is archived and untouched.
- Native release exe size and idle working set: no increase beyond ~1 % memory / ~50 KB exe. Record before/after numbers in the final commit body.
- Commits: `subject: description` form, medium granularity, on `main`, no co-author line.
- Comments: only the *why*; no decorative characters.

## Review Focus

1. **Existing database from a previous install** must open with every setting and the API key intact (Task 2 test `opens_database_written_by_previous_layout`).
2. **A chord whose key is held when settings change** must send its release (`Tracker::reset`) so recording never sticks on (Task 3 test `reconfigure_while_held_releases`).
3. **Unknown hotkey string in the DB** (hand-edited, or written by a future version) falls back to `Ctrl+Win` instead of failing startup (Task 4 test `invalid_stored_hotkey_falls_back`).
4. **Virtual-mic recovery journal left by an interrupted exit** is honoured after the move (Task 6 test `recover_restores_journaled_endpoint` keeps passing against the moved code).
5. **Both variants open at once** still refuse a second recording session (Task 7 manual check, instance guard).

---

### Task 1: Workspace and empty crates

**Files:**
- Create: `Cargo.toml` (repo root, Cargo workspace)
- Create: `crates/dictto-core/Cargo.toml`, `crates/dictto-core/src/lib.rs`
- Create: `crates/dictto-windows/Cargo.toml`, `crates/dictto-windows/src/lib.rs`
- Modify: `apps/native/Cargo.toml`, `apps/desktop/src-tauri/Cargo.toml` (add path deps; keep existing deps for now)
- Modify: `.gitignore` (root `target/` already ignored)

**Interfaces:**
- Produces: workspace members `crates/dictto-core`, `crates/dictto-windows`, `apps/native`, `apps/desktop/src-tauri`. Core features: `default = []`, `speech = []`.

- [ ] **Step 1:** Create the root workspace with `resolver = "2"`. Move each app's `Cargo.lock` usage to the single root lock (delete `apps/native/Cargo.lock` after the first root build succeeds). Keep each app's `[profile]` settings by moving them to the root manifest (profiles are only honoured there).
- [ ] **Step 2:** Run `cargo build --workspace` and `cargo test --workspace`. Expected: same test count as before (`cargo test --manifest-path apps/native/Cargo.toml` and `--manifest-path apps/desktop/src-tauri/Cargo.toml --lib` run on `main` before starting; note the counts).
- [ ] **Step 3:** Measure baseline: `scripts/build-native.ps1` release exe size and the idle working set / private bytes / threads of `dictto-native.exe --smoke-test` after 10 s (Task Manager or `Get-Process`). Save numbers in `docs/superpowers/plans/core-baseline.txt`.
- [ ] **Step 4:** Update `scripts/build-native.ps1` and `scripts/build-fleet.ps1` for the root `target/` directory. Verify both scripts still produce an installed build.
- [ ] **Step 5: Commit** `build: introduce a Cargo workspace with core and Windows crates`

### Task 2: Move neutral services into the core

**Files:**
- Move (git mv) from `apps/desktop/src-tauri/src/services/` to `crates/dictto-core/src/`: `db/` (mod, settings, keystore), `transcription/`, `llm/`, `pipeline.rs`, `warmup.rs`, `endpoint_tests.rs`
- Move: `apps/native/src/host.rs` to `crates/dictto-core/src/host.rs`
- Create: `crates/dictto-core/src/audio/mod.rs` (`MicrophoneInfo`, `DeviceSelection`, `DeviceMonitorState`, `AudioError`), `crates/dictto-core/src/audio/dsp.rs`
- Modify: `apps/native/src/lib.rs`, `apps/desktop/src-tauri/src/lib.rs` and every `crate::services::` / `crate::host` import

**Interfaces:**
- Produces:
  - `dictto_core::host::{AppHandle, Manager, Emitter, EventSink, async_runtime}` (same API as the current native shim; `Manager::state::<T>()` now resolves `dictto_core::CoreState`).
  - `dictto_core::db::Database` (unchanged API: `new(PathBuf)`, `get_setting`, `set_setting`, `get_all_settings`, `save_history`), `dictto_core::db::keystore::{set_api_key, get_api_key, delete_api_key}`.
  - `dictto_core::audio::dsp::{to_mono_i16_f32(&[f32], usize) -> Vec<i16>, to_mono_i16(&[i16], usize) -> Vec<i16>, resample(&[i16], u32, u32) -> Vec<i16>, has_speech(&[i16]) -> (bool, f64), encode_wav(&[i16]) -> Result<Vec<u8>, AudioError>}` (moved from `recorder.rs:1017-1135`, renamed from `convert_to_mono_*`).
  - `dictto_core::audio::DeviceSelection` and `DeviceMonitorState` moved out of `device_monitor.rs` (the struct and its getters only; the WASAPI notifier stays for Task 5).
- `keyring` features are chosen by the app: core depends on `keyring = "3"` without backend features; `dictto-windows` enables `windows-native`.

- [ ] **Step 1: Write the failing test** in `crates/dictto-core/src/db/mod.rs`:

```rust
#[test]
fn opens_database_written_by_previous_layout() {
    // fixture: copy of a dictto.db produced by the current main build
    let dir = tempdir_with_fixture("tests/fixtures/legacy-dictto.db");
    let db = Database::new(dir.clone()).unwrap();
    assert_eq!(db.get_setting("hotkey").as_deref(), Some("Ctrl+Win"));
    assert_eq!(db.get_setting("tts_hotkey_read_clone").as_deref(), Some("Ctrl+Shift+F9"));
}
```

Produce the fixture by running the current native build once against an empty temp data dir and copying the resulting database into `crates/dictto-core/tests/fixtures/`.
- [ ] **Step 2:** Run `cargo test -p dictto-core opens_database` — FAIL (module not moved yet).
- [ ] **Step 3:** Perform the moves with `git mv` so history follows. Replace `use crate::host as tauri;` with `use crate::host;` and `tauri::` paths accordingly. The `#[cfg_attr(feature = "tauri-host", tauri::command)]` attributes are removed from core; Tauri wraps them in Task 7.
- [ ] **Step 4:** Introduce `dictto_core::CoreState` with exactly the fields of today's native `AppState` minus anything Windows-typed (`audio_recorder` becomes `Mutex<Box<dyn Recorder>>` in Task 5; until then keep the concrete type re-exported from `dictto-windows`). Apps stop defining their own `AppState`.
- [ ] **Step 5:** Run `cargo test --workspace`. Expected: PASS, test count >= baseline.
- [ ] **Step 6: Commit** `core: move database, transcription, LLM and pipeline into dictto-core`

### Task 3: Neutral key model

**Files:**
- Create: `crates/dictto-core/src/keys.rs` (from `services/hotkey/chord.rs`)
- Create: `crates/dictto-windows/src/keymap.rs`
- Delete: `apps/desktop/src-tauri/src/services/hotkey/chord.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key { Ctrl, Alt, Shift, Super, Space, Enter, Tab, Backspace, Char(u8) /* A-Z, 0-9 */, F(u8) /* 1..=24 */ }
pub struct Chord { keys: BTreeSet<Key> }      // parse(&str) -> Result<Chord, String>, overlaps(&Chord) -> bool, display(&self) -> String
pub struct Tracker { pub chord: Chord, pub suspended: bool, held: BTreeSet<Key>, active: bool }
impl Tracker { pub fn reset(&mut self) -> Option<bool>; pub fn event(&mut self, key: Key, down: bool) -> Option<bool>; }
```
  `"WIN"` and `"SUPER"` both parse to `Key::Super`; `display` writes `Win` so stored strings round-trip unchanged.
- `dictto_windows::keymap::from_vk(u16) -> Option<Key>` mapping `0x11/0xA2/0xA3 -> Ctrl`, `0x12/0xA4/0xA5 -> Alt`, `0x10/0xA0/0xA1 -> Shift`, `0x5B/0x5C -> Super`, `0x70..=0x87 -> F(1..=24)`, letters/digits to `Char`.

- [ ] **Step 1: Write the failing tests** in `keys.rs`, porting every existing test of `chord.rs` to `Key`, plus:

```rust
#[test] fn win_and_super_are_the_same_key() { assert!(Chord::parse("Ctrl+Super").unwrap().overlaps(&Chord::parse("Ctrl+Win").unwrap())); }
#[test] fn stored_strings_round_trip() { assert_eq!(Chord::parse("CTRL+SHIFT+F8").unwrap().display(), "Ctrl+Shift+F8"); }
#[test] fn reconfigure_while_held_releases() {
    let mut t = Tracker { chord: Chord::parse("Ctrl+Win").unwrap(), ..Default::default() };
    t.event(Key::Ctrl, true); assert_eq!(t.event(Key::Super, true), Some(true));
    assert_eq!(t.reset(), Some(false));
}
```
  and in `keymap.rs`: `left_and_right_modifiers_map_to_one_key` (`from_vk(0xA2) == Some(Key::Ctrl)`, `from_vk(0x5C) == Some(Key::Super)`).
- [ ] **Step 2:** Run `cargo test -p dictto-core keys` — FAIL.
- [ ] **Step 3:** Implement. Check how today's `Tracker::event` treats left/right VK codes (the hook passes raw `vkCode`, which is `0xA2` etc. for left/right Ctrl): `from_vk` must preserve today's matching behaviour exactly.
- [ ] **Step 4:** Run `cargo test --workspace` — PASS.
- [ ] **Step 5: Commit** `core: describe shortcuts with platform-neutral keys`

### Task 4: Dictation state machine in the core

**Files:**
- Move: `services/hotkey/mod.rs` (state machine, queue, bar actions) and `services/hotkey/speech_session.rs` to `crates/dictto-core/src/dictation/mod.rs` and `crates/dictto-core/src/speech/session.rs`
- Move: the `hook_proc` / `spawn_hook_thread` code to `crates/dictto-windows/src/keyboard.rs`

**Interfaces:**
- Produces:
  - `dictto_core::dictation::start(app: &AppHandle) -> Result<(), String>` (was `register_hotkey`; reads `hotkey` setting, falls back to `"Ctrl+Win"`, spawns the state-machine task; does **not** install any OS hook).
  - `dictto_core::dictation::key_event(key: Key, down: bool) -> bool` — returns `true` when the event must be swallowed (only speech chords can request this, as today). Called by every platform keyboard source.
  - `dictto_core::dictation::{configure(Chord), set_capture(bool), bar_start(), bar_stop(), bar_cancel()}`.
  - `dictto_windows::keyboard::install() -> Result<(), String>`: low-level hook thread calling `keymap::from_vk` then `dictation::key_event`.
- The `cfg(not(feature = "tauri-host"))` blocks become `cfg(feature = "speech")`.

- [ ] **Step 1: Write the failing tests** in `dictation/mod.rs`:

```rust
#[test] fn invalid_stored_hotkey_falls_back() { assert_eq!(hotkey_from_setting(Some("Ctrl+Banana")).display(), "Ctrl+Win"); }
#[test] fn missing_hotkey_uses_default() { assert_eq!(hotkey_from_setting(None).display(), "Ctrl+Win"); }
```
  (`hotkey_from_setting(Option<&str>) -> Chord` is the extracted fallback logic from `register_hotkey`.) Keep all existing tests of `hotkey/mod.rs` (lines 677+) passing unchanged.
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3:** Move the code. Speech hooks into the core speech module (`crate::speech::{key_event, suspend, configure, parse_bindings}`) under `cfg(feature = "speech")`.
- [ ] **Step 4:** Run `cargo test --workspace` — PASS.
- [ ] **Step 5: Commit** `core: run the dictation state machine independently of the keyboard hook`

### Task 5: Platform traits; Windows audio, injection, sounds

**Files:**
- Create: `crates/dictto-core/src/platform.rs`
- Move: `services/audio/recorder.rs` (WASAPI parts), `services/audio/device_monitor.rs` (notifier), `services/injector/`, `services/sound/`, `services/autostart.rs`, `instance.rs` to `crates/dictto-windows/src/` (`recorder.rs`, `device_monitor.rs`, `injector.rs`, `sound.rs`, `autostart.rs`, `instance.rs`)
- Keep in `apps/desktop`: `services/cursor_passthrough.rs` (Tauri-only overlay hitbox)

**Interfaces:**
- Produces (in `dictto_core::platform`):

```rust
pub trait Recorder: Send {
    fn set_app_handle(&mut self, app: AppHandle);           // emits "waveform-data" as today
    fn start(&mut self, device: DeviceSelection) -> Result<(), AudioError>;
    fn stop(&mut self) -> Result<Vec<u8>, AudioError>;       // 16 kHz mono WAV via audio::dsp
    fn cancel(&mut self);
    fn is_recording(&self) -> bool;
}
#[async_trait] pub trait TextInjector: Send + Sync {
    async fn inject(&self, text: &str, paste_delay_ms: u64) -> Result<std::time::Instant, InjectionError>;
}
pub trait Sounds: Send + Sync { fn play(&self, sound: Sound); }
pub trait Devices: Send + Sync { fn list_microphones(&self) -> Result<Vec<MicrophoneInfo>, AudioError>; }
pub struct Platform {
    pub recorder: Box<dyn Recorder>, pub injector: Arc<dyn TextInjector>,
    pub sounds: Arc<dyn Sounds>, pub devices: Arc<dyn Devices>,
    #[cfg(feature = "speech")] pub speech: crate::speech::SpeechPlatform,   // Task 6
}
```
  `Sound` and `InjectionError` move to core unchanged. The device monitor stays platform-owned: it updates `CoreState::device_monitor` and emits the same events.
- `dictto_windows::{WindowsRecorder, WindowsInjector, WindowsSounds, WindowsDevices}` implement them; `dictto_windows::device_monitor::spawn(state, app) -> Sender<()>`.

- [ ] **Step 1:** Port the recorder tests at `recorder.rs:1300-1377` that cover pure DSP into `dictto-core/src/audio/dsp.rs` (they should already pass after Task 2); keep the WASAPI-dependent ones in `dictto-windows`.
- [ ] **Step 2:** Add `pipeline` test `pipeline_uses_platform_injector` with a fake `TextInjector` recording the text it receives; run — FAIL.
- [ ] **Step 3:** Implement the traits; `pipeline::run_pipeline` calls `state.platform.injector.inject(...)`; sounds through `state.platform.sounds`.
- [ ] **Step 4:** `cargo test --workspace` — PASS. `cargo tree -p dictto-core -e normal | findstr /i "windows tauri enigo arboard winreg"` — no output.
- [ ] **Step 5: Commit** `core: reach audio, sounds and text injection through platform traits`

### Task 6: Speech feature in the core

**Files:**
- Move from `apps/native/src/`: `speech_api.rs` to `crates/dictto-core/src/speech/api.rs`; `speech.rs` (actions, bindings, chunking, `preferred_text`, session flow) to `crates/dictto-core/src/speech/mod.rs`; `speech/cache.rs`, `speech/render.rs` (digest, `with_cache`, `speak`) to core.
- Move to `crates/dictto-windows/src/speech/`: `communications.rs` (`WindowsDefaults`, `cable_endpoint`), `playback.rs`, `virtual_mic.rs` (Windows player + session checks), the UI Automation selection code (`edit_selection`, `selection_or_clipboard`), cache file-sharing flags.
- Move journal logic `observed_after_set`, `recover`, `acquire` from `communications.rs` into `crates/dictto-core/src/speech/route.rs` (they are already generic over `Defaults`).

**Interfaces:**
- Produces (core, `cfg(feature = "speech")`):

```rust
pub trait Defaults { /* exactly today's communications::Defaults methods */ }
pub trait Player: Sync { /* exactly today's render::Player methods */ }
pub trait SelectionReader: Send + Sync { fn read(&self) -> Result<String, String>; } // selection, then clipboard; never writes clipboard
pub struct SpeechPlatform {
    pub player: Arc<dyn Player + Send>, pub mic_defaults: Arc<dyn Defaults + Send + Sync>,
    pub selection: Arc<dyn SelectionReader>, pub virtual_mic_available: fn() -> Result<String, String>,
}
pub mod route { pub fn recover(path: &Path, d: &impl Defaults) -> Result<(), String>; pub fn acquire(path: &Path, d: &impl Defaults) -> Result<String, String>; }
```
  The cache gets `Cache::new(dir, open: fn(&Path) -> io::Result<File>)` so Windows passes its share-mode opener and other platforms pass `File::open`.

- [ ] **Step 1:** Move the existing tests with their code (`speech.rs:422+`, `cache.rs`, `communications.rs`, `render.rs`, `speech_api.rs:213+`). Run `cargo test -p dictto-core --features speech` — FAIL until moves compile.
- [ ] **Step 2:** Add `route` test `recover_restores_journaled_endpoint` if not already present among moved tests (fake `Defaults`, journal file written, `recover` sets the journaled id back).
- [ ] **Step 3:** Implement the moves and the `SpeechPlatform` wiring in `CoreState`.
- [ ] **Step 4:** `cargo test --workspace --all-features` — PASS. `cargo check -p dictto-core --features speech --target x86_64-unknown-linux-gnu` via WSL (`wsl -d Ubuntu -- cargo check ...` from the repo's `/mnt/d/...` path, rustup installed in WSL if missing) — PASS.
- [ ] **Step 5: Commit** `core: share speech synthesis, cache and microphone routing logic`

### Task 7: Thin Windows hosts

**Files:**
- Move: `apps/native/src/runtime.rs` to `crates/dictto-core/src/runtime.rs` as `dictto_core::Runtime`
- Modify: `apps/native/src/lib.rs`, `main.rs`, `ui*.rs`, `overlay.rs` (imports only)
- Modify: `apps/desktop/src-tauri/src/lib.rs`, `commands/*.rs`; delete `apps/desktop/src-tauri/src/host.rs`, `services/mod.rs` and emptied service dirs

**Interfaces:**
- Produces: `dictto_core::Runtime::new(data_dir: PathBuf, platform: Platform, sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static) -> Result<Runtime, String>`, `start(&self) -> Result<(), String>` (recovers mic journal, starts dictation, warmup), and today's `NativeRuntime` accessor methods unchanged in name and signature.
- `dictto_windows::platform() -> Platform` assembles the Windows implementations.
- Tauri: `#[tauri::command]` wrappers in `commands/` call core functions; the sink passed to `Runtime::new` forwards to `app_handle.emit(event, payload)`. `set_hotkey_capture` becomes a command in `commands/audio.rs` calling `dictation::set_capture`.

- [ ] **Step 1:** Move and rewire. `cargo test --workspace` — PASS. `cargo build --release -p dictto-native` and `pnpm build` — PASS.
- [ ] **Step 2:** Manual checks on Windows (record results in the commit body):
  - `dictto-native.exe --smoke-test` opens settings; every tab renders.
  - Install with `scripts/build-native.ps1`: Ctrl+Win dictation pastes cleaned text; short tap retries; Ctrl+Shift+F9 reads a selection; "Send speech to Dictto Microphone" routes to VB-CABLE and restores the input.
  - Tauri build: dictation works; settings tabs load; no speech shortcuts.
  - With both variants running, the second refuses to record (instance guard).
- [ ] **Step 3:** Re-measure exe size and idle memory as in Task 1 Step 3; append to `core-baseline.txt`. Expected: within the Global Constraints tolerance.
- [ ] **Step 4:** Update `CLAUDE.md` module tree and `apps/native/README.md` "shared modules" paragraph to point at `crates/`.
- [ ] **Step 5: Commit** `native: host the shared core runtime` (body: measurements and manual check results)

### Task 8: Linux CI for the core

**Files:**
- Create: `.github/workflows/core.yml`

- [ ] **Step 1:** Workflow on `ubuntu-latest`: `cargo test -p dictto-core --all-features`; on `windows-latest`: `cargo test --workspace --exclude dictto-gnome`. Triggers: push and pull_request touching `crates/**`, `apps/**`, `Cargo.*`.
- [ ] **Step 2:** Run the Linux job locally first in WSL: `cargo test -p dictto-core --all-features` — PASS.
- [ ] **Step 3: Commit** `ci: test the shared core on Linux and Windows`
