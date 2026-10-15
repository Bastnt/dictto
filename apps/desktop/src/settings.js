import { platform } from './platform.js';
import { LANGUAGES } from './data/languages.js';
import './styles/settings.css';

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export async function mountSettings(root, api = platform) {
  let disposed = false;
  let settings = {};
  let hasKey = false;
  let keyHint = '';
  let currentMic = '';
  let modal = null;
  let deviceTimer;
  let capturing = false;
  let captureReady = false;
  let captureSession = 0;
  let captureQueue = Promise.resolve();
  let candidate = '';
  const subscriptions = [];
  const syncers = new Map();
  const dirty = new Set();
  const shell = element('div', 'settings-shell');
  const header = element('header', 'settings-header');
  header.setAttribute('data-tauri-drag-region', '');
  const title = element('h1', 'settings-title', 'Settings');
  const brand = element('span', 'settings-brand');
  brand.setAttribute('aria-hidden', 'true');
  brand.innerHTML = '<svg viewBox="0 0 120 120" fill="none"><defs><mask id="settings-brand-mask"><rect width="120" height="120" fill="white"/><rect x="44" y="36" width="9" height="48" rx="4.5" fill="black"/><rect x="56" y="28" width="9" height="64" rx="4.5" fill="black"/><rect x="68" y="38" width="9" height="44" rx="4.5" fill="black"/></mask></defs><path d="M26 16H58C88 16 106 36 106 60C106 84 88 104 58 104H26V16Z" fill="currentColor" mask="url(#settings-brand-mask)"/></svg>';
  title.prepend(brand);
  title.setAttribute('data-tauri-drag-region', '');
  const tabs = element('nav', 'settings-tabs');
  tabs.setAttribute('aria-label', 'Settings categories');
  tabs.setAttribute('role', 'tablist');
  const controls = element('div', 'settings-window-controls');
  const main = element('main', 'settings-main');
  const error = element('p', 'settings-error');
  error.setAttribute('role', 'alert');
  error.hidden = true;
  header.append(title, tabs, controls);
  shell.append(header, error, main);
  root.replaceChildren(shell);
  const report = (reason) => {
    if (disposed) return;
    error.textContent = reason instanceof Error ? reason.message : String(reason);
    error.hidden = false;
    if (modal) {
      let notice = modal.card.querySelector('[role="alert"]');
      if (!notice) { notice = element('p', 'settings-error'); notice.setAttribute('role', 'alert'); modal.card.insertBefore(notice, modal.footer); }
      notice.textContent = error.textContent;
    }
  };
  const run = async (work) => { try { return await work(); } catch (reason) { report(reason); return false; } };
  function button(text, action, className = 'settings-button') {
    const node = element('button', className, text);
    node.type = 'button';
    node.addEventListener('click', () => { void run(action); });
    return node;
  }
  function register(key, update) {
    if (!syncers.has(key)) syncers.set(key, []);
    syncers.get(key).push(update);
    update();
  }
  function sync(key) { for (const update of syncers.get(key) || []) update(); }
  async function save(key, value) {
    await api.invoke('set_setting', { key, value });
    if (disposed) return;
    settings[key] = value;
    sync(key);
  }
  function row(parent, label, description) {
    const node = element('div', 'settings-row');
    const copy = element('div', 'settings-copy');
    copy.append(element('p', 'settings-label', label));
    if (description) copy.append(element('p', 'settings-description', description));
    const action = element('div', 'settings-actions');
    node.append(copy, action);
    parent.append(node);
    return action;
  }
  function switchInput(label) {
    const input = element('input', 'settings-switch');
    input.type = 'checkbox';
    input.setAttribute('role', 'switch');
    input.setAttribute('aria-label', label);
    return input;
  }
  function settingSwitch(parent, key, label, description, fallback = false) {
    const action = row(parent, label, description);
    const input = switchInput(label);
    const update = () => { input.checked = settings[key] === undefined ? fallback : settings[key] === 'true'; };
    register(key, update);
    input.addEventListener('change', async () => {
      input.disabled = true;
      await run(() => save(key, String(input.checked)));
      update();
      input.disabled = false;
    });
    action.append(input);
    return input;
  }
  const panels = {};
  const tabButtons = {};
  function selectTab(id) {
    if (id !== 'general') void endCapture();
    for (const [name, panel] of Object.entries(panels)) {
      panel.hidden = name !== id;
      tabButtons[name].setAttribute('aria-selected', String(name === id));
      tabButtons[name].tabIndex = name === id ? 0 : -1;
    }
  }
  for (const [id, label] of [['general', 'General'], ['audio', 'Audio'], ['api', 'API']]) {
    const tab = button(label, () => selectTab(id), 'settings-tab');
    tab.id = `settings-tab-${id}`;
    tab.setAttribute('role', 'tab');
    tab.setAttribute('aria-controls', `settings-panel-${id}`);
    tab.addEventListener('keydown', (event) => {
      const names = Object.keys(panels);
      let index = names.indexOf(id);
      if (event.key === 'ArrowRight') index = (index + 1) % names.length;
      else if (event.key === 'ArrowLeft') index = (index + names.length - 1) % names.length;
      else if (event.key === 'Home') index = 0;
      else if (event.key === 'End') index = names.length - 1;
      else return;
      event.preventDefault(); selectTab(names[index]); tabButtons[names[index]].focus();
    });
    const panel = element('section', 'settings-panel');
    panel.id = `settings-panel-${id}`;
    panel.setAttribute('role', 'tabpanel');
    panel.setAttribute('aria-labelledby', tab.id);
    tabs.append(tab); main.append(panel);
    panels[id] = panel; tabButtons[id] = tab;
  }
  const win = api.getWindow();
  controls.append(button('−', () => win.minimize(), 'settings-window-button'));
  controls.lastChild.setAttribute('aria-label', 'Minimize');
  const maximize = button('□', () => win.toggleMaximize(), 'settings-window-button');
  maximize.setAttribute('aria-label', 'Maximize');
  controls.append(maximize);
  controls.append(button('×', () => win.close(), 'settings-window-button settings-window-close'));
  controls.lastChild.setAttribute('aria-label', 'Close');
  async function updateMaximized() {
    const value = await win.isMaximized();
    if (!disposed) { maximize.textContent = value ? '❐' : '□'; maximize.setAttribute('aria-label', value ? 'Restore' : 'Maximize'); }
  }
  async function subscribe(promise) {
    const unlisten = await promise;
    if (disposed) unlisten(); else subscriptions.push(unlisten);
  }
  void run(updateMaximized);
  void run(() => subscribe(win.onResized(() => { void run(updateMaximized); })));
  settingSwitch(panels.general, 'auto_start', 'Start on boot', 'Launch Dictto when you sign in');
  settingSwitch(panels.general, 'sound_effects_enabled', 'Sound effects', 'Play sounds on recording events');
  const languageAction = row(panels.general, 'Languages', 'The languages you speak');
  const languageSummary = element('span', 'settings-inline');
  languageAction.append(languageSummary, button('Change', openLanguages));
  languageAction.lastChild.setAttribute('aria-label', 'Change languages');
  function languages() {
    try { const values = JSON.parse(settings.languages || '[]'); return Array.isArray(values) ? values.filter((value) => typeof value === 'string') : []; }
    catch { return []; }
  }
  function flag(language) {
    if (!language.countryCode) return element('span', 'settings-globe', '◎');
    const image = element('img', 'settings-flag');
    image.src = `/flags/${language.countryCode}.svg`; image.alt = ''; return image;
  }
  register('languages', () => {
    const values = languages();
    languageSummary.replaceChildren();
    if (values.includes('auto')) { languageSummary.textContent = 'Auto-detect'; return; }
    const found = values.map((code) => LANGUAGES.find((language) => language.code === code)).filter(Boolean);
    for (const language of found.slice(0, 2)) {
      const item = element('span', 'settings-inline', language.name); item.prepend(flag(language)); languageSummary.append(item);
    }
    if (found.length > 2) languageSummary.append(element('span', '', `+${found.length - 2} more`));
  });
  const hotkeyAction = row(panels.general, 'Push-to-talk hotkey');
  const hotkeyText = element('span', 'settings-inline');
  register('hotkey', () => { hotkeyText.textContent = `Currently: ${settings.hotkey || 'Ctrl+Win'}`; });
  const captureInput = element('input', 'settings-input settings-capture');
  captureInput.readOnly = true; captureInput.hidden = true;
  captureInput.setAttribute('aria-label', 'Record shortcut');
  captureInput.placeholder = 'Press your shortcut; Escape cancels';
  const hotkeyButton = button('Change', async () => {
    if (capturing) { await endCapture(); return; }
    capturing = true; candidate = ''; captureInput.value = ''; hotkeyButton.textContent = 'Cancel';
    const session = ++captureSession;
    try {
      await setCapture(true);
      if (disposed || !capturing || session !== captureSession) return;
      captureReady = true; captureInput.hidden = false; captureInput.focus();
    } catch (reason) { await endCapture(); throw reason; }
  });
  hotkeyButton.setAttribute('aria-label', 'Change hotkey');
  hotkeyAction.append(hotkeyText, hotkeyButton);
  panels.general.append(captureInput);
  function setCapture(active) {
    // Preserve backend ordering when cancellation happens during a pending invoke.
    captureQueue = captureQueue.catch(() => {}).then(() => api.invoke('set_hotkey_capture', { active }));
    return captureQueue;
  }
  async function endCapture() {
    if (!capturing) return;
    capturing = false; captureReady = false; ++captureSession; candidate = '';
    captureInput.hidden = true; hotkeyButton.textContent = 'Change';
    await run(() => setCapture(false));
  }
  captureInput.addEventListener('blur', () => { void endCapture(); });
  const onWindowBlur = () => { void endCapture(); };
  window.addEventListener('blur', onWindowBlur);
  captureInput.addEventListener('keydown', (event) => {
    event.preventDefault(); event.stopPropagation();
    if (event.key === 'Escape') { void endCapture(); hotkeyButton.focus(); return; }
    if (!captureReady) return;
    const modifiers = [event.ctrlKey && 'Ctrl', event.altKey && 'Alt', event.shiftKey && 'Shift', event.metaKey && 'Win'].filter(Boolean);
    let key = '';
    if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3);
    else if (/^Digit[0-9]$/.test(event.code)) key = event.code.slice(5);
    else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code) || ['Space', 'Enter', 'Tab', 'Backspace'].includes(event.code)) key = event.code;
    else if (!['Control', 'Alt', 'Shift', 'Meta'].includes(event.key)) { candidate = ''; report('Use a letter, digit, function key, Space, Enter, Tab or Backspace.'); return; }
    candidate = ((key && (modifiers.length || /^F\d+$/.test(key))) || (!key && modifiers.length >= 2)) ? [...modifiers, ...(key ? [key] : [])].join('+') : '';
    captureInput.value = candidate;
    if (!candidate) report('Add a modifier, or use a function key.');
  });
  captureInput.addEventListener('keyup', async (event) => {
    event.preventDefault(); event.stopPropagation();
    if (!candidate || !capturing) return;
    const value = candidate;
    await endCapture();
    await run(() => save('hotkey', value)); hotkeyButton.focus();
  });
  const microphoneAction = row(panels.audio, 'Microphone', 'The input device for recording');
  const microphoneSummary = element('span', 'settings-inline');
  function updateMic() {
    microphoneSummary.textContent = !settings.microphone_device || settings.microphone_device === 'auto-detect'
      ? `Auto-detect${currentMic ? ` (${currentMic.replace(/\s*\([^)]*\)\s*$/, '')})` : ''}` : settings.microphone_device;
  }
  register('microphone_device', updateMic);
  microphoneAction.append(microphoneSummary, button('Change', openMicrophones));
  microphoneAction.lastChild.setAttribute('aria-label', 'Change microphone');
  for (const [label, description] of [['Noise suppression', 'Filter background noise during recording'], ['Auto-detect silence', 'Automatically stop recording after silence']]) {
    const action = row(panels.audio, label, `${description} · Coming soon`);
    action.parentElement.classList.add('settings-coming-soon');
    const input = switchInput(label); input.disabled = true; action.append(input);
  }
  async function refreshCurrentMic() { const name = await api.invoke('get_current_microphone'); if (!disposed) { currentMic = name; updateMic(); } }
  const endpoint = element('div', 'settings-endpoint');
  const keySection = element('section', 'settings-key');
  panels.api.append(endpoint);
  for (const [key, label, fallback] of [['api_base_url', 'API base URL', 'https://api.openai.com/v1'], ['transcription_model', 'Transcription model', 'gpt-4o-mini-transcribe'], ['llm_model', 'Cleanup model', 'gpt-4.1-nano']]) {
    const field = element('div', 'settings-field');
    const heading = element('div', 'settings-field-heading');
    const labelNode = element('label', 'settings-label', label); labelNode.htmlFor = key;
    heading.append(labelNode);
    const input = element('input', 'settings-input'); input.id = key;
    register(key, () => { if (!dirty.has(key) && document.activeElement !== input) input.value = settings[key] || fallback; });
    input.addEventListener('input', () => dirty.add(key));
    input.addEventListener('blur', async () => {
      const value = input.value.trim();
      if (!value) { dirty.delete(key); sync(key); return; }
      if (value !== settings[key]) {
        const result = await run(async () => { await save(key, value); return true; });
        if (!result) return;
      }
      dirty.delete(key); sync(key);
    });
    field.append(heading, input);
    if (key === 'api_base_url') {
      endpoint.append(field, keySection);
      field.append(element('p', 'settings-description', 'Include /v1. Used for transcription and cleanup.'));
    } else panels.api.append(field);
    if (key === 'transcription_model') {
      settingSwitch(heading, 'transcription_warmup', 'Warm transcription model');
      field.append(element('p', 'settings-description', 'Warm-up sends 1 second of silence at startup and recording start; provider charges may apply.'));
    }
    if (key === 'llm_model') {
      settingSwitch(heading, 'cleanup_enabled', 'Text cleanup', undefined, true);
      field.append(element('p', 'settings-description', 'When disabled, paste the original transcript without an extra model call.'));
      register('cleanup_enabled', () => { input.disabled = settings.cleanup_enabled === 'false'; });
    }
  }
  function renderKeyEditor(edit = !hasKey) {
    const heading = element('div', 'settings-field-heading');
    const label = element('label', 'settings-label', 'API Key'); label.htmlFor = 'api_key';
    heading.append(label);
    keySection.replaceChildren(heading);
    const input = element('input', 'settings-input');
    input.id = 'api_key';
    input.setAttribute('aria-label', 'API Key'); input.autocomplete = 'off';
    const actions = element('div', 'settings-actions');
    keySection.append(input, actions);
    if (edit) {
      input.type = 'password'; input.placeholder = 'Enter API key';
      const saveButton = button('Save Key', async () => {
        if (!input.value.trim()) return;
        saveButton.disabled = true;
        try {
          await api.invoke('set_api_key', { apiKey: input.value.trim() });
          hasKey = true;
          keyHint = '';
          await run(async () => { keyHint = await api.invoke('get_api_key_hint'); });
          if (!disposed) { renderKeyEditor(); updateKeyBadge(); }
        } finally { saveButton.disabled = false; }
      }, 'settings-button settings-button-primary');
      actions.append(saveButton);
      if (hasKey) actions.append(button('Cancel', () => renderKeyEditor(false)));
      keySection.append(element('p', 'settings-description', 'Required for transcription and text cleanup.'));
    } else {
      input.readOnly = true; input.value = keyHint;
      heading.append(element('span', 'settings-configured', 'Configured'));
      actions.append(button('Update Key', () => { renderKeyEditor(true); keySection.querySelector('input').focus(); }), button('Remove', () => {
        const dialog = openDialog('Remove API key');
        dialog.body.append(element('p', '', "Are you sure? Dictto won't work without an API key."));
        dialog.footer.append(button('Yes, remove', async () => {
          await api.invoke('remove_api_key');
          hasKey = false; keyHint = ''; closeDialog(); renderKeyEditor(); updateKeyBadge();
        }, 'settings-button settings-button-danger'));
      }, 'settings-button settings-button-danger'));
    }
  }
  function updateKeyBadge() { tabButtons.api.textContent = hasKey ? 'API' : 'API !'; }
  function closeDialog() {
    if (!modal) return;
    const previous = modal;
    modal = null; previous.overlay.remove(); shell.inert = false;
    if (previous.trigger?.isConnected) previous.trigger.focus();
  }
  function openDialog(name, kind) {
    closeDialog();
    const trigger = document.activeElement;
    const overlay = element('div', 'settings-backdrop');
    const card = element('section', `settings-dialog${kind === 'languages' ? ' settings-dialog-wide' : ''}`);
    card.setAttribute('role', 'dialog'); card.setAttribute('aria-modal', 'true'); card.setAttribute('aria-label', name); card.tabIndex = -1;
    const head = element('div', 'settings-dialog-header');
    head.append(element('h2', '', name), button('×', closeDialog));
    head.lastChild.setAttribute('aria-label', 'Close dialog');
    const body = element('div', 'settings-dialog-body');
    const footer = element('div', 'settings-dialog-footer');
    footer.append(button('Cancel', closeDialog));
    card.append(head, body, footer); overlay.append(card); root.append(overlay); shell.inert = true;
    modal = { overlay, card, body, footer, trigger, kind };
    overlay.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') { event.preventDefault(); closeDialog(); }
      if (event.key !== 'Tab') return;
      const focusable = [...card.querySelectorAll('button,input')].filter((node) => !node.disabled && !node.hidden);
      const first = focusable[0], last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    });
    card.focus(); return modal;
  }
  function openLanguages() {
    const dialog = openDialog('Languages', 'languages');
    let draft = languages().filter((code) => LANGUAGES.some((language) => language.code === code));
    const auto = switchInput('Auto-detect languages'); auto.checked = languages().includes('auto');
    const autoLabel = element('label', 'settings-auto-language', 'Auto-detect'); autoLabel.append(auto);
    const search = element('input', 'settings-input'); search.type = 'search'; search.placeholder = 'Search for any language...'; search.setAttribute('aria-label', 'Search languages');
    const layout = element('div', 'settings-language-layout');
    const grid = element('div', 'settings-language-grid');
    const selected = element('aside', 'settings-language-selected');
    layout.append(grid, selected); dialog.body.append(autoLabel, search, layout);
    const saveButton = button('Save and close', async () => {
      saveButton.disabled = true;
      try { await save('languages', JSON.stringify(auto.checked ? ['auto'] : draft)); closeDialog(); }
      finally { if (modal === dialog) update(); }
    });
    dialog.footer.append(saveButton);
    const cards = LANGUAGES.map((language) => {
      const card = button(language.name, () => { draft = draft.includes(language.code) ? draft.filter((code) => code !== language.code) : [...draft, language.code]; update(); }, 'settings-language-card');
      card.prepend(flag(language)); grid.append(card); return { language, card };
    });
    function update() {
      search.disabled = auto.checked;
      for (const { language, card } of cards) {
        card.hidden = !language.name.toLowerCase().includes(search.value.trim().toLowerCase());
        card.disabled = auto.checked;
        card.setAttribute('aria-pressed', String(draft.includes(language.code) && !auto.checked));
      }
      selected.replaceChildren(element('h3', '', 'Selected'));
      const chosen = auto.checked ? [] : draft.map((code) => LANGUAGES.find((language) => language.code === code));
      if (!chosen.length) selected.append(element('p', 'settings-description', auto.checked ? 'Auto-detect' : 'No languages selected'));
      for (const language of chosen) selected.append(element('p', '', language.name));
      saveButton.disabled = !auto.checked && !draft.length;
    }
    auto.addEventListener('change', update); search.addEventListener('input', update); update(); search.focus();
  }
  async function openMicrophones() {
    const dialog = openDialog('Select microphone', 'microphones');
    dialog.body.textContent = 'Loading microphones…';
    await refreshDevices(dialog);
  }
  async function refreshDevices(dialog) {
    const list = await api.invoke('list_microphones');
    if (!Array.isArray(list) || !list.every((device) => device && typeof device.name === 'string' && typeof device.id === 'string')) throw new Error('Unable to read microphone list. Try again.');
    if (disposed || modal !== dialog) return;
    const selected = settings.microphone_device || 'auto-detect';
    if (selected !== 'auto-detect' && !list.some((device) => device.name === selected)) await save('microphone_device', 'auto-detect');
    if (disposed || modal !== dialog) return;
    dialog.body.replaceChildren();
    const defaultName = list.find((device) => device.is_default)?.name.replace(/\s*\([^)]*\)\s*$/, '');
    for (const device of [{ name: `Auto-detect${defaultName ? ` (${defaultName})` : ''}`, value: 'auto-detect', form_factor: 'Follows the system default device' }, ...list.map((device) => ({ ...device, value: device.name }))]) {
      const card = button('', async () => { await save('microphone_device', device.value); closeDialog(); }, 'settings-device-card');
      card.append(element('span', '', device.name), element('small', 'settings-description', device.form_factor || ''));
      card.setAttribute('aria-pressed', String((settings.microphone_device || 'auto-detect') === device.value));
      dialog.body.append(card);
    }
    if (!list.length) dialog.body.append(element('p', 'settings-description', 'No microphones found'));
  }
  selectTab('general'); renderKeyEditor();
  void run(() => subscribe(api.listen('setting-changed', ({ payload }) => {
    if (disposed || !payload || typeof payload.key !== 'string') return;
    settings[payload.key] = payload.value; sync(payload.key);
  })));
  void run(() => subscribe(api.listen('audio-devices-changed', () => {
    clearTimeout(deviceTimer);
    deviceTimer = setTimeout(() => {
      void run(refreshCurrentMic);
      if (modal?.kind === 'microphones') void run(() => refreshDevices(modal));
    }, 200);
  })));
  await run(async () => {
    settings = await api.invoke('get_all_settings');
    if (disposed) return;
    for (const key of syncers.keys()) sync(key);
    hasKey = await api.invoke('has_api_key');
    if (hasKey) keyHint = await api.invoke('get_api_key_hint');
    if (disposed) return;
    renderKeyEditor(); updateKeyBadge(); if (!hasKey) selectTab('api');
  });
  void run(refreshCurrentMic);
  return async () => {
    disposed = true; clearTimeout(deviceTimer); closeDialog();
    window.removeEventListener('blur', onWindowBlur);
    await endCapture();
    for (const unlisten of subscriptions) unlisten();
    root.replaceChildren();
  };
}
