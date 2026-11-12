import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import St from 'gi://St';

import {SHELL_NAME, SHELL_PATH, SHELL_XML} from './dbus.js';
import {ClipboardTokens, pickRestoreMime, savableMimetypes} from './state.js';

// A source application that never answers a read must not stall the paste for the
// whole D-Bus timeout; whatever arrived by then is saved.
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
        this._saves = new Set();
        this._exported = Gio.DBusExportedObject.wrapJSObject(SHELL_XML, this);
        this._exported.export(Gio.DBus.session, SHELL_PATH);
        this._ownerId = Gio.bus_own_name(Gio.BusType.SESSION, SHELL_NAME,
            Gio.BusNameOwnerFlags.NONE, null, null, null);
    }

    destroy() {
        for (const save of this._saves) {
            GLib.source_remove(save.timeoutId);
            save.done = true;
            save.invocation.return_dbus_error(FAILED, 'Dictto extension disabled');
        }
        this._saves.clear();
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

    SaveClipboardAsync(_params, invocation) {
        if (this._refuse(invocation))
            return;
        const type = St.ClipboardType.CLIPBOARD;
        const mimetypes = savableMimetypes(this._clipboard.get_mimetypes(type));
        const read = new Array(mimetypes.length).fill(null);
        const save = {invocation, done: false, timeoutId: 0};
        let remaining = mimetypes.length;
        const finish = () => {
            if (save.done)
                return;
            save.done = true;
            if (save.timeoutId)
                GLib.source_remove(save.timeoutId);
            this._saves.delete(save);
            const entries = read.filter(entry => entry !== null);
            invocation.return_value(new GLib.Variant('(u)', [this._tokens.save(entries)]));
        };
        if (remaining === 0) {
            finish();
            return;
        }
        save.timeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SAVE_TIMEOUT_MS, () => {
            save.timeoutId = 0;
            finish();
            return GLib.SOURCE_REMOVE;
        });
        this._saves.add(save);
        mimetypes.forEach((mimetype, index) => {
            this._clipboard.get_content(type, mimetype, (_clipboard, bytes) => {
                if (save.done)
                    return;
                if (bytes && bytes.get_size() > 0)
                    read[index] = {mimetype, bytes};
                remaining -= 1;
                if (remaining === 0)
                    finish();
            });
        });
    }

    RestoreClipboardAsync([token], invocation) {
        if (this._refuse(invocation))
            return;
        const entries = this._tokens.take(token);
        if (entries === null) {
            invocation.return_dbus_error(INVALID_ARGS, `Unknown clipboard token ${token}`);
            return;
        }
        // An empty saved clipboard cannot be re-created: St has no way to clear it.
        const mimetype = pickRestoreMime(entries.map(entry => entry.mimetype));
        const entry = entries.find(e => e.mimetype === mimetype);
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
        this._clipboard.get_text(type, (_clipboard, text) => {
            invocation.return_value(new GLib.Variant('(bs)', [text !== null, text ?? '']));
        });
    }
}
