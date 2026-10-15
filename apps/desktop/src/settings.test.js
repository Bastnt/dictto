import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mountSettings } from './settings.js';

let root, api, handlers, dispose;
const flush = async () => { for (let index = 0; index < 12; index++) await Promise.resolve(); };
const byLabel = (label) => root.querySelector(`[aria-label="${label}"]`);
const clickText = (text, scope = root) => [...scope.querySelectorAll('button')].find((node) => node.textContent === text).click();

beforeEach(() => {
  root = document.createElement('div'); document.body.append(root); handlers = {};
  api = {
    invoke: vi.fn(async (command) => {
      if (command === 'get_all_settings') return { languages: '["en"]', microphone_device: 'Desk microphone' };
      if (command === 'has_api_key') return true;
      if (command === 'get_api_key_hint') return 'sk-…1234';
      if (command === 'get_current_microphone') return 'Desk microphone';
      if (command === 'list_microphones') return [{ id: 'desk', name: 'Desk microphone', is_default: true }];
    }),
    listen: vi.fn(async (name, handler) => { handlers[name] = handler; return vi.fn(); }),
    getWindow: () => ({ isMaximized: async () => false, onResized: async () => vi.fn(), minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn() }),
  };
});
afterEach(async () => { await dispose?.(); dispose = undefined; root.remove(); vi.useRealTimers(); });

describe('native settings', () => {
  it('opens API on first run and saves a key without clearing failed drafts', async () => {
    const original = api.invoke.getMockImplementation();
    api.invoke.mockImplementation(async (command, args) => command === 'has_api_key' ? false : original(command, args));
    dispose = await mountSettings(root, api);
    expect(root.querySelector('#settings-tab-api').getAttribute('aria-selected')).toBe('true');
    byLabel('API Key').value = '  new-secret  ';
    clickText('Save Key'); await flush();
    expect(api.invoke).toHaveBeenCalledWith('set_api_key', { apiKey: 'new-secret' });
    expect(byLabel('API Key').readOnly).toBe(true);
    expect(byLabel('API Key').value).toBe('sk-…1234');
    const model = root.querySelector('#transcription_model');
    model.focus(); model.value = ' custom-transcriber '; model.dispatchEvent(new Event('input'));
    model.blur(); await flush();
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'transcription_model', value: 'custom-transcriber' });
    expect(model.value).toBe('custom-transcriber');
  });

  it('uses one checkbox switch class and preserves cleanup/warmup defaults', async () => {
    dispose = await mountSettings(root, api);
    expect(byLabel('Text cleanup').checked).toBe(true);
    expect(byLabel('Warm transcription model').checked).toBe(false);
    expect(byLabel('Start on boot').className).toBe(byLabel('Text cleanup').className);
    byLabel('Warm transcription model').click(); await flush();
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'transcription_warmup', value: 'true' });
  });

  it('keeps input drafts and focus when settings events arrive', async () => {
    dispose = await mountSettings(root, api); clickText('API');
    const input = root.querySelector('#transcription_model');
    input.focus(); input.value = 'draft-model'; input.dispatchEvent(new Event('input'));
    handlers['setting-changed']({ payload: { key: 'transcription_model', value: 'external-model' } });
    handlers['setting-changed']({ payload: { key: 'cleanup_enabled', value: 'false' } });
    expect(document.activeElement).toBe(input); expect(input.value).toBe('draft-model');
    expect(root.querySelector('#llm_model').disabled).toBe(true);
  });

  it('keeps language drafts open and shows save failures', async () => {
    dispose = await mountSettings(root, api);
    byLabel('Change languages').focus(); byLabel('Change languages').click();
    api.invoke.mockImplementation(async (command) => { if (command === 'set_setting') throw new Error('Save unavailable'); });
    clickText('Save and close'); await flush();
    expect(root.querySelector('[role="dialog"]')).not.toBeNull();
    expect(root.querySelector('[role="dialog"] [role="alert"]').textContent).toBe('Save unavailable');
    root.querySelector('[role="dialog"]').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(root.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(byLabel('Change languages'));
  });

  it('restores a failed switch to the saved setting', async () => {
    dispose = await mountSettings(root, api);
    api.invoke.mockRejectedValueOnce(new Error('Cannot save'));
    byLabel('Start on boot').click(); await flush();
    expect(byLabel('Start on boot').checked).toBe(false);
    expect(root.querySelector('[role="alert"]').textContent).toBe('Cannot save');
  });

  it('releases hotkey capture on blur and disposal', async () => {
    dispose = await mountSettings(root, api);
    byLabel('Change hotkey').click(); await flush();
    expect(document.activeElement).toBe(byLabel('Record shortcut'));
    window.dispatchEvent(new Event('blur')); await flush();
    expect(api.invoke).toHaveBeenLastCalledWith('set_hotkey_capture', { active: false });
    byLabel('Change hotkey').click(); await flush(); await dispose(); dispose = undefined;
    expect(api.invoke).toHaveBeenLastCalledWith('set_hotkey_capture', { active: false });
  });

  it('does not revert microphones on malformed lists and debounces device events', async () => {
    dispose = await mountSettings(root, api); clickText('Audio');
    api.invoke.mockImplementation(async (command) => {
      if (command === 'list_microphones') return null;
      if (command === 'get_current_microphone') return 'Desk microphone';
    });
    byLabel('Change microphone').click(); await flush();
    expect(api.invoke).not.toHaveBeenCalledWith('set_setting', { key: 'microphone_device', value: 'auto-detect' });
    vi.useFakeTimers(); api.invoke.mockClear();
    handlers['audio-devices-changed']({}); handlers['audio-devices-changed']({});
    await vi.advanceTimersByTimeAsync(199); expect(api.invoke).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(api.invoke.mock.calls.filter(([command]) => command === 'list_microphones')).toHaveLength(1);
  });

  it('retains the API-key draft when saving fails and confirms removal', async () => {
    dispose = await mountSettings(root, api); clickText('API'); clickText('Update Key');
    const input = byLabel('API Key'); input.value = 'secret-key';
    api.invoke.mockRejectedValueOnce(new Error('Key save failed'));
    clickText('Save Key'); await flush();
    expect(byLabel('API Key')).toBe(input); expect(input.value).toBe('secret-key');
    clickText('Cancel'); clickText('Remove');
    expect(api.invoke).not.toHaveBeenCalledWith('remove_api_key');
    clickText('Yes, remove'); await flush();
    expect(api.invoke).toHaveBeenCalledWith('remove_api_key');
    expect(byLabel('API Key').type).toBe('password');
  });
});
