import { platform } from './platform.js';
import { LANGUAGES } from './data/languages.js';
import './styles/settings.css';

// Shown until get_all_settings answers, and when a key was never saved.
const DEFAULTS = {
  api_base_url: 'https://api.openai.com/v1',
  transcription_model: 'gpt-4o-mini-transcribe',
  llm_model: 'gpt-4.1-nano',
  recording_limit_seconds: '300',
  paste_delay_ms: '150',
  microphone_device: 'auto-detect',
};
const ENDPOINTS = ['https://api.openai.com/v1', 'https://api.groq.com/openai/v1'];
const LIMITS = [['60', '1 minute'], ['120', '2 minutes'], ['300', '5 minutes'], ['600', '10 minutes'], ['1800', '30 minutes']];
const DELAYS = [['0', 'None'], ['50', '50 ms'], ['150', '150 ms'], ['300', '300 ms'], ['500', '500 ms']];

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
  let microphones = [];
  let modal = null;
  let deviceTimer;
  let capturing = false;
  let captureReady = false;
  let captureSession = 0;
  let captureQueue = Promise.resolve();
  let candidate = '';
  // What the endpoint advertises; the runtime caches it until a refresh or a new endpoint or key.
  const catalog = { transcription: [], chat: [], loaded: false, loading: false, error: '', generation: 0 };
  const catalogViews = [];
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
  const value = (key) => settings[key] ?? DEFAULTS[key] ?? '';
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
  async function save(key, next) {
    await api.invoke('set_setting', { key, value: next });
    if (disposed) return;
    settings[key] = next;
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
  // A native select that saves on change. A saved value missing from `options()` stays
  // first, labelled by `missing()`, so loading a list never changes the setting by itself.
  function settingSelect(key, label, options, missing = () => '') {
    const select = element('select', 'settings-input settings-select');
    select.id = key;
    select.setAttribute('aria-label', label);
    function render() {
      const saved = value(key);
      const list = options();
      const note = missing();
      const all = list.some(([item]) => item === saved) ? list : [[saved, note ? `${saved} ${note}` : saved], ...list];
      select.replaceChildren(...all.map(([item, text]) => {
        const option = element('option', '', text);
        option.value = item;
        return option;
      }));
      select.value = saved;
    }
    select.addEventListener('change', () => { void run(() => save(key, select.value)).then((ok) => { if (ok === false) render(); }); });
    register(key, render);
    return { select, render };
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
  for (const [id, label] of [['general', 'General'], ['dictation', 'Dictation'], ['connection', 'Connection']]) {
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
    const maximized = await win.isMaximized();
    if (!disposed) { maximize.textContent = maximized ? '❐' : '□'; maximize.setAttribute('aria-label', maximized ? 'Restore' : 'Maximize'); }
  }
  async function subscribe(promise) {
    const unlisten = await promise;
    if (disposed) unlisten(); else subscriptions.push(unlisten);
  }
  void run(updateMaximized);
  void run(() => subscribe(win.onResized(() => { void run(updateMaximized); })));

  // General
  settingSwitch(panels.general, 'auto_start', 'Start with Windows', 'Launch Dictto when you sign in');
  settingSwitch(panels.general, 'sound_effects_enabled', 'Sound effects', 'Play a sound when recording starts and stops');
  const languageAction = row(panels.general, 'Spoken languages', 'Naming them helps the model; auto-detect guesses each time');
  const languageSummary = element('span', 'settings-inline');
  languageAction.append(languageSummary, button('Choose', openLanguages));
  languageAction.lastChild.setAttribute('aria-label', 'Choose languages');
  function languages() {
    try { const values = JSON.parse(settings.languages || '[]'); return Array.isArray(values) ? values.filter((item) => typeof item === 'string') : []; }
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
    if (!values.length || values.includes('auto')) { languageSummary.textContent = 'Auto-detect'; return; }
    const found = values.map((code) => LANGUAGES.find((language) => language.code === code)).filter(Boolean);
    for (const language of found.slice(0, 2)) {
      const item = element('span', 'settings-inline', language.name); item.prepend(flag(language)); languageSummary.append(item);
    }
    if (found.length > 2) languageSummary.append(element('span', '', `+${found.length - 2} more`));
  });
  const microphone = settingSelect('microphone_device', 'Microphone', () => [
    ['auto-detect', `Auto-detect${currentMic ? ` (${currentMic.replace(/\s*\([^)]*\)\s*$/, '')})` : ''}`],
    ...microphones.map((name) => [name, name]),
  ], () => '(not connected)');
  row(panels.general, 'Microphone', 'Auto-detect follows the Windows default input').append(microphone.select);
  async function refreshDevices() {
    const [list, name] = await Promise.all([api.invoke('list_microphones'), api.invoke('get_current_microphone')]);
    if (disposed) return;
    if (Array.isArray(list) && list.every((device) => device && typeof device.name === 'string')) microphones = list.map((device) => device.name);
    else report('Unable to read the microphone list. Try again.');
    currentMic = typeof name === 'string' ? name : '';
    microphone.render();
  }
  const hotkeyAction = row(panels.general, 'Dictation shortcut', 'Hold to dictate; double-press to keep recording');
  const hotkeyText = element('span', 'settings-inline');
  register('hotkey', () => { hotkeyText.textContent = settings.hotkey || 'Ctrl+Win'; });
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
  hotkeyButton.setAttribute('aria-label', 'Change shortcut');
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
    const chord = candidate;
    await endCapture();
    await run(() => save('hotkey', chord)); hotkeyButton.focus();
  });

  // Dictation
  const notAdvertised = () => catalog.loaded ? '(not advertised)' : '';
  const transcription = settingSelect('transcription_model', 'Transcription model', () => catalog.transcription.map((id) => [id, id]), notAdvertised);
  const refresh = button('Refresh', () => loadModels(true));
  refresh.setAttribute('aria-label', 'Refresh models');
  const transcriptionField = element('div', 'settings-field');
  const transcriptionHeading = element('div', 'settings-field-heading');
  const transcriptionLabel = element('label', 'settings-label', 'Transcription model'); transcriptionLabel.htmlFor = 'transcription_model';
  transcriptionHeading.append(transcriptionLabel);
  const picker = element('div', 'settings-actions settings-model-picker');
  picker.append(transcription.select, refresh);
  transcriptionField.append(transcriptionHeading, picker);
  panels.dictation.append(transcriptionField);
  settingSwitch(transcriptionHeading, 'transcription_warmup', 'Warm up');
  transcriptionField.append(element('p', 'settings-description', 'Warm-up sends 1 second of silence at startup and when recording starts, so the first words come back faster; provider charges may apply.'));
  const cleanup = settingSelect('llm_model', 'Cleanup model', () => catalog.chat.map((id) => [id, id]), notAdvertised);
  const cleanupField = element('div', 'settings-field');
  const cleanupHeading = element('div', 'settings-field-heading');
  const cleanupLabel = element('label', 'settings-label', 'Cleanup model'); cleanupLabel.htmlFor = 'llm_model';
  cleanupHeading.append(cleanupLabel);
  cleanupField.append(cleanupHeading, cleanup.select, element('p', 'settings-description', 'Fixes punctuation and removes filler words. When off, the raw transcript is pasted.'));
  panels.dictation.append(cleanupField);
  settingSwitch(cleanupHeading, 'cleanup_enabled', 'Text cleanup', undefined, true);
  register('cleanup_enabled', () => { cleanup.select.disabled = settings.cleanup_enabled === 'false'; });
  catalogViews.push(transcription.render, cleanup.render);
  const limit = settingSelect('recording_limit_seconds', 'Maximum recording', () => LIMITS, () => 'seconds');
  row(panels.dictation, 'Maximum recording', 'Recording stops by itself after this long').append(limit.select);
  const delay = settingSelect('paste_delay_ms', 'Paste delay', () => DELAYS, () => 'ms');
  row(panels.dictation, 'Paste delay', 'Raise it if the start of pasted text goes missing').append(delay.select);

  // Connection
  const endpoint = element('div', 'settings-endpoint');
  const keySection = element('section', 'settings-key');
  panels.connection.append(endpoint);
  const urlField = element('div', 'settings-field');
  const urlHeading = element('div', 'settings-field-heading');
  const urlLabel = element('label', 'settings-label', 'Endpoint'); urlLabel.htmlFor = 'api_base_url';
  urlHeading.append(urlLabel);
  const urlInput = element('input', 'settings-input'); urlInput.id = 'api_base_url';
  const presets = element('datalist'); presets.id = 'settings-endpoints';
  presets.append(...ENDPOINTS.map((url) => { const option = element('option'); option.value = url; return option; }));
  urlInput.setAttribute('list', presets.id);
  register('api_base_url', () => { if (!dirty.has('api_base_url') && document.activeElement !== urlInput) urlInput.value = value('api_base_url'); });
  urlInput.addEventListener('input', () => dirty.add('api_base_url'));
  // Picking a suggestion fires change without blur; typing saves when focus leaves.
  urlInput.addEventListener('change', async () => {
    const next = urlInput.value.trim();
    if (next && next !== value('api_base_url')) {
      const ok = await run(async () => { await save('api_base_url', next); return true; });
      if (!ok) return;
    }
    dirty.delete('api_base_url'); sync('api_base_url');
  });
  urlField.append(urlHeading, urlInput, presets, element('p', 'settings-description', 'Any OpenAI-compatible base URL, including /v1.'));
  endpoint.append(urlField, keySection);
  const connectionStatus = element('p', 'settings-status');
  connectionStatus.setAttribute('role', 'status');
  panels.connection.append(connectionStatus);
  function renderCatalog() {
    refresh.disabled = catalog.loading || !hasKey;
    refresh.textContent = catalog.loading ? 'Loading…' : 'Refresh';
    connectionStatus.textContent = !hasKey ? 'Add an API key to load the available models.'
      : catalog.loading ? 'Checking the endpoint…'
        : catalog.error ? `Could not list models: ${catalog.error}`
          : catalog.loaded ? `Connected: ${catalog.transcription.length} transcription and ${catalog.chat.length} chat models available.` : '';
    for (const render of catalogViews) render();
  }
  function forgetModels() {
    Object.assign(catalog, { transcription: [], chat: [], loaded: false, loading: false, error: '', generation: catalog.generation + 1 });
    renderCatalog();
  }
  async function loadModels(fresh) {
    if (!hasKey) return;
    const mine = ++catalog.generation;
    catalog.loading = true; catalog.error = ''; renderCatalog();
    try {
      const found = await api.invoke('get_models', { refresh: fresh });
      if (disposed || mine !== catalog.generation) return;
      catalog.transcription = found?.transcription ?? []; catalog.chat = found?.chat ?? []; catalog.loaded = true;
      if (fresh) error.hidden = true;
    } catch (reason) {
      if (disposed || mine !== catalog.generation) return;
      catalog.error = reason instanceof Error ? reason.message : String(reason);
      throw reason;
    } finally {
      if (!disposed && mine === catalog.generation) { catalog.loading = false; renderCatalog(); }
    }
  }
  // Set once settings have loaded, so only later endpoint changes reload the models.
  let knownEndpoint = null;
  register('api_base_url', () => {
    if (knownEndpoint === null || knownEndpoint === value('api_base_url')) return;
    knownEndpoint = value('api_base_url');
    forgetModels(); void run(() => loadModels(false));
  });
  function renderKeyEditor(edit = !hasKey) {
    const heading = element('div', 'settings-field-heading');
    const label = element('label', 'settings-label', 'API key'); label.htmlFor = 'api_key';
    heading.append(label);
    keySection.replaceChildren(heading);
    const input = element('input', 'settings-input');
    input.id = 'api_key';
    input.setAttribute('aria-label', 'API key'); input.autocomplete = 'off';
    const actions = element('div', 'settings-actions');
    keySection.append(input, actions);
    if (edit) {
      input.type = 'password'; input.placeholder = 'Paste your API key';
      const saveButton = button('Save key', async () => {
        if (!input.value.trim()) return;
        saveButton.disabled = true;
        try {
          await api.invoke('set_api_key', { apiKey: input.value.trim() });
          hasKey = true;
          keyHint = '';
          await run(async () => { keyHint = await api.invoke('get_api_key_hint'); });
          if (!disposed) { renderKeyEditor(); updateKeyBadge(); forgetModels(); void run(() => loadModels(false)); }
        } finally { saveButton.disabled = false; }
      }, 'settings-button settings-button-primary');
      actions.append(saveButton);
      if (hasKey) actions.append(button('Cancel', () => renderKeyEditor(false)));
      keySection.append(element('p', 'settings-description', 'Kept in Windows Credential Manager.'));
    } else {
      input.readOnly = true; input.value = keyHint;
      heading.append(element('span', 'settings-configured', 'Configured'));
      actions.append(button('Replace', () => { renderKeyEditor(true); keySection.querySelector('input').focus(); }), button('Remove', () => {
        const dialog = openDialog('Remove API key');
        dialog.body.append(element('p', '', "Are you sure? Dictto won't work without an API key."));
        dialog.footer.append(button('Yes, remove', async () => {
          await api.invoke('remove_api_key');
          hasKey = false; keyHint = ''; closeDialog(); renderKeyEditor(); updateKeyBadge(); forgetModels();
        }, 'settings-button settings-button-danger'));
      }, 'settings-button settings-button-danger'));
    }
  }
  function updateKeyBadge() { tabButtons.connection.textContent = hasKey ? 'Connection' : 'Connection !'; }
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
    const dialog = openDialog('Spoken languages', 'languages');
    let draft = languages().filter((code) => LANGUAGES.some((language) => language.code === code));
    const auto = switchInput('Auto-detect languages'); auto.checked = !draft.length || languages().includes('auto');
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
  selectTab('general'); renderKeyEditor(); renderCatalog();
  void run(() => subscribe(api.listen('setting-changed', ({ payload }) => {
    if (disposed || !payload || typeof payload.key !== 'string') return;
    settings[payload.key] = payload.value; sync(payload.key);
  })));
  void run(() => subscribe(api.listen('audio-devices-changed', () => {
    clearTimeout(deviceTimer);
    deviceTimer = setTimeout(() => { void run(refreshDevices); }, 200);
  })));
  await run(async () => {
    settings = await api.invoke('get_all_settings');
    if (disposed) return;
    for (const key of syncers.keys()) sync(key);
    knownEndpoint = value('api_base_url');
    hasKey = await api.invoke('has_api_key');
    if (hasKey) keyHint = await api.invoke('get_api_key_hint');
    if (disposed) return;
    renderKeyEditor(); updateKeyBadge(); renderCatalog(); if (!hasKey) selectTab('connection');
  });
  void run(refreshDevices);
  void run(() => loadModels(false));
  return async () => {
    disposed = true; clearTimeout(deviceTimer); closeDialog();
    window.removeEventListener('blur', onWindowBlur);
    await endCapture();
    for (const unlisten of subscriptions) unlisten();
    root.replaceChildren();
  };
}
