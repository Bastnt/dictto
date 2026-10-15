import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { platform } from "./platform.js";
import "./styles/overlay.css";

const PADDING = 50;
const SIZES = { recording: [130, 42], continuous: [192, 42], processing: [130, 42], error: [320, 64] };

function friendlyError(message) {
  if (/unknown_preset/.test(message)) return "Transcription model not found. Check Settings > API.";
  if (/worker_local_use|gpu_occupied|worker_preempted/.test(message)) return "Transcription server GPU is busy. Try again shortly.";
  if (/worker_unavailable/.test(message)) return "Transcription server is unavailable. Try again shortly.";
  if (/No API key/.test(message)) return "Set an API key in Settings > API.";
  if (/timeout/i.test(message)) return "Transcription timed out. Try again.";
  return "Dictation failed. Check the API settings and application log.";
}

export async function mountOverlay(root, api = platform) {
  const win = api.getWindow();
  const view = document.createElement("div");
  view.className = "recording-overlay";
  view.innerHTML = `<div class="recording-pill">
    <div class="overlay-controls">
      <button class="overlay-cancel" aria-label="Cancel recording" type="button"><span aria-hidden="true">×</span></button>
      <div class="overlay-waveform" aria-label="Audio waveform"></div>
      <button class="overlay-stop" aria-label="Stop recording" type="button"><span aria-hidden="true"></span></button>
    </div>
    <div class="overlay-processing" role="status" aria-label="Transcribing"><span></span><span></span><span></span></div>
    <div class="overlay-error" role="alert"></div>
    <div class="overlay-tooltip">Press <kbd>Ctrl + Win</kbd> again to stop dictating</div>
  </div>`;
  root.replaceChildren(view);
  const pill = view.querySelector(".recording-pill");
  const controls = view.querySelector(".overlay-controls");
  const processing = view.querySelector(".overlay-processing");
  const error = view.querySelector(".overlay-error");
  const tooltip = view.querySelector(".overlay-tooltip");
  const cancelButton = view.querySelector(".overlay-cancel");
  const stopButton = view.querySelector(".overlay-stop");
  const waveform = view.querySelector(".overlay-waveform");
  const bars = Array.from({ length: 14 }, () => {
    const bar = document.createElement("span");
    bar.style.height = "2px";
    waveform.append(bar);
    return bar;
  });
  let disposed = false;
  let revision = 0;
  let state = "idle";
  let mode = null;
  let message = "";
  let monitor = null;
  let previousSize = null;
  let errorTimer;
  let hitboxTimer;
  let opacityChanged = false;
  let nativeQueue = Promise.resolve();
  const unlisteners = [];
  const report = (reason) => console.error("Recording overlay:", reason);
  const enqueue = (operation) => {
    nativeQueue = nativeQueue.then(operation).catch(report);
    return nativeQueue;
  };
  const listen = (name, handler) => {
    Promise.resolve(api.listen(name, (event) => { if (!disposed) handler(event.payload); }))
      .then((unlisten) => { if (disposed) unlisten(); else unlisteners.push(unlisten); }).catch(report);
  };
  const currentVisual = () => message ? "error" : state === "recording" && mode === "continuous" ? "continuous" : state;
  const setOpacity = (value) => {
    const number = Number.parseFloat(value);
    view.style.setProperty("--bar-opacity", String(Number.isFinite(number) ? Math.max(0, Math.min(1, number)) : 0.9));
  };

  function scheduleHitbox(version, width, height) {
    clearTimeout(hitboxTimer);
    hitboxTimer = setTimeout(() => enqueue(async () => {
      if (disposed || version !== revision) return;
      const position = await win.innerPosition();
      if (disposed || version !== revision) return;
      const dpr = window.devicePixelRatio || 1;
      await api.invoke("update_pill_hitbox", {
        left: Math.floor(position.x + ((window.innerWidth - width) / 2) * dpr),
        top: Math.floor(position.y + (window.innerHeight - PADDING - height) * dpr),
        right: Math.ceil(position.x + ((window.innerWidth + width) / 2) * dpr),
        bottom: Math.ceil(position.y + (window.innerHeight - PADDING) * dpr),
      });
    }), 350);
  }

  function render() {
    const visual = currentVisual();
    const size = SIZES[visual];
    const version = ++revision;
    clearTimeout(hitboxTimer);
    view.hidden = !size;
    view.dataset.state = visual;
    controls.hidden = visual !== "recording" && visual !== "continuous";
    processing.hidden = visual !== "processing";
    error.hidden = visual !== "error";
    error.textContent = message;
    tooltip.hidden = visual !== "continuous";
    cancelButton.disabled = stopButton.disabled = visual !== "continuous";
    for (const dot of processing.children) dot.style.animationPlayState = visual === "processing" ? "running" : "paused";
    if (size) {
      pill.style.width = `${size[0]}px`;
      pill.style.height = `${size[1]}px`;
    }
    enqueue(async () => {
      const stale = () => disposed || version !== revision;
      if (stale()) return;
      await api.invoke("clear_pill_hitbox");
      if (stale()) return;
      if (!size) {
        previousSize = null;
        await win.hide();
        if (!stale()) await win.setSize(new LogicalSize(1, 1));
        return;
      }
      if (monitor) {
        // Keep the larger window until idle: separate native resize/move calls can visibly jump.
        const width = Math.max(size[0] + PADDING * 2, previousSize?.[0] || 0);
        const height = Math.max(size[1] + PADDING * 2, previousSize?.[1] || 0);
        previousSize = [width, height];
        await win.setSize(new LogicalSize(width, height));
        if (stale()) return;
        const scale = monitor.scaleFactor || 1;
        await win.setPosition(new LogicalPosition(
          monitor.position.x / scale + (monitor.size.width / scale - width) / 2,
          monitor.position.y / scale + monitor.size.height / scale - height - 60 + PADDING,
        ));
        if (stale()) return;
      }
      await win.show();
      if (!stale()) scheduleHitbox(version, ...size);
    });
  }

  cancelButton.addEventListener("click", () => { if (!disposed && !cancelButton.disabled) void api.invoke("bar_cancel_recording").catch(report); });
  stopButton.addEventListener("click", () => { if (!disposed && !stopButton.disabled) void api.invoke("bar_stop_recording").catch(report); });
  listen("recording-state-changed", (payload) => {
    if (!["idle", "recording", "processing"].includes(payload?.state)) return;
    state = payload.state;
    mode = payload.mode;
    if (state === "recording") { message = ""; clearTimeout(errorTimer); }
    view.classList.remove("is-hovered");
    render();
  });
  listen("waveform-data", (payload) => {
    const values = payload?.amplitudes;
    if (!Array.isArray(values)) return;
    for (let i = 0; i < bars.length; i++) {
      const index = i / (bars.length - 1) * (values.length - 1);
      const low = Math.floor(index);
      const fraction = index - low;
      const amplitude = values.length ? Number(values[low]) * (1 - fraction) + Number(values[Math.min(low + 1, values.length - 1)]) * fraction : 0.05;
      bars[i].style.height = `${Math.max(2, Math.min(1, Number.isFinite(amplitude) ? amplitude : 0) * 24 * 0.85)}px`;
    }
  });
  listen("dictation-error", (payload) => {
    clearTimeout(errorTimer);
    message = friendlyError(String(payload?.message ?? ""));
    render();
    errorTimer = setTimeout(() => { message = ""; render(); }, 15000);
  });
  listen("setting-changed", (payload) => {
    if (payload?.key === "bar_opacity") { opacityChanged = true; setOpacity(payload.value); }
  });
  listen("pill-cursor-entered", () => view.classList.toggle("is-hovered", currentVisual() === "continuous"));
  listen("pill-cursor-left", () => view.classList.remove("is-hovered"));
  listen("hitbox-click", async (payload) => {
    const version = revision;
    try {
      if (currentVisual() !== "continuous") return;
      const position = await win.innerPosition();
      if (disposed || version !== revision) return;
      const dpr = window.devicePixelRatio || 1;
      const element = document.elementFromPoint((payload.screenX - position.x) / dpr, (payload.screenY - position.y) / dpr);
      const button = element?.closest?.("button");
      if (!button || !view.contains(button) || button.disabled) return;
      // Rust consumes the physical click, so dispatch without activating or focusing the webview.
      button.click();
    } catch (reason) { report(reason); }
  });
  const onResize = () => {
    const size = SIZES[currentVisual()];
    if (size && !disposed) scheduleHitbox(revision, ...size);
  };
  window.addEventListener("resize", onResize);
  setOpacity(0.9);
  void api.invoke("get_setting", { key: "bar_opacity" }).then((value) => { if (!disposed && !opacityChanged && value != null) setOpacity(value); }).catch(report);
  void api.setTransparent().catch(report);
  void api.currentMonitor().then((value) => { if (!disposed) { monitor = value; render(); } }).catch(report);
  render();

  return () => {
    if (disposed) return nativeQueue;
    disposed = true;
    revision++;
    clearTimeout(errorTimer);
    clearTimeout(hitboxTimer);
    window.removeEventListener("resize", onResize);
    for (const unlisten of unlisteners) unlisten();
    view.remove();
    return enqueue(async () => {
      await api.invoke("clear_pill_hitbox");
      await win.hide();
    });
  };
}
