# Fedora GNOME bring-up

Written for a Claude CLI session on the owner's Fedora machine. Everything in `apps/gnome`, `apps/gnome-settings` and `apps/gnome/extension` was developed on Windows/WSL with unit tests and fakes only. Nothing GNOME-facing (Shell extension, mutter, PipeWire, AT-SPI, logind, uinput, libadwaita on a real display) has ever run. Expect bugs; fix them in small commits (`subject: description`, no co-author line).

## Prerequisites

```bash
sudo dnf install gcc clang-devel pkgconf gtk4-devel libadwaita-devel pipewire-devel \
  dbus-devel openssl-devel gjs jq git
# Rust: either `sudo dnf install rust cargo` or rustup (https://rustup.rs)
```

## Build and install

```bash
scripts/install-gnome.sh        # builds release, installs to ~/.local, one-time sudo for uinput
```

Then log out and back in (needed for the `input` group and for GNOME Shell to see the new extension on Wayland), and:

```bash
gnome-extensions enable dictto@dictto.app
dictto-gnome                     # or launch Dictto from the app grid
```

Re-running the script after a change stops a running daemon (D-Bus `Quit`), replaces the binaries and restarts it. Extension JS changes need a new login session (Wayland cannot reload the shell).

For development builds: `cargo build -p dictto-gnome -p dictto-gnome-settings` and `cargo test -p dictto-gnome -p dictto-gnome-settings`. `scripts/wsl-build-gnome.sh` is Ubuntu/WSL only.

## Watching logs

- Daemon: run `dictto-gnome` in a terminal (stderr), or `journalctl --user -f` when started from autostart or the app grid.
- Extension: `journalctl -f -o cat /usr/bin/gnome-shell`
- D-Bus: `dbus-monitor --session "sender=com.dictto.App"` and `gdbus introspect --session --dest com.dictto.App --object-path /com/dictto/App`
- Extension syntax check: `apps/gnome/extension/check.sh`; unit tests: `apps/gnome/extension/test.js` (gjs).

## Never run on real GNOME

All of Tasks 2-9 beyond unit tests: logind session tracking and LockedHint, the uinput keyboard grab and virtual keyboard, PipeWire capture/devices/sounds/virtual microphone, AT-SPI selection reads, the Shell extension (indicator, overlay, St.Clipboard), the D-Bus services, and the libadwaita settings window.

## Checklist (in this order)

1. **Extension loads.** After login and `gnome-extensions enable dictto@dictto.app`: `gnome-extensions info dictto@dictto.app` says ENABLED and the panel indicator is visible. Smoke points: `St.Clipboard` get/set, DrawingArea scale, overlay buttons not stealing focus, input region released when hidden, symbolic icon recolour in light and dark.
2. **Daemon Status.** `gdbus call --session --dest com.dictto.App --object-path /com/dictto/App --method org.freedesktop.DBus.Properties.GetAll com.dictto.App1` reports no problems. A second `dictto-gnome` instance must exit without grabbing keyboards.
3. **Permissions.** `id` lists `input`; `ls -l /dev/uinput` is `crw-rw---- root input`; the `uinput` module is loaded. When denied, the daemon must report a clear problem. A group change needs a new process; a `user@` manager that survives logout can keep a stale daemon, so check `pgrep -a dictto` after login.
4. **Ctrl+Super dictation and paste.** Hold Ctrl+Super, speak, release; text is pasted at the cursor. Check whether Super alone opens Activities during or after the chord. If it does, change the Dictto default hotkey, not GNOME's `overlay-key`.
5. **Short tap retry.** A very short press shows the retry/too-short behaviour and recovers without stuck state.
6. **Overlay states.** Recording (waveform), processing, error text (friendly errors), idle hidden; correct monitor; multi-monitor if available.
7. **Settings window.** `dictto-gnome --settings` opens `dictto-gnome-settings`. Walk all tabs: General, Dictation, Voice, Shortcuts, Connection. Every change saves at once (typed text on Enter, focus-out or close) with no Save buttons; with a key, the model lists and the Connection status fill in the background, and Refresh asks the endpoint again. Shortcut recording works by physical key while the daemon's chords are suspended; Escape cancels, Backspace clears. AZERTY note: keys are named by US-layout position, so `A` on AZERTY shows as `Q`. Saved shortcuts take effect without restart. With the daemon stopped, the Start Dictto page appears.
8. **Microphones.** The list includes virtual and filter sources; selecting one records from it; unplug/replug updates the list.
9. **Speech shortcuts.** Select text in a GTK4 app and in Firefox, press the read-selection shortcut; AT-SPI should supply the text. Verify the fallback (primary selection, then clipboard) when AT-SPI yields nothing.
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
