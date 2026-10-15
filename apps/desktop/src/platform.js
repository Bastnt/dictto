import { invoke as tauriInvoke, isTauri } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { getCurrentWindow, currentMonitor } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";

const previewSettings = { sound_effects_enabled: "true", languages: '["en"]' };
const previewListeners = new Map();
const previewWindow = {
  label: "main", show: async () => {}, hide: async () => {},
  minimize: async () => {}, toggleMaximize: async () => {}, close: async () => {},
  isMaximized: async () => false, onResized: async () => () => {},
  innerPosition: async () => ({ x: 0, y: 0 }),
  setSize: async () => {}, setPosition: async () => {},
};

async function previewInvoke(command, args = {}) {
  switch (command) {
    case "get_all_settings": return { ...previewSettings };
    case "get_setting": return previewSettings[args.key] ?? null;
    case "set_setting":
      previewSettings[args.key] = args.value;
      for (const handler of previewListeners.get("setting-changed") ?? []) {
        handler({ payload: { key: args.key, value: args.value } });
      }
      return;
    case "has_api_key": return true;
    case "get_api_key_hint": return "Preview only";
    case "list_microphones": return [];
    case "get_current_microphone": return "System default";
    default: return null;
  }
}

export const platform = {
  invoke: (command, args) => isTauri() ? tauriInvoke(command, args) : previewInvoke(command, args),
  listen: async (name, handler) => {
    if (isTauri()) return tauriListen(name, handler);
    if (!previewListeners.has(name)) previewListeners.set(name, new Set());
    previewListeners.get(name).add(handler);
    return () => previewListeners.get(name)?.delete(handler);
  },
  getWindow: () => isTauri() ? getCurrentWindow() : previewWindow,
  currentMonitor: () => isTauri() ? currentMonitor() : Promise.resolve(null),
  setTransparent: () => isTauri()
    ? getCurrentWebview().setBackgroundColor({ red: 0, green: 0, blue: 0, alpha: 0 })
    : Promise.resolve(),
};
