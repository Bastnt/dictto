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
In **Speech**, Dictto discovers the fleet models and selects an initial model and
speaker when available. Refresh the list or choose another model at any time. The available
speakers, instructions and advanced options come from that model's advertised
capabilities. The **Shortcuts** page shows the assigned bindings. Edit a field
and save to change it, or clear it to disable that action.

| Default shortcut | Action |
| --- | --- |
| Ctrl+Shift+F6 | Record voice sample from the microphone |
| Ctrl+Shift+F7 | Record voice sample from Windows playback |
| Ctrl+Shift+F8 | Repeat my speech in the cloned voice |
| Ctrl+Shift+F9 | Read text in the cloned voice |
| Ctrl+Shift+F10 | Read text in the engine voice |

On first setup or upgrade, Dictto preserves custom bindings and chooses an
alternative when a default overlaps an existing Dictto shortcut. Both read
actions use the focused app's selection and fall back to clipboard text.

- **Record clone sample from microphone / Windows playback** records from the
  selected microphone or default playback endpoint. Press the shortcut again
  or use the overlay stop button to replace the local sample. Cancel discards
  the take. Sample capture stops automatically just before 30 seconds.
- **Repeat my speech in the cloned voice** records the microphone. Press the shortcut again
  to transcribe with your existing STT settings, synthesize in the last
  sampled voice, and play the result. The transcript stays in memory.
- **Read text in the cloned voice / engine voice** reads the
  focused application's accessible text selection, falling back to clipboard
  text. Dictto reads the clipboard without writing to it. Long passages play
  in sequential chunks. Press the same shortcut during synthesis or playback
  to cancel.

The current sample is stored as `voice-sample.wav` beside Dictto's database
and uploaded with each clone request. Engine-voice reading needs no sample.

In **Speech**, select **Open audio folder** to see the WAV files in
`%LOCALAPPDATA%\com.dictto.app\Speech Audio`. Dictto keeps up to six complete
synthesis results across speech-to-speech and both text-reading modes.
Each result is one WAV, including longer passages. Reusing a result refreshes
its place in the cache. A new result replaces the least recently used one
when the folder is full. Transcripts and selected or clipboard text are not
saved alongside these files.

### Send speech to Discord

Install the Dictto Microphone capture driver and select **Default** as the input
in Discord. Leave your normal microphone selected as the Windows default
communications input. Enable **Speech → Send speech to Dictto Microphone** in
Dictto. The existing speech shortcuts then send generated or cached speech to
that input. Turning the checkbox off returns speech to local playback.

Dictto changes only the communications capture role, once synthesized audio is
ready. It waits up to eight seconds for an app to open the virtual input before
sending the first sample. When speech drains, is cancelled, or the desktop
locks or disconnects, Dictto restores the previous input if its own endpoint
is still selected. A manual input change takes precedence. The previous
endpoint ID is journaled for recovery after an interrupted exit. If the driver
or an existing communications input is unavailable in this session, Dictto
reports the problem without rerouting the microphone.

Developer checks:

```powershell
cargo test --workspace
```

`dictto-native.exe --smoke-test` opens actual settings without starting the microphone,
global hotkeys or model warmup. It reads existing settings; using Save still changes
them. Add `--autostart` to verify tray-only startup without recording services.

The shared logic lives in `crates/dictto-core` (speech behind its `speech` feature) and
the Win32 services in `crates/dictto-windows`; the native host drives `dictto_core::Runtime`
through an event sink that posts Win32 messages, on a two-worker Tokio runtime. The hidden tray host remains alive without settings.
Changes to shared services must be checked in both variants. Measure working set,
private committed memory, threads and on-disk size separately. `--smoke-test`
comparisons exclude recording hooks, device monitoring and model warmup.
