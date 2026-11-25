# Fedora GNOME bring-up

Written for a Claude CLI session on the owner's Fedora machine. Everything in `apps/gnome`, `apps/gnome-settings` and `apps/gnome/extension` was developed on Windows/WSL with unit tests and fakes only. Nothing GNOME-facing (Shell extension, mutter, PipeWire, AT-SPI, logind, uinput, libadwaita on a real display) has ever run. Expect bugs; fix them in small commits (`subject: description`, no co-author line).

## Prerequisites

```bash
sudo dnf install clang-devel pkgconf gtk4-devel libadwaita-devel pipewire-devel \
  dbus-devel openssl-devel gjs jq git rust cargo
```

With rustup, omit `rust` and `cargo` from the DNF command. On Fedora 45,
`clang-devel` brings in Clang and its GCC package dependencies.

## Build and install

```bash
CC=clang RUSTFLAGS="-C linker=clang" scripts/install-gnome.sh
```

This selects Clang for native C compilation and as Rust's linker driver.
The script builds a release, installs to `~/.local` and uses `sudo` for
the one-time uinput setup.

Then log out and back in (needed for the `input` group and for GNOME Shell to see the new extension on Wayland), and:

```bash
gnome-extensions enable dictto@dictto.app
dictto-gnome                     # or launch Dictto from the app grid
```

Re-running the script after a change stops a running daemon (D-Bus `Quit`), replaces the binaries and restarts it. Extension JS changes need a new login session (Wayland cannot reload the shell).

If the user manager survives the logout, GNOME Shell and apps it launches may still lack the new `input` group. Compare `id -G "$USER"` with the `Groups:` line in `/proc/$(pgrep -x gnome-shell)/status`. Reboot if the running session still has the old groups.

For development builds: `cargo build -p dictto-gnome -p dictto-gnome-settings` and `cargo test -p dictto-gnome -p dictto-gnome-settings`. `scripts/wsl-build-gnome.sh` is Ubuntu/WSL only.

## Watching logs

- Daemon: run `dictto-gnome` in a terminal (stderr), or `journalctl --user -f` when started from autostart or the app grid.
- Extension: `journalctl -f -o cat /usr/bin/gnome-shell`
- D-Bus: `dbus-monitor --session "sender=com.dictto.App"` and `gdbus introspect --session --dest com.dictto.App --object-path /com/dictto/App`
- Extension syntax check: `apps/gnome/extension/check.sh`; unit tests: `apps/gnome/extension/test.js` (gjs).

## Fedora 45 bring-up status

- The owner’s desktop runs GNOME Shell 51.1 on Wayland with an active logind session and a working PipeWire connection. Shell extension version validation is enabled, so the extension metadata includes GNOME 51.
- The extension syntax, import, D-Bus XML and icon checks pass on Fedora 45. All 21 GJS tests pass.
- The release build and installation completed on Fedora 45. Both installed binaries match the release artifacts, and the installed extension metadata matches the checkout.
- The installer added the user to the `input` group and set `/dev/uinput` to mode `0660` with group `input`. The surviving user manager kept its old groups after the first relogin. A reboot refreshed the daemon's groups, and it detected one keyboard.
- GNOME Shell 51 rejected the overlay's `affectsInputRegion` chrome argument. The corrected extension is active after reboot, the panel icon is visible and the Shell clipboard D-Bus service is registered.
- The daemon registered `com.dictto.App`, connected to PipeWire, followed the active logind session and opened Settings. After reboot the D-Bus Status was `ok`. An idle sample showed 17,116 KiB RSS, seven threads and zero libgtk mappings. With Settings open, its process showed 103,232 KiB RSS.
- With the Dictto virtual source present, Codex dictation stalled. PipeWire still showed the built-in mic as the default and logged a Chromium input stream timeout. Turning off **Voice → Send speech to Dictto Microphone** removed the virtual source. Codex then transcribed while Dictto stayed running. The failed capture's target and the exact cause remain unknown. Speech routing with the virtual source is pending.
- With the virtual source off, the owner confirmed that Ctrl+Super dictation pasted into an editor. The test captured from the built-in microphone. Transcription completed about 2 seconds after release, and cleanup took another 46 seconds before paste.
- The Settings launcher now matches the GTK application ID (`com.dictto.Settings`). The installed desktop entry resolves to the Dictto name and icon. Visual confirmation in Alt+Tab is pending.
- On the live session bus, the panel's Settings action opened the Settings window and the Quit action closed both the window and the daemon.
- The owner verified both holding and double-tapping a speech recording shortcut on the desktop.
- The app launcher started the daemon and opened Settings from a stopped state. A second launch reused both running processes.

## Checklist (in this order)

1. **Extension loads.** After login and `gnome-extensions enable dictto@dictto.app`: `gnome-extensions info dictto@dictto.app` says ENABLED and the panel indicator is visible. Smoke points: `St.Clipboard` get/set, DrawingArea scale, overlay buttons not stealing focus, input region released when hidden, symbolic icon recolour in light and dark.
2. **Daemon Status.** `gdbus call --session --dest com.dictto.App --object-path /com/dictto/App --method org.freedesktop.DBus.Properties.GetAll com.dictto.App1` reports no problems. A second `dictto-gnome` instance must exit without grabbing keyboards.
3. **Permissions.** `id` lists `input`; `ls -l /dev/uinput` is `crw-rw---- root input`; the `uinput` module is loaded. When denied, the daemon must report a clear problem. A group change needs a new process; a `user@` manager that survives logout can keep a stale daemon, so check `pgrep -a dictto` after login.
4. **Ctrl+Super dictation and paste.** Hold Ctrl+Super, speak, release; text is pasted at the cursor. Check whether Super alone opens Activities during or after the chord. If it does, change the Dictto default hotkey, not GNOME's `overlay-key`.
5. **Short tap retry.** A very short press shows the retry/too-short behaviour and recovers without stuck state.
6. **Overlay states.** Recording (waveform), processing, error text (friendly errors), idle hidden; correct monitor; multi-monitor if available.
7. **Settings window.** Launching Dictto from the app grid starts the daemon if needed and opens Settings. Running `dictto-gnome --settings` opens the Settings process directly, showing the Start Dictto page if the daemon is stopped. Walk all tabs: General, Dictation, Voice, Shortcuts, Connection. Every change saves at once (typed text on Enter, focus-out or close) with no Save buttons; with a key, the model lists and the Connection status fill in the background, and Refresh asks the endpoint again. Shortcut recording works by physical key while the daemon's chords are suspended; Escape cancels, Backspace clears. AZERTY note: keys are named by US-layout position, so `A` on AZERTY shows as `Q`. Saved shortcuts take effect without restart.
8. **Microphones.** The list includes virtual and filter sources; selecting one records from it; unplug/replug updates the list.
9. **Speech shortcuts.** For microphone samples, playback samples and speech-to-speech, hold the shortcut to record and release to stop. Double-tap to keep recording and press again to stop. A lone tap discards the recording. Select text in a GTK4 app and in Firefox and press a read shortcut to check AT-SPI. Verify the fallback (primary selection, then clipboard) when AT-SPI yields nothing.
10. **Dictto Microphone.** `wpctl status` shows the virtual "Dictto Microphone" node while routing; the default source switches to it during playback and is restored afterwards (also after killing the daemon: the per-user journal `microphone-route.json` restores it on next start); the controls are locked during playback.
11. **Autostart.** Enable it in settings, check the `~/.config/autostart/` entry has a clean `Exec=` path (no "(deleted)"), log out and in, and confirm the daemon starts.
12. **Idle footprint.** With the daemon idle for a minute: `ps -o rss,nlwp -p $(pgrep -x dictto-gnome)`, and `grep -c libgtk /proc/$(pgrep -x dictto-gnome)/maps` must print 0. Also measure `dictto-gnome-settings` RSS while open. Report all numbers.

## Known follow-ups

From the task ledger; none block bring-up.

- Ctrl+V does not paste into GNOME Terminal (needs Ctrl+Shift+V). Windows parity kept; consider detecting terminals.
- Keyboard: a device that opens but cannot be read reopens in a tight loop; no `/dev/input` watch while access is denied; a dead session path is retried forever.
- Keyboard: Active is fed before LockedHint is read on subscribe (brief unlocked window if the read fails); an Unlock hint enables input even if GNOME ignores it.
- Input devices are still read while suspended (events are dropped in core).
- Status priority puts Input above Audio; `JOURNAL_STREAM` is only checked for presence; `xdg_base` falls back to "/" when HOME is unset.
- Autostart opens settings on problems at every login (Windows parity); revisit.
- Audio: `AutoDetect(None)` falls back to the lowest-id mic when the default is `dictto_microphone`; loopback gap heuristic is mostly inert; one stream per speech chunk (gaps, same as Windows); a blocking WAV open in an async fn; start/stop sounds may overlap.
- Device monitors keep the last device list while PipeWire is down.
- Speech: no virtual-node re-creation after a PipeWire restart until the next read; `wait_ready` counts any link (level meters); `with_speech` RSS and startup unmeasured; one a11y bus connection per read; no `priority.session` on the node (harmless); legacy journal deleted on a transient recovery failure and legacy `.pending` files left behind.
- D-Bus: `KeyHint` swallows keyring errors into "" (no key vs locked keyring indistinguishable); a hung keyring call blocks later settings calls (add a comment); log `EventReceiver::dropped()` periodically; when the event queue is full, an incoming waveform evicts a state event (should drop the waveform instead).
- Extension: `GetText` has no timeout; possible spurious "name lost" warning on disable; paste status stays stale until the next paste after the user fixes the extension; AccessDenied window right after unlock until `GetNameOwner` resolves.
- Settings: `release_capture` blocks the GTK thread up to 1 s on close; Status is read once per load (no PropertiesChanged); client-side shortcut check is stricter than core when speech is unconfigured; the Start Dictto error text assumes journal logging.
