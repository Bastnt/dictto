# Dictto Native

A separate Rust/Win32 application using operating-system controls and a GDI
recording overlay. No HTML, JavaScript, WebView, XAML or Windows App SDK runtime
is used. Settings use compact layouts, Segoe UI and system light/dark colours;
controls have the simpler classic Windows appearance.

Build and install alongside the web variant from the repository root:

```powershell
pwsh -NoProfile -File scripts/build-native.ps1
```

This installs a single executable to `%LOCALAPPDATA%\Programs\Dictto-Native` and
adds a Start menu shortcut. The icon and common-controls manifest are embedded.
When upgrading the previous WinUI installation, the script moves that complete
folder to a dated sibling backup before installing the smaller build. Old SDK
files in Cargo's target directory are not copied into the new install.

The Tauri/JavaScript build remains in `Dictto-Fleet`. Quit one variant from its tray
before opening the other: both use the same database, keyring and hotkey pipeline,
and a cross-variant session guard prevents simultaneous recording. No settings or
credentials are copied into the installation folder. Autostart uses whichever
variant last enabled/start-restored that setting.

Native settings use explicit save controls for editable values. Closing settings
destroys the settings window and its controls; use the tray to recreate settings
or quit. Unsaved edits are discarded on close. The recording overlay does not
activate its window. Its compact recording waveform updates across the pill on
audio events; processing displays a white spinner at 5 frames per second. No
drawing loop runs while idle.

### Speech shortcuts

In **API**, use the same authenticated fleet base URL and key as dictation.
In **Speech**, refresh the model list and choose a speech model. The available
speakers, instructions and advanced options come from that model's advertised
capabilities. In **Shortcuts**, assign the actions you want to use. All five
bindings start empty.

- **Record clone sample from microphone / Windows playback** records from the
  selected microphone or default playback endpoint. Press the shortcut again
  or use the overlay stop button to replace the local sample. Cancel discards
  the take. Sample capture stops automatically just before 30 seconds.
- **Speak in cloned voice** records the microphone. Press the shortcut again
  to transcribe with your existing STT settings, synthesize in the last
  sampled voice, and play the result. The transcript stays in memory.
- **Read selection / clipboard in cloned voice / engine voice** reads the
  focused application's accessible text selection, falling back to clipboard
  text. Dictto reads the clipboard without writing to it. Long passages play
  in sequential chunks. Press the same shortcut during synthesis or playback
  to cancel.

The current sample is stored as `voice-sample.wav` beside Dictto's database
and uploaded with each clone request. Engine-voice reading needs no sample.

Developer checks:

```powershell
cargo test --manifest-path apps/native/Cargo.toml
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
```

`dictto-native.exe --smoke-test` opens actual settings without starting the microphone,
global hotkeys or model warmup. It reads existing settings; using Save still changes
them. Add `--autostart` to verify tray-only startup without recording services.

The shared speech modules live under `apps/desktop/src-tauri/src/services`; the
native host maps their events/state access onto a two-worker Tokio runtime and
posted Win32 messages. The hidden tray host remains alive without settings.
Changes to shared services must be checked in both variants. Measure working set,
private committed memory, threads and on-disk size separately. `--smoke-test`
comparisons exclude recording hooks, device monitoring and model warmup.
