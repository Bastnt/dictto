import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';

import {SHELL_NAME, SHELL_PATH, SHELL_XML} from './dbus.js';
import {ClipboardTokens, pickRestoreMime} from './state.js';

// A source application that never answers a read must not stall the paste for the
// whole D-Bus timeout; the token then restores nothing.
const SAVE_TIMEOUT_MS = 2000;

const ACCESS_DENIED = 'org.freedesktop.DBus.Error.AccessDenied';
const INVALID_ARGS = 'org.freedesktop.DBus.Error.InvalidArgs';
const FAILED = 'org.freedesktop.DBus.Error.Failed';

// Serves com.dictto.Shell1. On Wayland only the compositor can read the clipboard
// without focus, so this hands it to the daemon and nobody else.
export class ClipboardService {
    // isAllowed(sender) says whether a unique bus name may use the clipboard.
    constructor(isAllowed) {
        this._isAllowed = isAllowed;
        this._clipboard = St.Clipboard.get_default();
        this._tokens = new ClipboardTokens();
        // Calls waiting on an asynchronous clipboard read; answered once, by whichever
        // of the read, the timeout or destroy() comes first.
        this._pending = new Set();
        this._exported = Gio.DBusExportedObject.wrapJSObject(SHELL_XML, this);
        this._exported.export(Gio.DBus.session, SHELL_PATH);
        this._ownerId = Gio.bus_own_name(Gio.BusType.SESSION, SHELL_NAME,
            Gio.BusNameOwnerFlags.NONE, null, null,
            () => console.warn(`Dictto: lost or could not own ${SHELL_NAME}; pasting is unavailable`));
    }

    destroy() {
        for (const call of [...this._pending])
            this._settle(call, () => call.invocation.return_dbus_error(FAILED, 'Dictto extension disabled'));
        Gio.bus_unown_name(this._ownerId);
        this._exported.unexport();
        this._exported = null;
        this._tokens.clear();
    }

    _refuse(invocation) {
        if (this._isAllowed(invocation.get_sender()))
            return false;
        invocation.return_dbus_error(ACCESS_DENIED, 'Only the Dictto daemon may use this service');
        return true;
    }

    _track(invocation) {
        const call = {invocation, done: false, timeoutId: 0};
        this._pending.add(call);
        return call;
    }

    _settle(call, reply) {
        if (call.done)
            return;
        call.done = true;
        if (call.timeoutId)
            GLib.source_remove(call.timeoutId);
        this._pending.delete(call);
        reply();
    }

    // Only the mimetype a restore can use is read: St sets one mimetype per selection.
    SaveClipboardAsync(_params, invocation) {
        if (this._refuse(invocation))
            return;
        const type = St.ClipboardType.CLIPBOARD;
        const mimetype = pickRestoreMime(this._clipboard.get_mimetypes(type));
        const reply = entry => () =>
            invocation.return_value(new GLib.Variant('(u)', [this._tokens.save(entry)]));
        if (mimetype === null) {
            reply(null)();
            return;
        }
        const call = this._track(invocation);
        call.timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SAVE_TIMEOUT_MS, () => {
            call.timeoutId = 0;
            this._settle(call, reply(null));
            return GLib.SOURCE_REMOVE;
        });
        this._clipboard.get_content(type, mimetype, (_clipboard, bytes) => {
            const entry = bytes && bytes.get_size() > 0 ? {mimetype, bytes} : null;
            this._settle(call, reply(entry));
        });
    }

    RestoreClipboardAsync([token], invocation) {
        if (this._refuse(invocation))
            return;
        if (!this._tokens.has(token)) {
            invocation.return_dbus_error(INVALID_ARGS, `Unknown clipboard token ${token}`);
            return;
        }
        // An empty saved clipboard cannot be re-created: St has no way to clear it.
        const entry = this._tokens.take(token);
        if (entry)
            this._clipboard.set_content(St.ClipboardType.CLIPBOARD, entry.mimetype, entry.bytes);
        invocation.return_value(null);
    }

    SetTextAsync([text], invocation) {
        if (this._refuse(invocation))
            return;
        this._clipboard.set_text(St.ClipboardType.CLIPBOARD, text);
        invocation.return_value(null);
    }

    GetTextAsync([which], invocation) {
        if (this._refuse(invocation))
            return;
        let type;
        if (which === 'clipboard')
            type = St.ClipboardType.CLIPBOARD;
        else if (which === 'primary')
            type = St.ClipboardType.PRIMARY;
        else {
            invocation.return_dbus_error(INVALID_ARGS, `Unknown selection "${which}"`);
            return;
        }
        const call = this._track(invocation);
        this._clipboard.get_text(type, (_clipboard, text) => {
            this._settle(call, () =>
                invocation.return_value(new GLib.Variant('(bs)', [text !== null, text ?? ''])));
        });
    }
}
