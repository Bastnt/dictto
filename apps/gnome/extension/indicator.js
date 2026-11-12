import Gio from 'gi://Gio';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

// Panel button mirroring the Windows tray menu.
export const Indicator = GObject.registerClass(
class DicttoIndicator extends PanelMenu.Button {
    _init(iconPath, {onSettings, onQuit}) {
        super._init(0.0, 'Dictto', false);
        this.add_child(new St.Icon({
            gicon: Gio.icon_new_for_string(iconPath),
            style_class: 'system-status-icon',
        }));

        this._status = new PopupMenu.PopupMenuItem('', {reactive: false});
        this._statusSeparator = new PopupMenu.PopupSeparatorMenuItem();
        this.menu.addMenuItem(this._status);
        this.menu.addMenuItem(this._statusSeparator);
        this.menu.addAction('Settings', () => onSettings());
        this.menu.addAction('Quit Dictto', () => onQuit());
        this.setStatus('ok');
    }

    // "ok" hides the line; anything else is a problem worth showing.
    setStatus(text) {
        const problem = typeof text === 'string' && text !== '' && text !== 'ok';
        this._status.label.text = problem ? text : '';
        this._status.visible = problem;
        this._statusSeparator.visible = problem;
    }

    setShown(shown) {
        if (!shown)
            this.menu.close();
        this.visible = shown;
        this.container.visible = shown;
    }
});
