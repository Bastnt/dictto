# Dictto GNOME Native Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `apps/gnome`, a native Dictto for Fedora GNOME (Wayland) with feature parity with Dictto Native for Windows, on top of `dictto-core`.

**Architecture:** A graphics-free Rust daemon owns `dictto_core::Runtime` and implements its `Platform` with evdev (keys), uinput (paste), PipeWire (audio, virtual mic), AT-SPI (selection) and D-Bus. A minimal GNOME Shell extension draws the indicator and overlay and handles the clipboard. Settings are a GTK4/libadwaita process started on demand from the same binary.

**Tech Stack:** Rust, tokio, `zbus` (D-Bus + logind), `evdev` (input and uinput virtual keyboard), `pipewire` (libpipewire bindings), `atspi`, `gtk4` + `libadwaita`, `keyring` (Secret Service backend), GNOME Shell extension in ES modules (GJS).

**Spec:** `docs/superpowers/specs/2026-11-11-shared-core-gnome-design.md`
**Depends on:** `docs/superpowers/plans/2026-11-11-shared-core-extraction.md` (complete first). Uses `dictto_core::{Runtime, Platform, host::AppHandle, keys::Key, dictation::key_event, platform::{Recorder, TextInjector, Sounds, Devices}, speech::{SpeechPlatform, Player, Defaults, SelectionReader}, audio::dsp}` exactly as defined there.

## Amendments from the completed core extraction (binding; supersede task text below)

The core landed with these exact interfaces (read the code in `crates/dictto-core` before each task):

- `dictto_core::Runtime::with_speech(data_dir: PathBuf, platform: Platform, speech: SpeechPlatform, sink: impl Fn(&str, serde_json::Value) + Send + Sync + 'static) -> Result<Runtime, String>` (feature `speech`); `Runtime::new` is the no-speech variant. `start`, `shutdown`, `session_changed(suspended)`, `set_setting`, `get_setting`, `get_settings`, `microphones`, `set_hotkey_capture`, `start_recording`/`stop_recording`/`cancel_recording`, `discover_speech_models`, key accessors.
- `Platform { injector: Arc<dyn TextInjector>, sounds: Arc<dyn Sounds>, devices: Arc<dyn Devices>, services: Arc<dyn Services>, new_recorder: Arc<dyn Fn() -> Box<dyn Recorder> + Send + Sync> }`. `Services { set_autostart(bool), install_input() (must fail without side effects; called once), spawn_device_monitor(state, app) -> mpsc::Sender<()> }`. So: the evdev keyboard (Task 2) is started from `Services::install_input`; the PipeWire device monitor (Task 3) from `spawn_device_monitor`; autostart (Task 6) from `set_autostart`. There is no separate recorder field: `new_recorder` builds it.
- `dictto_core::dictation::key_event(source: u16, key: Option<Key>, down: bool) -> bool`: pass the evdev code as `source`, `keymap::from_evdev(code)` as `key`, also for unmapped keys (`None`). Ignore the returned bool (evdev cannot swallow).
- `SpeechPlatform { player: Arc<dyn Player + Send>, virtual_mic: Arc<dyn VirtualMic>, mic_route: Arc<dyn MicRouteControl>, selection: Arc<dyn SelectionReader>, cache_files: cache::CacheFiles }`. Virtual-mic policy (journal, manual-change-wins polling, restore, session suspend) is already in core: `speech::virtual_mic::{MicRoute::new(journal, defaults, output), virtual_mic(&route)}` generic over `route::Defaults` (implement `system_name()`/`input_name()` with Linux wording, e.g. "PipeWire" / "default microphone") and `MicOutput { ensure_present, locate, wait_ready, play }`. Task 8 implements only `PipewireDefaults: Defaults` and `PipewireMicOutput: MicOutput`; `wait_ready` is the up-to-8-second wait for a link to `dictto_microphone`. Do not re-implement route rules.
- `cache::CacheFiles::default()` is correct for Linux.
- `keyring` in core has no backend: `apps/gnome` must enable a Secret Service backend feature on its own `keyring` dependency (same major version as core) or keys silently go to the mock store. Add a test that the enabled backend is not the mock (e.g. `keyring::default::default_credential_builder()` type name / `keyring` API for the active builder).
- **Keyring backend from Task 1**, not Task 6: the very first `apps/gnome` Cargo.toml enables the Secret Service backend, with a test proving the active credential builder is not the mock. A mock store makes "saved" keys vanish and every dictation fail with NoApiKey.
- **Recorder** (Task 3): use `dictto_core::audio::dsp::finish` (min-length + speech gates + WAV) and `dictto_core::audio::Waveform` (bar cadence and scaling, `"waveform-data"` payload) instead of re-implementing them. Event names come from `dictto_core::events`.
- **Input interruption** (Task 2): call `Runtime::input_suspended(true/false)` (or `dictation::set_input_suspended`) when the logind session becomes inactive/locked or active again, and `dictation::reset_input()` when a keyboard device disappears or the evdev reader errors. `session_changed` stays for the mic route.
- **Settings writes go through the daemon** (Tasks 4 and 9): `com.dictto.App1` gains methods `SetSetting(s key, s value)` (calls `runtime.set_setting`, returns the error string or empty), `GetSettings() -> a{ss}`, `SaveKey(s)`, `RemoveKey()`, `KeyHint() -> s`, `Microphones() -> a(ss)`. `ReloadSettings()` is dropped. The settings process never writes the DB directly (that would skip binding validation and live side effects). Speech model discovery goes through a daemon method `DiscoverSpeechModels(t generation)` whose result arrives on the `Event` signal (`speech-catalog`). If the daemon is not running, the settings window shows "Dictto is not running" and a Start button that launches the daemon.
- **Selection reading** (Task 8): `SelectionReader::read` is synchronous; run the AT-SPI query on a dedicated thread with its own small current-thread runtime or blocking zbus connection, with a 500 ms timeout before falling back to primary/clipboard.
- **One async runtime**: the daemon runs zbus and its own tasks on `dictto_core::host::async_runtime` (spawn there; use zbus with its tokio feature) instead of building a second tokio runtime.
- `MicOutput::wait_ready` receives the route `check` (after the core fix wave): return early when it fails.
- Root workspace has `[profile.release.package.dictto-core] codegen-units = 1` and the same for `dictto-windows`.
- `.github/workflows/core.yml` exists (jobs `core-linux`, `workspace-windows`); Task 10 extends it. `apps/gnome` must compile as a stub on Windows so `cargo test --workspace` there keeps passing.
- `pnpm` is not available locally; WSL Ubuntu 26.04 has rustup, GTK 4.22, libadwaita 1.9, PipeWire 1.6, gjs 1.88, jq, shellcheck, dbus. Build with `CARGO_TARGET_DIR=$HOME/dictto-target` (never write Linux output into the repo `target/`).

## Global Constraints

- Written on Windows; compiled and unit-tested in WSL Ubuntu (`wsl -d Ubuntu`, repo at `/mnt/d/Bastnt/projets/dictto`); validated later on the owner's Fedora by a Claude CLI session. Every task ends with a WSL build + test, never with a claim that GNOME behaviour works.
- The daemon never links GTK: `gtk4`/`libadwaita` are behind cargo feature `settings` (default on), and the daemon code path must not touch them. Check with `ldd`-free test: the daemon mode must not call `gtk::init`. GTK libraries are loaded only when the process is started with `--settings` (keep GTK in a separate binary target `dictto-gnome-settings` if the linker pulls it into the daemon's resident set; measure in Task 9).
- The extension holds no business logic: draw, relay, clipboard only.
- Extension `metadata.json`: `"uuid": "dictto@dictto.app"`, `"shell-version": ["47", "48", "49"]`.
- D-Bus names (fixed; extension and daemon both depend on them):
  - Daemon: bus name `com.dictto.App`, object `/com/dictto/App`, interface `com.dictto.App1`.
  - Extension: bus name `com.dictto.Shell`, object `/com/dictto/Shell`, interface `com.dictto.Shell1`.
- Data dir: `$XDG_DATA_HOME/com.dictto.app` (default `~/.local/share/com.dictto.app`). Autostart file: `~/.config/autostart/com.dictto.App.desktop`.
- Default dictation hotkey stays `"Ctrl+Win"` (parsed as Ctrl+Super). Speech defaults are the core's defaults.
- Instance: one per user (D-Bus name ownership). Keyboard listening and pasting only while the daemon's logind session is active.
- Virtual mic: PipeWire node named `dictto_microphone`, description "Dictto Microphone". Before switching the default source, the daemon pins its own capture to the concrete node of the selected (or current default) real microphone.
- Commits: `subject: description`, medium granularity, `main`, no co-author line. Comments: only the *why*.

## Review Focus

1. **Keyboard plugged in after startup** (USB or Bluetooth) must trigger dictation without restart (Task 2 test `new_event_device_is_opened`).
2. **Extension disabled or GNOME just upgraded and extension refused**: dictation still records and transcribes, then shows a desktop notification "Dictto's GNOME Shell extension is not running" instead of silently dropping text (Task 5 test `paste_without_shell_reports_missing_extension`).
3. **User not yet in `input` group / no uinput access** (first run before re-login): daemon starts, notifies which permission is missing and how to fix it, does not crash (Task 2 test `permission_denied_is_reported`, Task 5 test `uinput_denied_is_reported`).
4. **Selected microphone unplugged mid-recording**: same behaviour as Windows (recording stops with the existing device-lost error, default re-selected) (Task 3 test `removed_node_ends_capture_with_device_lost`).
5. **Default source changed by the user while speech is routed to the virtual mic**: Dictto must not restore over the manual choice (Task 8 test `manual_change_wins_over_restore`, reusing core `route` rules).

---

### Task 1: Crate skeleton, build in WSL, shared settings model

**Files:**
- Create: `apps/gnome/Cargo.toml`, `apps/gnome/src/main.rs`, `apps/gnome/src/lib.rs`
- Create: `scripts/wsl-build-gnome.sh` (apt deps check + `cargo build -p dictto-gnome` + `cargo test -p dictto-gnome`)
- Move: neutral parts of `apps/native/src/ui_models.rs` (setting validation, model lists, display labels) to `crates/dictto-core/src/settings_model.rs`; `apps/native` imports them from core.
- Modify: root `Cargo.toml` (member `apps/gnome`, Linux-only via `[target.'cfg(target_os = "linux")']` deps inside the crate so `cargo build --workspace` on Windows skips platform code: the crate compiles to a stub `main` on non-Linux)

**Interfaces:**
- Produces: binary `dictto-gnome` with modes parsed in `main.rs`: no args = daemon, `--autostart` = daemon started by session, `--settings` = settings window, `--smoke-test` = settings without starting microphone/keyboard/warmup (same meaning as Windows).
- `dictto_core::settings_model` exposes whatever `ui_models.rs` exposes today, same names.

- [ ] **Step 1:** Write `scripts/wsl-build-gnome.sh` that verifies (and prints the `apt install` line for) `libgtk-4-dev libadwaita-1-dev libpipewire-0.3-dev libclang-dev libdbus-1-dev pkg-config`, then builds and tests the crate.
- [ ] **Step 2:** Move `ui_models.rs` neutral logic; `cargo test --workspace` on Windows — PASS (existing `ui_models` tests move along).
- [ ] **Step 3:** `wsl -d Ubuntu -- bash scripts/wsl-build-gnome.sh` — PASS with the stub `main`.
- [ ] **Step 4: Commit** `gnome: add the GNOME crate and share the settings model`

### Task 2: evdev keyboard source with hotplug and session gating

**Files:**
- Create: `apps/gnome/src/keyboard/mod.rs`, `apps/gnome/src/keyboard/keymap.rs`, `apps/gnome/src/session.rs`

**Interfaces:**
- Produces:
  - `keymap::from_evdev(code: u16) -> Option<Key>`: `KEY_LEFTCTRL/RIGHTCTRL -> Ctrl`, `KEY_LEFTALT/RIGHTALT -> Alt`, `KEY_LEFTSHIFT/RIGHTSHIFT -> Shift`, `KEY_LEFTMETA/RIGHTMETA -> Super`, `KEY_F1..=KEY_F24 -> F(n)`, `KEY_A..KEY_Z -> Char(b'A'..)`, `KEY_0..KEY_9 -> Char(b'0'..)`, `KEY_SPACE/ENTER/TAB/BACKSPACE`.
  - `keyboard::spawn(active: Arc<AtomicBool>) -> Result<KeyboardHandle, KeyboardError>`: opens every `/dev/input/event*` device that reports `EV_KEY` with `KEY_A`, reads events (value 1 = down, 0 = up, 2 = repeat ignored), calls `dictto_core::dictation::key_event` only while `active` is true; watches `/dev/input` (inotify) for new/removed devices. The `bool` returned by `key_event` is ignored (evdev cannot swallow events).
  - `KeyboardError::PermissionDenied` when no device can be opened because of `EACCES`.
  - `session::watch(active: Arc<AtomicBool>, runtime: Runtime)`: logind `org.freedesktop.login1.Session` of `$XDG_SESSION_ID` (or the session from `GetSessionByPID`), property `Active` and `Lock`/`Unlock` signals; calls `runtime.session_changed(suspended)` like Windows `WTS_SESSION_*`.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test] fn modifiers_map_left_and_right() { assert_eq!(from_evdev(KEY_RIGHTMETA), Some(Key::Super)); assert_eq!(from_evdev(KEY_LEFTCTRL), Some(Key::Ctrl)); }
#[test] fn function_keys_up_to_f24() { assert_eq!(from_evdev(KEY_F24), Some(Key::F(24))); }
#[test] fn repeat_events_are_ignored() { assert_eq!(classify(EV_KEY, KEY_A, 2), None); assert_eq!(classify(EV_KEY, KEY_A, 1), Some((Key::Char(b'A'), true))); }
#[test] fn new_event_device_is_opened() { /* DeviceSet::on_created("event7") with a fake opener returns Opened; on_removed("event7") drops it */ }
#[test] fn permission_denied_is_reported() { /* fake opener returning EACCES for all devices -> spawn() Err(PermissionDenied) */ }
```
  The device set is a pure struct `DeviceSet<O: Opener>` so hotplug logic is testable without `/dev/input`.
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement with the `evdev` crate (tokio stream feature) and `inotify` (or `notify`).
- [ ] **Step 4:** Run in WSL — PASS.
- [ ] **Step 5: Commit** `gnome: read shortcuts from input devices while the session is active`

### Task 3: PipeWire audio: capture, devices, playback, sounds

**Files:**
- Create: `apps/gnome/src/audio/mod.rs` (PipeWire thread + command channel), `audio/recorder.rs`, `audio/devices.rs`, `audio/player.rs`, `audio/sounds.rs`
- Sound assets: reuse the files Windows plays (`services/sound` source of truth after core extraction); embed with `include_bytes!`.

**Interfaces:**
- Produces: `PipewireRecorder: dictto_core::platform::Recorder`, `PipewireDevices: Devices`, `PipewireSounds: Sounds`, `PipewirePlayer: dictto_core::speech::Player`.
  - One PipeWire main-loop thread for the process; others talk to it through a channel.
  - `DeviceSelection::Default` resolves to the node id of the current default source at `start` time and the stream targets that node (`target.object`), which also satisfies the virtual-mic pinning constraint.
  - Playback-capture (`DeviceSelection` loopback variant used for clone samples) captures the monitor of the default sink (`stream.capture.sink = true`).
  - Samples go through `dictto_core::audio::dsp` (mono, 16 kHz, WAV) and waveform events identical to Windows (`"waveform-data"` payload shape unchanged).
  - `MicrophoneInfo.id` = PipeWire `node.name` (stable across reboots), `name` = `node.description`.
  - Device monitor updates `CoreState::device_monitor` and emits the same events as `dictto_windows::device_monitor`.
- Test seam: `fn select_target(selection: &DeviceSelection, nodes: &[NodeInfo], default: Option<&str>) -> Option<NodeInfo>`; `fn on_node_removed(capturing: &str, removed: &str) -> Option<AudioError>`.

- [ ] **Step 1: Write the failing tests:** `default_resolves_to_concrete_node`, `named_device_found_by_node_name`, `missing_named_device_falls_back_to_default` (match Windows behaviour in `dictto-windows` recorder), `virtual_mic_is_never_a_capture_target` (`dictto_microphone` excluded from lists and resolution), `removed_node_ends_capture_with_device_lost`.
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement with the `pipewire` crate.
- [ ] **Step 4:** Run in WSL — PASS.
- [ ] **Step 5: Commit** `gnome: capture and play audio through PipeWire`

### Task 4: D-Bus service and shell client

**Files:**
- Create: `apps/gnome/src/dbus/mod.rs`, `dbus/app.rs` (served by the daemon), `dbus/shell.rs` (proxy to the extension), `apps/gnome/dbus/com.dictto.App1.xml`, `apps/gnome/dbus/com.dictto.Shell1.xml`

**Interfaces:**
- Produces `com.dictto.App1` (daemon):
  - Methods: `ShowSettings()`, `Quit()`, `BarStart()`, `BarStop()`, `BarCancel()`, `ReloadSettings()`, `SetHotkeyCapture(b active)`.
  - Signal: `Event(s name, s payload_json)` emitted for every core event (the `EventSink` of `Runtime::new`).
  - Property: `Status` (`s`): `"ok"` or a user-facing problem string (missing permission, missing extension) that the indicator shows in its menu.
- Consumes `com.dictto.Shell1` (extension) through `ShellProxy`:
  - `SaveClipboard() -> u token` (extension keeps every mimetype's bytes in memory under the token)
  - `RestoreClipboard(u token)`
  - `SetText(s text)` (clipboard)
  - `GetText(s which) -> (b present, s text)` with `which` in `"clipboard" | "primary"`
- Instance: if `com.dictto.App` is already owned, the new process calls `ShowSettings()` on it and exits 0 (equivalent of Windows "already running" message).

- [ ] **Step 1: Write the failing tests:** `event_signal_carries_json_payload` (serialize a `recording-state-changed` event through the sink adapter, assert name and JSON), `second_instance_requests_settings` (pure decision fn `on_name_taken() -> Action::ShowSettingsAndExit`).
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement with `zbus` (tokio feature); XML files are the documentation the extension is written against.
- [ ] **Step 4:** Run in WSL — PASS. If `dbus-daemon` is available in WSL, add an integration test under `#[ignore]` that starts a private bus and calls `ReloadSettings`.
- [ ] **Step 5: Commit** `gnome: expose the daemon on D-Bus`

### Task 5: Paste through uinput and the shell clipboard

**Files:**
- Create: `apps/gnome/src/injector.rs`, `apps/gnome/src/notify.rs`

**Interfaces:**
- Produces `GnomeInjector: TextInjector` with today's Windows order: `SaveClipboard` -> `SetText` -> virtual keyboard Ctrl+V -> wait `paste_delay_ms` -> `RestoreClipboard`. Virtual keyboard created once (`evdev::uinput::VirtualDeviceBuilder`, name "Dictto virtual keyboard", keys `KEY_LEFTCTRL`, `KEY_V`).
- `notify::show(summary: &str, body: &str)` via `org.freedesktop.Notifications`.
- Errors: `InjectionError::ClipboardError("Dictto's GNOME Shell extension is not running")` when the shell proxy is unreachable; `InjectionError::KeyboardError` mentioning `/dev/uinput` access when uinput fails. Both also set the App1 `Status` property and show one notification per daemon run.

- [ ] **Step 1: Write the failing tests** with fake `Shell` and fake `VirtualKeys` traits: `paste_sequence_matches_windows_order` (records calls: save, set, ctrl-v, restore), `paste_without_shell_reports_missing_extension`, `uinput_denied_is_reported`, `restore_runs_even_if_paste_fails`.
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run in WSL — PASS.
- [ ] **Step 5: Commit** `gnome: paste dictated text through the shell clipboard and a virtual keyboard`

### Task 6: Daemon assembly

**Files:**
- Create: `apps/gnome/src/daemon.rs`, `apps/gnome/src/autostart.rs`, `apps/gnome/src/paths.rs`
- Modify: `apps/gnome/src/main.rs`

**Interfaces:**
- Consumes: Tasks 2-5, `dictto_core::Runtime::new(data_dir, platform, sink)`.
- Produces: `daemon::run(mode: Mode) -> Result<(), String>`; `autostart::{enable(exe: &Path), disable(), is_enabled()}` writing/removing `~/.config/autostart/com.dictto.App.desktop` with `Exec=<exe> --autostart`, `X-GNOME-Autostart-enabled=true`; `paths::data_dir() -> PathBuf`.
- `keyring` with Secret Service backend feature (`sync-secret-service` + `crypto-rust`, or `linux-native-sync-persistent` — pick the one the core's `keystore` API works with and that needs no extra runtime beyond gnome-keyring).
- `ShowSettings` spawns `current_exe() --settings` (single settings process; a second request focuses it through the settings process's own GApplication uniqueness).

- [ ] **Step 1: Write the failing tests:** `autostart_file_contents` (exact `.desktop` text), `data_dir_respects_xdg_data_home`.
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement; `main.rs` dispatches modes; logging to stderr + `journald` friendly format.
- [ ] **Step 4:** Run in WSL — PASS; `cargo build --release -p dictto-gnome` — PASS.
- [ ] **Step 5: Commit** `gnome: assemble the daemon around the shared runtime`

### Task 7: GNOME Shell extension

**Files:**
- Create: `apps/gnome/extension/metadata.json`, `extension.js`, `indicator.js`, `overlay.js`, `clipboard.js`, `stylesheet.css`, `icons/dictto-symbolic.svg` (from `packages/brand`)

**Interfaces:**
- Consumes `com.dictto.App1` (Task 4), exports `com.dictto.Shell1` (Task 4 XML).
- Indicator: panel button with the Dictto symbolic icon; menu items "Settings" (`ShowSettings`), "Quit Dictto" (`Quit`), and a disabled status line when `Status != "ok"`. Hidden while `com.dictto.App` has no owner.
- Overlay: `St.BoxLayout` pill bottom-centre of the primary monitor, added with `Main.layoutManager.addTopChrome(actor, {affectsInputRegion: true})` sized to the pill only; never takes focus. Mirrors the Windows overlay states from `recording-state-changed` and `waveform-data` (same bar count and spinner timing as `apps/native/src/overlay.rs`), stop/cancel buttons call `BarStop`/`BarCancel`.
- Clipboard via `St.Clipboard.get_default()`: `SaveClipboard` reads `get_mimetypes` then `get_content` per type; `RestoreClipboard` re-sets the first textual type plus any image type with `set_content`; `GetText("primary")` uses `St.ClipboardType.PRIMARY`.
- `disable()` destroys every actor, signal and exported object (extension review rule).

- [ ] **Step 1:** Write the extension. Unit tests are not practical for GJS here; instead add `apps/gnome/extension/check.sh` that runs `gjs -m` syntax checking on each file (`gjs --version` present in WSL via `apt install gjs`) and validates `metadata.json` with `jq`.
- [ ] **Step 2:** Run `check.sh` in WSL — PASS.
- [ ] **Step 3: Commit** `gnome: draw the indicator and recording overlay in GNOME Shell`

### Task 8: Speech on GNOME: selection, virtual mic, playback routing

**Files:**
- Create: `apps/gnome/src/speech/selection.rs`, `speech/virtual_mic.rs`, `speech/defaults.rs`

**Interfaces:**
- Produces:
  - `AtspiSelection: SelectionReader`: focused accessible's text selection via AT-SPI; fallback `GetText("primary")`, then `GetText("clipboard")`; never writes the clipboard.
  - `PipewireDefaults: dictto_core::speech::Defaults`: reads/writes `default.configured.audio.source` in the PipeWire `default` metadata object (value JSON `{"name":"<node.name>"}`), the same mechanism `wpctl set-default` uses.
  - `VirtualMic`: creates the `dictto_microphone` node (`media.class = "Audio/Source/Virtual"`, mono 48 kHz) only while the "Send speech to Dictto Microphone" setting is on; `PipewirePlayer` (Task 3) gets a `to_virtual_mic` target that writes into it.
  - `SpeechPlatform { player, mic_defaults: PipewireDefaults, selection: AtspiSelection, virtual_mic_available }` where `virtual_mic_available` returns the node name or a user-facing error.
  - Core `route::{acquire, recover}` drive the switch/restore and journal unchanged; the eight-second wait for a consumer uses the node's link count (any link to `dictto_microphone`'s output port counts as an app opening it).
- Session lock/inactive (Task 2 `session::watch`) triggers the same restore path as Windows lock/disconnect.

- [ ] **Step 1: Write the failing tests:** `default_metadata_value_round_trips` (`{"name":"alsa_input.usb-..."}` parse/format), `manual_change_wins_over_restore` (fake metadata store: acquire, user sets another source, restore leaves it), `selection_falls_back_to_primary_then_clipboard` (fakes), `virtual_mic_hidden_when_setting_off`.
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement with `atspi` (tokio) and `pipewire`.
- [ ] **Step 4:** Run in WSL — PASS; `cargo test -p dictto-core --features speech` still PASS.
- [ ] **Step 5: Commit** `gnome: read selections and route speech to a PipeWire virtual microphone`

### Task 9: GTK4 settings

**Files:**
- Create: `apps/gnome/src/settings/mod.rs`, `settings/general.rs`, `settings/audio.rs`, `settings/api.rs`, `settings/speech.rs`, `settings/shortcuts.rs`

**Interfaces:**
- Consumes: `dictto_core::settings_model` (Task 1), `dictto_core::db`, `dictto_core::db::keystore`, `dictto_core::speech::api::discover`, `com.dictto.App1.ReloadSettings` / `SetHotkeyCapture`.
- Pages and controls mirror `apps/native/src/ui.rs`, `ui_speech.rs` (pages General, Audio, API, Speech, Shortcuts; same fields, same labels, same validation, explicit Save buttons, unsaved edits discarded on close). `adw::PreferencesWindow`-style layout with `adw::ViewSwitcher` for the five pages.
- While a shortcut field has focus, call `SetHotkeyCapture(true)` (Windows does the same so typing a shortcut does not trigger it); `false` on blur/close.
- "Open audio folder" uses `gio::AppInfo::launch_default_for_uri` on the speech audio dir.
- Autostart checkbox uses `autostart` (Task 6).
- Process exits when the window closes.

- [ ] **Step 1: Write the failing tests** for the non-GTK glue: `save_general_writes_expected_keys` (fake DB through the core model), `shortcut_conflict_is_rejected` (reuse core overlap rules).
- [ ] **Step 2:** Run in WSL — FAIL.
- [ ] **Step 3:** Implement the window.
- [ ] **Step 4:** Run in WSL — PASS. Under WSLg, `cargo run -p dictto-gnome -- --smoke-test` opens the window: take a screenshot of each page for the owner (render check only; GNOME styling differs on Fedora). Measure the daemon process (no args, with `DICTTO_NO_INPUT=1` env to skip evdev in WSL) resident memory and confirm `/proc/<pid>/maps` contains no `libgtk-4`. If it does, split settings into a second binary target as noted in Global Constraints.
- [ ] **Step 5: Commit** `gnome: add the GTK settings window`

### Task 10: Install script and Fedora bring-up guide

**Files:**
- Create: `scripts/install-gnome.sh`, `apps/gnome/packaging/com.dictto.App.desktop`, `apps/gnome/packaging/70-dictto-uinput.rules`, `apps/gnome/FEDORA-BRINGUP.md`, `apps/gnome/README.md`

**Interfaces:**
- `install-gnome.sh`: `cargo build --release -p dictto-gnome`; install binary to `~/.local/bin/dictto-gnome`, `.desktop` + icon to `~/.local/share/{applications,icons/hicolor/...}`, extension to `~/.local/share/gnome-shell/extensions/dictto@dictto.app`, `gnome-extensions enable dictto@dictto.app`; then with `sudo` once: udev rule `KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"`, `usermod -aG input $USER`, `udevadm control --reload && udevadm trigger`. Prints that a log out / log in is required. Idempotent.
- `FEDORA-BRINGUP.md` for the Fedora Claude CLI session: `dnf install` list (`gtk4-devel libadwaita-devel pipewire-devel clang-devel dbus-devel gjs`), build/install commands, what was never run on GNOME (everything in Tasks 2-9 beyond unit tests), and a manual checklist per feature from the spec's goal list, including the Review Focus items, the Ctrl+Super vs Activities check (spec decision), and the idle memory/thread measurement to report.

- [ ] **Step 1:** `shellcheck scripts/install-gnome.sh` in WSL — no warnings.
- [ ] **Step 2:** Dry-run the non-sudo part of the script in WSL against a temp `HOME` — files land where listed.
- [ ] **Step 3:** Add the Linux build of `apps/gnome` (with apt deps) to `.github/workflows/core.yml`.
- [ ] **Step 4: Commit** `gnome: install script and Fedora bring-up guide`
