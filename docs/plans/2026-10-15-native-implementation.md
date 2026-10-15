# Windows 11 native implementation plan

User approved the separate native build; visual identity need not match the web UI.
Use the preceding Windows native proposal as the spec. Work on main per user rules.

Architecture: a separate Rust `dictto-native` executable in `apps/native`, using
WinUI 3 through pinned windows-reactor 0.100 and a native Win32 recording overlay.
Share the existing speech/settings services through a narrow host bridge instead
of duplicating the pipeline. The native dependency tree must contain no Tauri or
WebView renderer. Native controls follow system light/dark preferences.

- [x] Update installed Dictto-Fleet executable from the verified release build,
  keep the prior binary in build/, and compare SHA-256.
- [x] Runtime: expose the existing recorder/pipeline/hotkey/settings/keyring through
  a native host, preserving current Tauri adapter behavior and backend tests.
- [x] UI: General/Audio/API pages, native switches/forms/language and microphone
  selection/key management, tray lifetime, system theme and visible failures.
- [x] Overlay: nonactivating topmost Win32 window, waveform/state/error display,
  stop/cancel buttons and safe cross-thread messages.
- [x] Integration: compile and exercise both builds, verify native UI startup,
  package native runtime dependencies, install separately under Dictto-Native,
  add a cross-variant single-instance guard, review and commit.

Runtime contract consumed by UI: NativeRuntime::new(PathBuf, event_sink) -> Result;
get_settings/set_setting, has_key/key_hint/save_key/remove_key, microphones/current_microphone,
set_hotkey_capture(bool), start_recording/stop_recording/cancel_recording. Events retain
the existing event names and JSON payloads. Runtime operations return explicit errors;
UI state changes only after successful persistence. No credential values enter logs.

Validation: Rust tests/check for Tauri and native; release executables; actual WinUI
startup and theme behavior; overlay focus/hit testing; no WebView2 child process in
the native variant. Preserve user data and keyring; never overwrite a live database
with a test fixture. A smoke-test mode may suppress microphone/model calls.

Validation: native library 42 tests plus 2 overlay tests passed; desktop library
41 tests and frontend 16 tests passed. Both release executables built and were
copied into separate Programs folders with matching SHA-256. Tauri is absent from
the native dependency graph. Actual WinUI startup, existing settings and the compact
General layout were inspected on Windows. The overlay test creates a real HWND,
verifies NOACTIVATE, foreground preservation and stop/cancel routing.

Review fixes: hidden autostart, early DPI awareness, Explorer tray recovery and
single-owner overlay creation state. Live microphone/model calls, live system-theme
switching and exhaustive tray interaction were not exercised. The user stopped
Computer Use with Escape during the final UI check; the native smoke window was
left untouched with recording services disabled.
