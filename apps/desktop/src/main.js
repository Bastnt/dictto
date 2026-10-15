import { platform } from "./platform.js";
import { isTauri } from "@tauri-apps/api/core";
import "./styles/globals.css";

const root = document.getElementById("root");
const isOverlay = platform.getWindow().label === "recording-bar";
document.documentElement.dataset.window = isOverlay ? "overlay" : "settings";
if (!isTauri()) {
  document.title = "Dictto (browser preview)";
  if (new URLSearchParams(window.location.search).get("theme") === "light") {
    document.documentElement.dataset.previewTheme = "light";
  }
}

try {
  const module = isOverlay ? await import("./overlay.js") : await import("./settings.js");
  const mount = isOverlay ? module.mountOverlay : module.mountSettings;
  const dispose = await mount(root);
  window.addEventListener("pagehide", () => dispose?.(), { once: true });
} catch (error) {
  console.error("Unable to open Dictto:", error);
  root.textContent = "Unable to open Dictto. Restart the app and check the application log.";
  root.setAttribute("role", "alert");
}
