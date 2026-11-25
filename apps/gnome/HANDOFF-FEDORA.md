# Handoff: install and test Dictto on Fedora GNOME

You are a Claude CLI session on the owner's Fedora PC (GNOME, Wayland). Your job:
install the GNOME build of Dictto, make it work end to end, then bring it up to
date with the Windows features added since it was written. Commit on `main`
with the form `subject: description` and no co-author line (owner's rule).

## What exists

- `crates/dictto-core`: shared, OS-neutral logic (dictation state machine,
  pipeline, live chunk transcription, settings, keyring, speech, mic routing).
  Read the root `CLAUDE.md` first.
- `apps/gnome`: daemon `dictto-gnome` (no GTK). evdev keyboard, logind
  session gating, PipeWire audio and the `dictto_microphone` virtual source,
  uinput Ctrl+V paste, AT-SPI selection reading, D-Bus service
  `com.dictto.App` (`apps/gnome/dbus/com.dictto.App1.xml`).
- `apps/gnome-settings`: `dictto-gnome-settings`, GTK4/libadwaita window,
  talks to the daemon only over D-Bus. `dictto-gnome --settings` execs it.
- `apps/gnome/extension`: GNOME Shell extension `dictto@dictto.app`
  (indicator, recording pill, clipboard service `com.dictto.Shell1`).
- `scripts/install-gnome.sh`: build, install to `~/.local`, one-time sudo step
  (uinput udev rule, `input` group).

Everything was written on Windows and only compiled and unit-tested in WSL
Ubuntu. Nothing has ever run on real GNOME: expect bugs in the PipeWire, evdev,
uinput, AT-SPI and GNOME Shell code paths. Fix them as you find them.

## Phase 1: install and bring-up

Follow `apps/gnome/FEDORA-BRINGUP.md` in order: dnf packages, build, install,
log out and back in, enable the extension, then the checklist (extension
loads, daemon Status, permissions, Ctrl+Super dictation and paste, short-tap
retry, overlay states, settings window, microphones, speech shortcuts,
Dictto Microphone routing and restore, autostart, idle memory with no libgtk
mapped in the daemon). Its "Known follow-ups" list holds the edge cases
reviewers deferred; fix the ones you actually hit.

The owner needs an API endpoint and key in Settings for dictation to work;
ask them for it rather than guessing. Ask before any step that needs their
hands (log out, speaking into the mic, unlocking).

## Phase 2: catch up with commits made after the GNOME build

The GNOME build matches the core as of `560b575`. These later commits change
behaviour the GNOME side must follow. It still compiles against them, but it
does not use the new features yet.

1. **Live chunk transcription** (`62a5dd1`, `771976c`, `b4543eb`, `f49e8b9`,
   `74dc646`, `28e9a0f`). The core now calls
   `Recorder::start_with_tap(device, tap: SampleTap)` and streams 16 kHz mono
   `i16` packets to it while recording; `live.rs` transcribes chunks cut by
   `audio/segmenter.rs` at pauses, so release only waits for the last chunk.
   `PipewireRecorder` (apps/gnome/src/audio/recorder.rs) only implements
   `start`, so the default falls back to whole-recording transcription.
   Implement `start_with_tap`: pass the tap with the capture command and call
   it on the PipeWire loop with exactly the resampled 16 kHz mono samples that
   are appended to the recording (see `crates/dictto-windows/src/recorder.rs`
   for the reference order). Check that release is faster on a long dictation.
2. **Model catalog** (`7f7db0d`, `05ab826`). `Runtime::models(refresh)` and
   `Runtime::discover_models(generation, refresh)` list the endpoint's
   transcription and chat models from one cached `/models` call; the result
   arrives as event `model-catalog` (`dictto_core::events::MODEL_CATALOG`).
   Add a D-Bus method to `com.dictto.App1` (e.g. `DiscoverModels(t generation,
   b refresh)`), update the XML, the `AppBackend` trait and its fake, and the
   private-bus test. Events already reach clients through the `Event` signal.
3. **Settings reorganised** (`74fcf8e` native, `8af9137` Tauri, `34fd797`).
   Native now has tabs General, Dictation, Voice, Shortcuts, Connection;
   every setting saves as it changes (no Save buttons); models load in the
   background when the window opens and stay until Refresh; transcription and
   cleanup models, microphone, recording limit, paste delay and endpoint are
   lists (endpoint typed or preset); engine options get one typed control
   each instead of JSON; the debug Record/Stop/Cancel buttons are gone; a
   status line tells whether the endpoint answered. Source of truth:
   `apps/native/src/ui.rs`, `ui_api.rs`, `ui_keys.rs`, `ui_speech.rs`.
   Rework `apps/gnome-settings` to the same structure and behaviour, keeping
   all writes through the daemon (`SetSetting`, `SaveKey`, `RemoveKey`, the new
   models method). Keep shortcut fields recording physical keys (evdev
   positions) as they do now.
4. **Behaviour that comes with the core for free** (`fd1dd07`, `1dbcd30`,
   `b3e01be`): failed recordings queue for a tap retry, one automatic
   transcription retry, better cleanup prompt. Just confirm them while testing.

## Ground rules

- Keep the daemon free of GTK (check `/proc/<pid>/maps`) and light: report
  its idle RSS and thread count once everything works.
- No business logic in the Shell extension.
- Do not change Windows behaviour; if a core change is needed, run
  `cargo test -p dictto-core --all-features` and say what Windows needs
  checked.
- Test on the real desktop rather than writing more mock-heavy tests; add a
  unit test only for logic you fix that can be tested without GNOME.
- When done, update `FEDORA-BRINGUP.md` (what now works, what does not) and
  report back to the owner with a short list of anything left.
