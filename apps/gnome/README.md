# Dictto for GNOME

Voice dictation for Fedora GNOME on Wayland. Hold Ctrl+Super, speak, release: the cleaned text is pasted at the cursor. It also reads the selected text aloud and can route speech to a virtual microphone.

- `dictto-gnome`: background daemon (audio, hotkeys, transcription, paste), D-Bus name `com.dictto.App`.
- `dictto-gnome-settings`: GTK4/libadwaita settings window, started by `dictto-gnome --settings`.
- `extension/`: GNOME Shell extension `dictto@dictto.app` (panel indicator, recording overlay, clipboard access).

## Install

From the repository root, with the build dependencies from [FEDORA-BRINGUP.md](FEDORA-BRINGUP.md):

```bash
scripts/install-gnome.sh
```

The script installs into `~/.local` and asks for sudo once to grant `/dev/uinput` access (udev rule plus the `input` group).

**Log out and log back in afterwards.** On Wayland, group membership and a newly installed shell extension only take effect in a new session. Then run `gnome-extensions enable dictto@dictto.app` and start Dictto from the app grid.

This code has not yet run on a real GNOME session; [FEDORA-BRINGUP.md](FEDORA-BRINGUP.md) is the validation checklist and the list of known follow-ups.
