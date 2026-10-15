# Proposal: separate Windows 11 native build

Status: implemented as the separate `apps/native` application. See its README for
build/install instructions. The JavaScript/CSS Tauri variant remains available.
No claim of lower RAM or power use without benchmarks.

## UI and theme

Use actual WinUI 3 controls for a Windows Settings-like experience: a NavigationView
with General, Audio and API pages; ToggleSwitch for boolean settings; TextBox and
PasswordBox for endpoint/models/key; ContentDialog for languages and microphones;
standard title bar and optional Mica backdrop. Follow the system application theme,
including live light/dark changes and high contrast; use theme brushes rather than
hard-coded text/background colors. Keep a functional opaque fallback for disabled
transparency or unsupported backdrop settings.

Rust-first candidate: Microsoft's `windows-reactor`, which describes native WinUI 3
controls in Rust. It is currently preview, so pin the exact tested version and Windows
App SDK runtime. Default deployment requires that runtime; self-contained deployment
must account for its installer size. It is not a zero-runtime or minimum-size Win32
build. If the preview API proves unsuitable, a small C# WinUI 3 host with a stable Rust
service boundary is the fallback; do not couple speech services to XAML UI types.

Sources: [Reactor API and deployment](https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-reactor.md),
[Microsoft theme guidance](https://github.com/MicrosoftDocs/windows-dev-docs/blob/docs/hub/apps/develop/ui/theming.md),
[Windows app theme behavior](https://github.com/MicrosoftDocs/windows-dev-docs/blob/docs/hub/apps/desktop/modernize/ui/apply-windows-themes.md).

## Shared service boundary

First extract existing audio, API/transcription/cleanup, settings/history and keyring
operations into a shared Rust crate. Replace service-level Tauri AppHandle/State/Emitter
dependencies with explicit configuration/state and a typed event sink. The Tauri adapter
retains existing commands/events; the native adapter routes those events into the UI
thread. Keep keys in the same keyring and settings/history in the same application-data
location, with a single-instance policy to prevent both variants recording concurrently.

Use separate app entry points and package names/artifact filenames. Reuse resources
and icons. Preserve tray, autostart, hotkeys, clipboard restoration, device switching,
custom API endpoints, cleanup, warmup and error behavior in both variants. Replace
Tauri's updater/process/window bindings explicitly in the native host.

## First deliverable and decision gate

Build a small standalone feasibility executable before migrating the full UI:

1. Native General/API page with working toggles and system theme updates.
2. Tray close/reopen lifecycle and a background task posting events to the UI thread.
3. A tiny separate Win32 overlay, custom drawn with Direct2D or a layered bitmap,
   shown without activation and absent from the taskbar. WinUI settings can own an
   ordinary focusable window; the overlay must not take keyboard focus.
4. Verify recording/cancel/stop interactions while typing in another app, DPI and
   multiple monitors, dark/light switching, keyboard navigation and Narrator.

Win32's WS_EX_NOACTIVATE specifically prevents click activation. Topmost, transparency,
click-through hit testing and focus behavior require separate checks; setting window
opacity or hiding the taskbar entry alone is insufficient.
[Window styles](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles).

Compare release builds on this PC in tray-only, settings-open, recording and processing
states: process-tree private bytes, idle CPU/wakeups, active CPU/GPU, startup and install
size. Only proceed to the second complete build once the focus test passes and the
measured result justifies maintaining a second frontend.
