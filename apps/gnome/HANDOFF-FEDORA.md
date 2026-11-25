# Handoff: install and test Dictto on Fedora GNOME

You are a Claude CLI session on the owner's Fedora PC (GNOME, Wayland). Your job:
install the GNOME build of Dictto and make it work end to end. Aligning it with
new Windows features is not your job: that happens on the Windows side and
reaches you through `git pull`. Commit fixes on `main` with the form
`subject: description` and no co-author line (owner's rule).

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

## Install and bring-up

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

## Ground rules

- Before starting, `git pull` so you test the latest GNOME code.
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
