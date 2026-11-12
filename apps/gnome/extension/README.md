# Dictto GNOME Shell extension

UI half of `dictto-gnome` on GNOME 47-49: panel indicator, recording pill and the
`com.dictto.Shell1` clipboard service. It holds no business logic; everything goes
through the daemon over D-Bus (`../dbus/*.xml`).

## Lock screen

`metadata.json` has no `session-modes`, so the extension is disabled on the lock
screen on purpose: `com.dictto.Shell` vanishes and the daemon cannot paste into the
unlock dialog. Do not add `unlock-dialog`.

## Development only

`check.sh` (metadata, module syntax, imports, XML) and `test.js` (`gjs -m test.js`,
pure logic in `state.js`) are dev tools. Do not install them with the extension;
`test.js` also reads `../dbus`.
