import Gio from 'gi://Gio';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {ClipboardService} from './clipboard.js';
import {APP_NAME, APP_PATH, APP_XML} from './dbus.js';
import {Indicator} from './indicator.js';
import {Overlay} from './overlay.js';

// Draws and relays only: every decision stays in the dictto-gnome daemon.
export default class DicttoExtension extends Extension {
    enable() {
        this._appOwner = null;
        this._proxy = null;
        this._proxySignals = [];
        this._cancellable = new Gio.Cancellable();

        this._overlay = new Overlay({
            onStop: () => this._call('BarStop'),
            onCancel: () => this._call('BarCancel'),
        });
        this._indicator = new Indicator(`${this.path}/icons/dictto-symbolic.svg`, {
            onSettings: () => this._call('ShowSettings'),
            onQuit: () => this._call('Quit'),
        });
        Main.panel.addToStatusArea(this.uuid, this._indicator);
        this._indicator.setShown(false);

        this._clipboard = new ClipboardService(
            sender => this._appOwner !== null && sender === this._appOwner);

        // Built here, not at module scope: extensions do no work until enable() (review rule).
        const AppProxy = Gio.DBusProxy.makeProxyWrapper(APP_XML);
        // DO_NOT_AUTO_START: the extension must never launch the daemon by itself.
        new AppProxy(Gio.DBus.session, APP_NAME, APP_PATH, (proxy, error) => {
            if (error) {
                if (!error.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED))
                    console.warn(`Dictto: cannot create the daemon proxy: ${error.message}`);
                return;
            }
            this._attach(proxy);
        }, this._cancellable, Gio.DBusProxyFlags.DO_NOT_AUTO_START);

        this._watchId = Gio.bus_watch_name(Gio.BusType.SESSION, APP_NAME,
            Gio.BusNameWatcherFlags.NONE,
            (_connection, _name, owner) => this._setOwner(owner),
            () => this._setOwner(null));
    }

    disable() {
        this._cancellable.cancel();
        this._cancellable = null;
        Gio.bus_unwatch_name(this._watchId);
        this._watchId = 0;
        for (const [connect, id] of this._proxySignals) {
            if (connect === 'signal')
                this._proxy.disconnectSignal(id);
            else
                this._proxy.disconnect(id);
        }
        this._proxySignals = [];
        this._proxy = null;
        this._appOwner = null;
        this._clipboard.destroy();
        this._clipboard = null;
        this._overlay.destroy();
        this._overlay = null;
        this._indicator.destroy();
        this._indicator = null;
    }

    _attach(proxy) {
        this._proxy = proxy;
        this._proxySignals.push(['signal', proxy.connectSignal('Event',
            (_proxy, _sender, [name, payload]) => this._overlay.handleEvent(name, payload))]);
        this._proxySignals.push(['gobject', proxy.connect('g-properties-changed',
            () => this._indicator.setStatus(proxy.Status))]);
        this._indicator.setStatus(proxy.Status);
    }

    _setOwner(owner) {
        this._appOwner = owner;
        const present = owner !== null;
        this._indicator.setShown(present);
        this._indicator.setStatus(present ? this._proxy?.Status : 'ok');
        this._overlay.setPresent(present);
    }

    _call(method) {
        if (!this._proxy || this._appOwner === null)
            return;
        this._proxy[`${method}Remote`]((_result, error) => {
            if (error)
                console.warn(`Dictto: ${method} failed: ${error.message}`);
        });
    }
}
