import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mountSettings } from './settings.js';

let root, api, handlers, dispose;
const flush = async () => { for (let index = 0; index < 12; index++) await Promise.resolve(); };
const byLabel = (label) => root.querySelector(`[aria-label="${label}"]`);
const clickText = (text, scope = root) => [...scope.querySelectorAll('button')].find((node) => node.textContent === text).click();
const options = (select) => [...select.options].map((option) => option.textContent);
const choose = async (select, value) => { select.value = value; select.dispatchEvent(new Event('change')); await flush(); };
const MODELS = { transcription: ['canary-1b-v2', 'whisper-1'], chat: ['gpt-4.1-nano', 'text-light'] };

beforeEach(() => {
  root = document.createElement('div'); document.body.append(root); handlers = {};
  api = {
    invoke: vi.fn(async (command) => {
      if (command === 'get_all_settings') return { languages: '["en"]', microphone_device: 'Desk microphone' };
      if (command === 'has_api_key') return true;
      if (command === 'get_api_key_hint') return 'sk-…1234';
      if (command === 'get_current_microphone') return 'Desk microphone';
      if (command === 'list_microphones') return [{ id: 'desk', name: 'Desk microphone', is_default: true }];
      if (command === 'get_models') return MODELS;
    }),
    listen: vi.fn(async (name, handler) => { handlers[name] = handler; return vi.fn(); }),
    getWindow: () => ({ isMaximized: async () => false, onResized: async () => vi.fn(), minimize: vi.fn(), toggleMaximize: vi.fn(), close: vi.fn() }),
  };
});
afterEach(async () => { await dispose?.(); dispose = undefined; root.remove(); vi.useRealTimers(); });

describe('native settings', () => {
  it('opens Connection on first run, saves a key and then loads the models', async () => {
    const original = api.invoke.getMockImplementation();
    api.invoke.mockImplementation(async (command, args) => command === 'has_api_key' ? false : original(command, args));
    dispose = await mountSettings(root, api); await flush();
    expect(root.querySelector('#settings-tab-connection').getAttribute('aria-selected')).toBe('true');
    expect(root.querySelector('#settings-tab-connection').textContent).toBe('Connection !');
    expect(api.invoke).not.toHaveBeenCalledWith('get_models', expect.anything());
    byLabel('API key').value = '  new-secret  ';
    clickText('Save key'); await flush();
    expect(api.invoke).toHaveBeenCalledWith('set_api_key', { apiKey: 'new-secret' });
    expect(byLabel('API key').readOnly).toBe(true);
    expect(byLabel('API key').value).toBe('sk-…1234');
    expect(api.invoke).toHaveBeenCalledWith('get_models', { refresh: false });
    expect(root.querySelector('[role="status"]').textContent).toBe('Connected: 2 transcription and 2 chat models available.');
  });

  it('loads the advertised models on open and keeps the saved one first', async () => {
    dispose = await mountSettings(root, api); await flush();
    expect(api.invoke).toHaveBeenCalledWith('get_models', { refresh: false });
    const model = root.querySelector('#transcription_model');
    expect(options(model)).toEqual(['gpt-4o-mini-transcribe (not advertised)', 'canary-1b-v2', 'whisper-1']);
    expect(model.value).toBe('gpt-4o-mini-transcribe');
    await choose(model, 'whisper-1');
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'transcription_model', value: 'whisper-1' });
    expect(options(model)).toEqual(['canary-1b-v2', 'whisper-1']);
    const cleanup = root.querySelector('#llm_model');
    expect(options(cleanup)).toEqual(['gpt-4.1-nano', 'text-light']);
    await choose(cleanup, 'text-light');
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'llm_model', value: 'text-light' });
    byLabel('Refresh models').click(); await flush();
    expect(api.invoke).toHaveBeenCalledWith('get_models', { refresh: true });
  });

  it('keeps the selection and reports a failed load', async () => {
    const original = api.invoke.getMockImplementation();
    api.invoke.mockImplementation(async (command, args) => { if (command === 'get_models') throw new Error('unauthorized'); return original(command, args); });
    dispose = await mountSettings(root, api); await flush();
    expect(root.querySelector('[role="alert"]').textContent).toBe('unauthorized');
    expect(root.querySelector('[role="status"]').textContent).toBe('Could not list models: unauthorized');
    expect(root.querySelector('#transcription_model').value).toBe('gpt-4o-mini-transcribe');
    expect(byLabel('Refresh models').disabled).toBe(false);
  });

  it('reloads the models when the endpoint changes', async () => {
    dispose = await mountSettings(root, api); await flush();
    api.invoke.mockClear();
    const url = root.querySelector('#api_base_url');
    url.value = 'https://api.groq.com/openai/v1'; url.dispatchEvent(new Event('input')); url.dispatchEvent(new Event('change')); await flush();
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'api_base_url', value: 'https://api.groq.com/openai/v1' });
    expect(api.invoke).toHaveBeenCalledWith('get_models', { refresh: false });
  });

  it('offers presets instead of typed numbers', async () => {
    dispose = await mountSettings(root, api); await flush();
    const limit = root.querySelector('#recording_limit_seconds');
    expect(limit.value).toBe('300');
    await choose(limit, '120');
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'recording_limit_seconds', value: '120' });
    handlers['setting-changed']({ payload: { key: 'paste_delay_ms', value: '75' } });
    expect(options(root.querySelector('#paste_delay_ms'))[0]).toBe('75 ms');
  });

  it('uses one checkbox switch class and preserves cleanup/warmup defaults', async () => {
    dispose = await mountSettings(root, api);
    expect(byLabel('Text cleanup').checked).toBe(true);
    expect(byLabel('Warm up').checked).toBe(false);
    expect(byLabel('Start with Windows').className).toBe(byLabel('Text cleanup').className);
    byLabel('Warm up').click(); await flush();
    expect(api.invoke).toHaveBeenCalledWith('set_setting', { key: 'transcription_warmup', value: 'true' });
    handlers['setting-changed']({ payload: { key: 'cleanup_enabled', value: 'false' } });
    expect(root.querySelector('#llm_model').disabled).toBe(true);
  });

  it('keeps the endpoint draft and focus when settings events arrive', async () => {
    dispose = await mountSettings(root, api);
    const input = root.querySelector('#api_base_url');
    input.focus(); input.value = 'http://draft/v1'; input.dispatchEvent(new Event('input'));
    handlers['setting-changed']({ payload: { key: 'api_base_url', value: 'http://external/v1' } });
    expect(document.activeElement).toBe(input); expect(input.value).toBe('http://draft/v1');
  });

  it('keeps language drafts open and shows save failures', async () => {
    dispose = await mountSettings(root, api);
    byLabel('Choose languages').focus(); byLabel('Choose languages').click();
    api.invoke.mockImplementation(async (command) => { if (command === 'set_setting') throw new Error('Save unavailable'); });
    clickText('Save and close'); await flush();
    expect(root.querySelector('[role="dialog"]')).not.toBeNull();
    expect(root.querySelector('[role="dialog"] [role="alert"]').textContent).toBe('Save unavailable');
    root.querySelector('[role="dialog"]').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(root.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(byLabel('Choose languages'));
  });

  it('restores a failed switch to the saved setting', async () => {
    dispose = await mountSettings(root, api);
    api.invoke.mockRejectedValueOnce(new Error('Cannot save'));
    byLabel('Start with Windows').click(); await flush();
    expect(byLabel('Start with Windows').checked).toBe(false);
    expect(root.querySelector('[role="alert"]').textContent).toBe('Cannot save');
  });

  it('releases hotkey capture on blur and disposal', async () => {
    dispose = await mountSettings(root, api);
    byLabel('Change shortcut').click(); await flush();
    expect(document.activeElement).toBe(byLabel('Record shortcut'));
    window.dispatchEvent(new Event('blur')); await flush();
    expect(api.invoke).toHaveBeenLastCalledWith('set_hotkey_capture', { active: false });
    byLabel('Change shortcut').click(); await flush(); await dispose(); dispose = undefined;
    expect(api.invoke).toHaveBeenLastCalledWith('set_hotkey_capture', { active: false });
  });

  it('lists microphones in a select, keeps a missing one and debounces device events', async () => {
    dispose = await mountSettings(root, api); await flush();
    const microphone = byLabel('Microphone');
    expect(options(microphone)).toEqual(['Auto-detect (Desk microphone)', 'Desk microphone']);
    expect(microphone.value).toBe('Desk microphone');
    api.invoke.mockImplementation(async (command) => {
      if (command === 'list_microphones') return [{ id: 'usb', name: 'USB headset', is_default: true }];
      if (command === 'get_current_microphone') return 'USB headset';
    });
    vi.useFakeTimers(); api.invoke.mockClear();
    handlers['audio-devices-changed']({}); handlers['audio-devices-changed']({});
    await vi.advanceTimersByTimeAsync(199); expect(api.invoke).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(api.invoke.mock.calls.filter(([command]) => command === 'list_microphones')).toHaveLength(1);
    expect(options(microphone)).toEqual(['Desk microphone (not connected)', 'Auto-detect (USB headset)', 'USB headset']);
    expect(api.invoke).not.toHaveBeenCalledWith('set_setting', expect.anything());
  });

  it('retains the API-key draft when saving fails and confirms removal', async () => {
    dispose = await mountSettings(root, api); clickText('Connection'); clickText('Replace');
    const input = byLabel('API key'); input.value = 'secret-key';
    api.invoke.mockRejectedValueOnce(new Error('Key save failed'));
    clickText('Save key'); await flush();
    expect(byLabel('API key')).toBe(input); expect(input.value).toBe('secret-key');
    clickText('Cancel'); clickText('Remove');
    expect(api.invoke).not.toHaveBeenCalledWith('remove_api_key');
    clickText('Yes, remove'); await flush();
    expect(api.invoke).toHaveBeenCalledWith('remove_api_key');
    expect(byLabel('API key').type).toBe('password');
    expect(root.querySelector('[role="status"]').textContent).toBe('Add an API key to load the available models.');
  });
});
