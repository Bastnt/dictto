// Runs under plain gjs (no Shell): gjs -m test.js
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import System from 'system';

import {APP_XML, SHELL_XML} from './dbus.js';
import * as S from './state.js';

const tests = [];
const test = (name, body) => tests.push([name, body]);

function equal(actual, expected, message = '') {
    const a = JSON.stringify(actual);
    const e = JSON.stringify(expected);
    if (a !== e)
        throw new Error(`${message} expected ${e}, got ${a}`);
}

function close(actual, expected, message = '') {
    if (Math.abs(actual - expected) > 1e-6)
        throw new Error(`${message} expected ${expected}, got ${actual}`);
}

const state = (s, mode = null) => JSON.stringify({state: s, mode});

test('hold recording shows the normal pill with bars', () => {
    const m = new S.OverlayModel();
    equal(m.apply(S.RECORDING_STATE_CHANGED, state('recording', 'hold')), {layout: true, errorTimer: null});
    equal(m.view, 'recording');
    equal(m.size, {width: 216, height: 44});
    equal(m.active, true);
});

test('continuous recording widens the pill for Cancel and Stop', () => {
    const m = new S.OverlayModel();
    m.apply(S.RECORDING_STATE_CHANGED, state('recording', 'continuous'));
    equal(m.view, 'continuous');
    equal(m.size, {width: 340, height: 44});
});

test('processing shows the spinner and drops continuous mode', () => {
    const m = new S.OverlayModel();
    m.apply(S.RECORDING_STATE_CHANGED, state('recording', 'continuous'));
    m.apply(S.RECORDING_STATE_CHANGED, state('processing', 'continuous'));
    equal(m.view, 'processing');
    equal(m.size, {width: 216, height: 44});
});

test('idle hides the pill', () => {
    const m = new S.OverlayModel();
    m.apply(S.RECORDING_STATE_CHANGED, state('recording'));
    m.apply(S.RECORDING_STATE_CHANGED, JSON.stringify({state: 'idle'}));
    equal(m.view, 'hidden');
    equal(m.active, false);
});

test('waveform frames refit to 24 bars without a relayout', () => {
    const m = new S.OverlayModel();
    const result = m.apply(S.WAVEFORM_DATA, JSON.stringify({amplitudes: [0, 2, -1]}));
    equal(result, {layout: false, errorTimer: null});
    equal(m.bars.length, 24);
    close(m.bars[0], 0);
    close(m.bars[23], 0);
    close(m.bars[1], 2 / 23, 'interpolated toward the clamped middle sample');
});

test('fitBars handles empty, single and exact inputs', () => {
    equal(S.fitBars([]), new Array(24).fill(0.05));
    equal(S.fitBars([0.5]), new Array(24).fill(0.5));
    const exact = Array.from({length: 24}, (_, i) => i / 23);
    S.fitBars(exact).forEach((v, i) => close(v, exact[i]));
});

test('dictation error shows its message and starts the error timer', () => {
    const m = new S.OverlayModel();
    m.apply(S.RECORDING_STATE_CHANGED, state('processing'));
    const result = m.apply(S.DICTATION_ERROR, JSON.stringify({message: 'No API key'}));
    equal(result, {layout: true, errorTimer: 'start'});
    equal(m.view, 'error');
    equal(m.error, 'No API key');
    equal(m.size, {width: 340, height: 80});
    equal(m.processing, false);
});

test('errors without a message get a fallback', () => {
    const m = new S.OverlayModel();
    m.apply(S.SPEECH_ERROR, '{}');
    equal(m.error, 'Speech failed');
    m.apply(S.DICTATION_ERROR, 'not json');
    equal(m.error, 'Dictation failed');
});

test('a new recording clears the error and its timer', () => {
    const m = new S.OverlayModel();
    m.apply(S.DICTATION_ERROR, JSON.stringify({message: 'x'}));
    equal(m.apply(S.RECORDING_STATE_CHANGED, state('recording', 'continuous')), {layout: true, errorTimer: 'clear'});
    equal(m.view, 'continuous');
});

test('idle after an error keeps the error until its timer', () => {
    const m = new S.OverlayModel();
    m.apply(S.DICTATION_ERROR, JSON.stringify({message: 'x'}));
    equal(m.apply(S.RECORDING_STATE_CHANGED, state('idle')).errorTimer, null);
    equal(m.view, 'error');
    m.clearError();
    equal(m.view, 'hidden');
});

test('unrelated events are ignored', () => {
    const m = new S.OverlayModel();
    equal(S.OverlayModel.handles(S.RECORDING_STATE_CHANGED), true);
    equal(S.OverlayModel.handles('setting-changed'), false);
    equal(m.apply('setting-changed', '{"key":"language"}'), {layout: false, errorTimer: null});
    equal(m.view, 'hidden');
});

test('spinner advances through 8 steps and resets when processing ends', () => {
    const m = new S.OverlayModel();
    m.apply(S.RECORDING_STATE_CHANGED, state('processing'));
    for (let i = 0; i < 9; i++)
        m.tick();
    equal(m.phase, 1);
    m.apply(S.RECORDING_STATE_CHANGED, state('idle'));
    equal(m.phase, 0);
    equal(S.SPINNER_INTERVAL_MS, 200, '5 fps like the Windows timer');
});

test('spinner dots match the Windows shading', () => {
    const dark = S.spinnerDots(0, true);
    equal(dark.length, 8);
    close(dark[0].x, 8);
    close(dark[0].y, 0);
    equal(dark[0].gray, 0x4c);
    equal(dark[7].gray, 0x4c + 7 * 0x19);
    const light = S.spinnerDots(3, false);
    equal(light[5].gray, 0xf2);
    close(light[3].x, 8, 'the dot at the phase sits at angle 0');
});

test('bars are 2px wide, 5px apart, centred and 3-22px tall', () => {
    const rects = S.barRects(new Array(24).fill(0).map((_, i) => (i === 23 ? 1 : 0)), 216, 44);
    equal(rects.length, 24);
    close(rects[0].x, 108 - 57.5);
    close(rects[1].x - rects[0].x, 5);
    equal(rects[0].width, 2);
    close(rects[0].height, 3);
    close(rects[0].y, 20.5);
    close(rects[23].height, 22);
});

test('placement is bottom centre, 18px above the work area edge', () => {
    equal(S.placement({x: 0, y: 32, width: 1920, height: 1048}, {width: 216, height: 44}, 1),
        {x: 852, y: 1018, width: 216, height: 44});
    equal(S.placement({x: 1920, y: 0, width: 2560, height: 1440}, {width: 340, height: 80}, 2),
        {x: 1920 + 940, y: 1440 - 160 - 36, width: 680, height: 160});
});

test('clipboard tokens are sequential, single use and evict the oldest', () => {
    const t = new S.ClipboardTokens(3);
    equal([t.save(['a']), t.save(['b']), t.save(['c'])], [1, 2, 3]);
    equal(t.save(['d']), 4);
    equal(t.size, 3);
    equal(t.take(1), null, 'evicted');
    equal(t.take(2), ['b']);
    equal(t.take(2), null, 'single use');
    equal(t.take(99), null);
    equal(t.size, 2);
});

test('clipboard tokens wrap within u32 and never issue 0', () => {
    const t = new S.ClipboardTokens(3);
    t._next = 0xffffffff;
    equal(t.save([]), 0xffffffff);
    equal(t.save([]), 1);
});

test('restore picks plain text, then an image, then other text', () => {
    equal(S.pickRestoreMime(['text/html', 'text/plain', 'text/plain;charset=utf-8']), 'text/plain;charset=utf-8');
    equal(S.pickRestoreMime(['text/html', 'image/png', 'image/jpeg']), 'image/png');
    equal(S.pickRestoreMime(['text/html', 'text/uri-list']), 'text/html');
    equal(S.pickRestoreMime([]), null);
    equal(S.savableMimetypes(['TARGETS', 'text/plain', 'image/png', 'x-special/gnome-copied-files']),
        ['text/plain', 'image/png']);
});

function describe(xml) {
    const node = Gio.DBusNodeInfo.new_for_xml(xml);
    return node.interfaces.map(iface => ({
        name: iface.name,
        methods: iface.methods.map(m => [m.name,
            m.in_args.map(a => `${a.name}:${a.signature}`),
            m.out_args.map(a => `${a.name}:${a.signature}`)]),
        signals: iface.signals.map(s => [s.name, s.args.map(a => `${a.name}:${a.signature}`)]),
        properties: iface.properties.map(p => [p.name, p.signature, p.flags]),
    }));
}

const here = GLib.path_get_dirname(GLib.filename_from_uri(import.meta.url)[0]);
const contract = file => {
    const [, bytes] = GLib.file_get_contents(GLib.build_filenamev([here, '..', 'dbus', file]));
    return new TextDecoder().decode(bytes);
};

test('embedded App1 interface matches the contract file', () => {
    equal(describe(APP_XML), describe(contract('com.dictto.App1.xml')));
});

test('embedded Shell1 interface matches the contract file', () => {
    equal(describe(SHELL_XML), describe(contract('com.dictto.Shell1.xml')));
});

let failed = 0;
for (const [name, body] of tests) {
    try {
        body();
        print(`ok   ${name}`);
    } catch (error) {
        failed += 1;
        print(`FAIL ${name}: ${error.message}`);
    }
}
print(`${tests.length - failed} passed, ${failed} failed`);
if (failed > 0)
    System.exit(1);
