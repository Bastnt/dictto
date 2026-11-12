// Pure view-state and bookkeeping for the extension: no gi:// imports, so test.js can
// run it under plain gjs. Values mirror apps/native/src/overlay.rs (logical pixels).

export const BAR_COUNT = 24;
export const BAR_SPACING = 5;
export const BAR_WIDTH = 2;
export const BAR_MIN_HEIGHT = 3;
export const BAR_RANGE = 19;
export const IDLE_BAR = 0.05;

export const NORMAL_WIDTH = 216;
export const CONTINUOUS_WIDTH = 340;
export const ERROR_WIDTH = 340;
export const HEIGHT = 44;
export const ERROR_HEIGHT = 80;
export const BOTTOM_MARGIN = 18;

export const SPINNER_STEPS = 8;
export const SPINNER_INTERVAL_MS = 200;
export const SPINNER_RADIUS = 8;
export const SPINNER_DOT = 4;
export const ERROR_TIMEOUT_MS = 15000;

export const RECORDING_STATE_CHANGED = 'recording-state-changed';
export const WAVEFORM_DATA = 'waveform-data';
export const DICTATION_ERROR = 'dictation-error';
export const SPEECH_ERROR = 'speech-error';
const OVERLAY_EVENTS = new Set([RECORDING_STATE_CHANGED, WAVEFORM_DATA, DICTATION_ERROR, SPEECH_ERROR]);

function parsePayload(payload) {
    if (typeof payload !== 'string')
        return payload ?? {};
    try {
        const value = JSON.parse(payload);
        return value && typeof value === 'object' ? value : {};
    } catch {
        return {};
    }
}

export function fitBars(samples, count = BAR_COUNT) {
    const values = (Array.isArray(samples) ? samples : [])
        .map(n => Math.min(Math.max(Number(n) || 0, 0), 1));
    if (values.length === 0)
        return new Array(count).fill(IDLE_BAR);
    if (values.length === 1)
        return new Array(count).fill(values[0]);
    const bars = [];
    for (let index = 0; index < count; index++) {
        const position = index * (values.length - 1) / Math.max(count - 1, 1);
        const lower = Math.floor(position);
        const upper = Math.min(lower + 1, values.length - 1);
        const blend = position - lower;
        bars.push(values[lower] * (1 - blend) + values[upper] * blend);
    }
    return bars;
}

// What the pill shows, driven only by daemon events.
export class OverlayModel {
    constructor() {
        this.reset();
    }

    reset() {
        this.recording = false;
        this.processing = false;
        this.continuous = false;
        this.error = '';
        this.bars = new Array(BAR_COUNT).fill(IDLE_BAR);
        this.phase = 0;
    }

    static handles(name) {
        return OVERLAY_EVENTS.has(name);
    }

    // Returns whether the pill must be re-laid out and what to do with the error timer
    // ('start', 'clear' or null).
    apply(name, payload) {
        if (!OverlayModel.handles(name))
            return {layout: false, errorTimer: null};
        const value = parsePayload(payload);
        const hadError = this.error !== '';
        switch (name) {
        case RECORDING_STATE_CHANGED:
            this.recording = value.state === 'recording';
            this.processing = value.state === 'processing';
            this.continuous = this.recording && value.mode === 'continuous';
            if (this.recording || this.processing)
                this.error = '';
            if (!this.processing)
                this.phase = 0;
            return {layout: true, errorTimer: hadError && this.error === '' ? 'clear' : null};
        case WAVEFORM_DATA:
            if (Array.isArray(value.amplitudes))
                this.bars = fitBars(value.amplitudes);
            return {layout: false, errorTimer: null};
        default:
            this.recording = false;
            this.processing = false;
            this.continuous = false;
            this.phase = 0;
            this.error = typeof value.message === 'string' && value.message !== ''
                ? value.message
                : name === SPEECH_ERROR ? 'Speech failed' : 'Dictation failed';
            return {layout: true, errorTimer: 'start'};
        }
    }

    clearError() {
        this.error = '';
    }

    tick() {
        this.phase = (this.phase + 1) % SPINNER_STEPS;
    }

    get active() {
        return this.recording || this.processing || this.error !== '';
    }

    // 'hidden' | 'error' | 'processing' | 'continuous' | 'recording'
    get view() {
        if (this.error !== '')
            return 'error';
        if (this.processing)
            return 'processing';
        if (this.continuous)
            return 'continuous';
        if (this.recording)
            return 'recording';
        return 'hidden';
    }

    get size() {
        if (this.error !== '')
            return {width: ERROR_WIDTH, height: ERROR_HEIGHT};
        if (this.continuous)
            return {width: CONTINUOUS_WIDTH, height: HEIGHT};
        return {width: NORMAL_WIDTH, height: HEIGHT};
    }
}

// Bar rectangles in logical pixels, centred on an area of the given size.
export function barRects(bars, width, height) {
    const count = Math.min(bars.length, BAR_COUNT);
    const startX = width / 2 - Math.max(count - 1, 0) * BAR_SPACING / 2;
    const rects = [];
    for (let i = 0; i < count; i++) {
        const barHeight = BAR_MIN_HEIGHT + bars[i] * BAR_RANGE;
        rects.push({x: startX + i * BAR_SPACING, y: (height - barHeight) / 2, width: BAR_WIDTH, height: barHeight});
    }
    return rects;
}

// Dot centres relative to the spinner centre, with a grey level 0-255.
export function spinnerDots(phase, dark) {
    const dots = [];
    for (let dot = 0; dot < SPINNER_STEPS; dot++) {
        const angle = (dot - phase) * Math.PI / 4;
        const shade = (dot + phase) % SPINNER_STEPS;
        dots.push({
            x: SPINNER_RADIUS * Math.cos(angle),
            y: SPINNER_RADIUS * Math.sin(angle),
            gray: dark ? 0x4c + shade * 0x19 : 0xf2 - shade * 0x19,
        });
    }
    return dots;
}

// Physical rectangle of the pill at the bottom centre of a work area.
export function placement(workArea, size, scale) {
    const width = Math.round(size.width * scale);
    const height = Math.round(size.height * scale);
    return {
        x: workArea.x + Math.floor((workArea.width - width) / 2),
        y: workArea.y + workArea.height - height - Math.round(BOTTOM_MARGIN * scale),
        width,
        height,
    };
}

// St.Clipboard.set_content takes one mimetype, so one is chosen and only it is saved:
// plain text first, then an image (a lone text/html next to an image is usually the
// image's wrapper), then any other text. Other types cannot come back; null if none.
export function pickRestoreMime(mimetypes) {
    const types = mimetypes ?? [];
    for (const preferred of ['text/plain;charset=utf-8', 'text/plain']) {
        if (types.includes(preferred))
            return preferred;
    }
    return types.find(m => m.startsWith('image/')) ??
        types.find(m => m.startsWith('text/')) ??
        null;
}

const MAX_TOKEN = 0xffffffff;

// Saved clipboards keyed by a u32 token. The daemon restores each one right after a
// paste, so only the newest few are kept; older ones belong to failed pastes.
export class ClipboardTokens {
    constructor(limit = 3) {
        this._limit = limit;
        this._next = 1;
        this._saved = new Map();
    }

    // entry may be null: the clipboard held nothing restorable.
    save(entry) {
        const token = this._next;
        this._next = token >= MAX_TOKEN ? 1 : token + 1;
        this._saved.delete(token);
        this._saved.set(token, entry);
        while (this._saved.size > this._limit)
            this._saved.delete(this._saved.keys().next().value);
        return token;
    }

    has(token) {
        return this._saved.has(token);
    }

    // A token is restored once; it is gone afterwards. Unknown tokens give null too,
    // so callers tell them apart with has().
    take(token) {
        const entry = this._saved.get(token) ?? null;
        this._saved.delete(token);
        return entry;
    }

    clear() {
        this._saved.clear();
    }

    get size() {
        return this._saved.size;
    }
}
