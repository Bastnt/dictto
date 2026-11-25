import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Pango from 'gi://Pango';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {
    ERROR_TIMEOUT_MS, OverlayModel, SPINNER_DOT, SPINNER_INTERVAL_MS,
    barRects, placement, spinnerDots,
} from './state.js';

// The recording pill: bars while recording, a spinner while processing, the message
// after an error. Nothing here takes keyboard focus, so dictation lands in the window
// the user was typing in.
export class Overlay {
    constructor({onStop, onCancel}) {
        this._model = new OverlayModel();
        this._present = false;
        this._spinnerId = 0;
        this._errorId = 0;
        this._dark = false;

        this._pill = new St.BoxLayout({
            style_class: 'dictto-pill',
            can_focus: false,
            reactive: true,
            visible: false,
        });
        this._cancel = this._button('Cancel', onCancel);
        this._area = new St.DrawingArea({x_expand: true, y_expand: true});
        this._area.connect('repaint', area => this._paint(area));
        this._label = new St.Label({
            style_class: 'dictto-pill-message',
            x_expand: true,
            x_align: Clutter.ActorAlign.CENTER,
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._label.clutter_text.line_wrap = true;
        this._label.clutter_text.ellipsize = Pango.EllipsizeMode.NONE;
        this._label.clutter_text.line_alignment = Pango.Alignment.CENTER;
        this._stop = this._button('Stop', onStop);
        for (const child of [this._cancel, this._area, this._label, this._stop])
            this._pill.add_child(child);

        Main.layoutManager.addTopChrome(this._pill);

        this._interface = new Gio.Settings({schema_id: 'org.gnome.desktop.interface'});
        this._signals = [
            [this._interface, this._interface.connect('changed::color-scheme', () => this._syncTheme())],
            [Main.layoutManager, Main.layoutManager.connect('monitors-changed', () => this._place())],
            [global.display, global.display.connect('workareas-changed', () => this._place())],
        ];
        const theme = St.ThemeContext.get_for_stage(global.stage);
        this._signals.push([theme, theme.connect('notify::scale-factor', () => this._place())]);
        this._syncTheme();
    }

    _button(label, action) {
        const button = new St.Button({
            style_class: 'dictto-pill-button',
            label,
            can_focus: false,
            track_hover: true,
            y_expand: true,
        });
        button.connect('clicked', () => action());
        return button;
    }

    destroy() {
        this._stopSpinner();
        this._stopErrorTimer();
        for (const [object, id] of this._signals)
            object.disconnect(id);
        this._signals = [];
        this._interface = null;
        Main.layoutManager.removeChrome(this._pill);
        this._pill.destroy();
        this._pill = null;
    }

    // The daemon owns com.dictto.App; without it nothing is recording.
    setPresent(present) {
        this._present = present;
        if (!present) {
            this._model.reset();
            this._stopErrorTimer();
        }
        this._layout();
    }

    handleEvent(name, payloadJson) {
        if (!OverlayModel.handles(name))
            return;
        const {layout, errorTimer} = this._model.apply(name, payloadJson);
        if (errorTimer === 'start')
            this._startErrorTimer();
        else if (errorTimer === 'clear')
            this._stopErrorTimer();
        if (layout)
            this._layout();
        else if (this._pill.visible)
            this._area.queue_repaint();
    }

    _syncTheme() {
        this._dark = this._interface.get_string('color-scheme') === 'prefer-dark';
        if (this._dark)
            this._pill.add_style_class_name('dark');
        else
            this._pill.remove_style_class_name('dark');
        this._area.queue_repaint();
    }

    _layout() {
        const view = this._model.view;
        const visible = this._present && view !== 'hidden';
        this._pill.visible = visible;
        if (visible) {
            const continuous = view === 'continuous';
            this._cancel.visible = continuous;
            this._stop.visible = continuous;
            this._area.visible = view !== 'error';
            this._label.visible = view === 'error';
            this._label.text = this._model.error;
            this._place();
            this._area.queue_repaint();
        }
        if (visible && view === 'processing')
            this._startSpinner();
        else
            this._stopSpinner();
    }

    _place() {
        if (!this._pill.visible)
            return;
        const index = Main.layoutManager.primaryIndex;
        if (index < 0)
            return;
        const workArea = Main.layoutManager.getWorkAreaForMonitor(index);
        const scale = St.ThemeContext.get_for_stage(global.stage).scale_factor;
        const rect = placement(workArea, this._model.size, scale);
        this._pill.set_position(rect.x, rect.y);
        this._pill.set_size(rect.width, rect.height);
    }

    _startSpinner() {
        if (this._spinnerId)
            return;
        this._spinnerId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, SPINNER_INTERVAL_MS, () => {
            this._model.tick();
            this._area.queue_repaint();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _stopSpinner() {
        if (this._spinnerId) {
            GLib.source_remove(this._spinnerId);
            this._spinnerId = 0;
        }
    }

    _startErrorTimer() {
        this._stopErrorTimer();
        this._errorId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, ERROR_TIMEOUT_MS, () => {
            this._errorId = 0;
            this._model.clearError();
            this._layout();
            return GLib.SOURCE_REMOVE;
        });
    }

    _stopErrorTimer() {
        if (this._errorId) {
            GLib.source_remove(this._errorId);
            this._errorId = 0;
        }
    }

    _paint(area) {
        const view = this._model.view;
        if (view !== 'processing' && view !== 'recording' && view !== 'continuous')
            return;
        const cr = area.get_context();
        const [width, height] = area.get_surface_size();
        const scale = St.ThemeContext.get_for_stage(global.stage).scale_factor;
        if (view === 'processing') {
            for (const dot of spinnerDots(this._model.phase, this._dark)) {
                const gray = dot.gray / 255;
                cr.setSourceRGBA(gray, gray, gray, 1);
                cr.arc(width / 2 + dot.x * scale, height / 2 + dot.y * scale,
                    SPINNER_DOT * scale / 2, 0, 2 * Math.PI);
                cr.fill();
            }
        } else {
            const color = area.get_theme_node().get_foreground_color();
            cr.setSourceRGBA(color.red / 255, color.green / 255, color.blue / 255, color.alpha / 255);
            for (const bar of barRects(this._model.bars, width / scale, height / scale))
                cr.rectangle(bar.x * scale, bar.y * scale, bar.width * scale, bar.height * scale);
            cr.fill();
        }
        cr.$dispose();
    }
}
