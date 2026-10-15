# Lightweight native UI migration

The approved approach replaces the WinUI settings frontend with Windows common
controls called through Rust's `windows` crate. Keep the shared speech pipeline,
Credential Manager integration, database, tray and existing Win32/GDI overlay.
Use compact General, Audio and API pages with an additional filtered language
selector. Follow system light/dark colours with a simpler classic appearance.

Implemented:

- Removed windows-reactor, XAML and Windows App SDK build/runtime dependencies.
- Embedded the common-controls v6 manifest and application icon.
- Created settings only on demand; close destroys controls and font resources.
- Kept the tray host and recording services independent of settings lifetime.
- Used posted event messages instead of a polling UI receiver.
- Limited the async runtime to two workers.
- Installed only the executable; retained the previous SDK directory in a dated
  sibling backup. Settings and credentials remain outside the install folder.

Validation on this Windows machine (2026-10-15):

| Launch mode | Total working set | Private committed memory | Threads |
| --- | ---: | ---: | ---: |
| Previous WinUI, --smoke-test --autostart | 100.0 MB | 66.6 MB | 84 |
| Win32, --smoke-test --autostart | 12.3 MB | 2.1 MB | 7 |
| Win32, normal services with --autostart | 16.3 MB | 2.7 MB | 11 |

These are point-in-time Get-Process measurements in MiB (PowerShell 1MB), not the
unlabelled Task Manager column in the user's screenshot. The comparison mode
excludes hotkeys, device monitoring and model warmup. UI Automation inspections
and opening settings can increase process memory and thread counts.

The installation decreased from 71,064,573 bytes across 208 files to one executable
of approximately 8.2 MB. No XAML or WebView module was present in the running
Win32 process. The previous installation is retained as a backup, so its disk
space is not reclaimed automatically.

All 46 native tests pass: 43 library tests and 3 window/overlay tests. The new UI
test checks saving actual settings to a temporary database, invalid numeric
values, draft retention across tabs, filtered multi-language selection, window
resource release and reopening through the tray message handler. The runtime
test checks that exactly two async workers are configured. General, API and Audio
pages were inspected in the installed application in dark mode. The installed
application was then restarted with normal services in tray-only mode.

Live theme switching, cross-monitor DPI movement and actual microphone-to-paste
dictation were not manually exercised in this migration. Existing mocked HTTP
pipeline tests pass. No real dictation was submitted during UI verification.
