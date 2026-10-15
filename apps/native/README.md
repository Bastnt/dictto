# Dictto Native

A separate Rust/WinUI 3 Windows 11 application. It uses actual Windows controls,
system light/dark theming and a Win32 recording overlay. No HTML, JavaScript or
WebView renderer is used by this executable.

Build and install alongside the web variant from the repository root:

```powershell
pwsh -NoProfile -File scripts/build-native.ps1
```

This installs to `%LOCALAPPDATA%\Programs\Dictto-Native`, including the pinned
Windows App SDK runtime files, and adds a Start menu shortcut. The SDK DLLs and
resource folders must accompany `dictto-native.exe`; copying only the executable
will not produce a runnable self-contained install.

The Tauri/JavaScript build remains in `Dictto-Fleet`. Quit one variant from its tray
before opening the other: both use the same database, keyring and hotkey pipeline,
and a cross-variant session guard prevents simultaneous recording. No settings or
credentials are copied into the installation folder. Autostart uses whichever
variant last enabled/start-restored that setting.

Native settings use explicit save controls for editable values. Closing settings
hides the window; use the tray to reopen settings or quit. The recording overlay
does not activate its window. Waveforms update on incoming audio events; there is
no continuous animation loop while idle or processing.

Developer checks:

```powershell
cargo test --manifest-path apps/native/Cargo.toml
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
```

`dictto-native.exe --smoke-test` opens actual settings without starting the microphone,
global hotkeys or model warmup. It reads existing settings; using Save still changes
them. Add `--autostart` to verify tray-only startup without recording services.

WinUI is pinned to `windows-reactor` and `windows-reactor-setup` 0.100.0. The shared
speech modules live under `apps/desktop/src-tauri/src/services`; the native host maps
their events/state access onto a Tokio runtime and typed UI messages. Changes to
those services must be checked in both variants. Native deployment has a larger
on-disk SDK footprint than the WebView-based build; measure CPU/private memory
separately instead of inferring them from installer size.
