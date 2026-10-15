import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountOverlay } from "./overlay.js";

function fakeApi() {
  const handlers = new Map();
  const win = Object.fromEntries(["show", "hide", "setSize", "setPosition"].map((name) => [name, vi.fn().mockResolvedValue() ]));
  win.innerPosition = vi.fn().mockResolvedValue({ x: -1200, y: 300 });
  const api = {
    invoke: vi.fn().mockResolvedValue(null),
    getWindow: () => win,
    setTransparent: vi.fn().mockResolvedValue(),
    currentMonitor: vi.fn().mockResolvedValue({ position: { x: -1920, y: 0 }, size: { width: 1920, height: 1080 }, scaleFactor: 1.5 }),
    listen: vi.fn(async (name, handler) => { handlers.set(name, handler); return () => handlers.delete(name); }),
    emit: (name, payload) => handlers.get(name)?.({ payload }),
    handlers, win,
  };
  return api;
}

const flush = async () => { for (let i = 0; i < 35; i++) await Promise.resolve(); };
let cleanup;
beforeEach(() => { vi.useFakeTimers(); document.body.innerHTML = '<main id="root"></main>'; });
afterEach(async () => { if (cleanup) await cleanup(); cleanup = null; vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

async function mount(api = fakeApi()) {
  cleanup = await mountOverlay(document.querySelector("main"), api);
  await flush();
  return api;
}

describe("native recording overlay", () => {
  it("hides at idle and preserves logical monitor positioning and physical hitbox coordinates", async () => {
    vi.stubGlobal("devicePixelRatio", 1.5);
    const api = await mount();
    expect(document.querySelector(".recording-overlay").hidden).toBe(true);
    expect(api.win.hide).toHaveBeenCalled();
    api.emit("recording-state-changed", { state: "recording", mode: "continuous" });
    await flush();
    expect(api.win.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 292, height: 142 }));
    expect(api.win.setPosition).toHaveBeenLastCalledWith(expect.objectContaining({ x: -786, y: 568 }));
    expect(api.win.show).toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(350);
    const dpr = window.devicePixelRatio || 1;
    expect(api.invoke).toHaveBeenCalledWith("update_pill_hitbox", {
      left: Math.floor(-1200 + (window.innerWidth - 192) / 2 * dpr),
      right: Math.ceil(-1200 + (window.innerWidth + 192) / 2 * dpr),
      top: Math.floor(300 + (window.innerHeight - 50 - 42) * dpr),
      bottom: Math.ceil(300 + (window.innerHeight - 50) * dpr),
    });
    api.emit("recording-state-changed", { state: "idle" });
    await flush();
    expect(api.win.hide).toHaveBeenCalledTimes(2);
    expect(api.invoke).toHaveBeenLastCalledWith("clear_pill_hitbox");
  });

  it("updates existing waveform nodes without creating render timers and pauses inactive processing", async () => {
    const api = await mount();
    const bars = Array.from(document.querySelectorAll(".overlay-waveform span"));
    const timerCount = vi.getTimerCount();
    api.emit("waveform-data", { amplitudes: [0, 1] });
    expect(document.querySelector(".overlay-waveform span")).toBe(bars[0]);
    expect(bars[0].style.height).toBe("2px");
    expect(Number.parseFloat(bars.at(-1).style.height)).toBeCloseTo(20.4);
    expect(vi.getTimerCount()).toBe(timerCount);
    const dots = Array.from(document.querySelectorAll(".overlay-processing span"));
    expect(dots.every((dot) => dot.style.animationPlayState === "paused")).toBe(true);
    api.emit("recording-state-changed", { state: "processing" });
    expect(dots.every((dot) => dot.style.animationPlayState === "running")).toBe(true);
    api.emit("recording-state-changed", { state: "recording", mode: "push-to-talk" });
    expect(dots.every((dot) => dot.style.animationPlayState === "paused")).toBe(true);
  });

  it("sanitizes backend errors, resets their timeout and clears errors when recording starts", async () => {
    const api = await mount();
    const alert = document.querySelector('[role="alert"]');
    api.emit("dictation-error", { message: '<img src=x onerror="alert(1)"> secret token' });
    expect(alert.textContent).toBe("Dictation failed. Check the API settings and application log.");
    expect(alert.querySelector("img")).toBeNull();
    await vi.advanceTimersByTimeAsync(10000);
    api.emit("dictation-error", { message: "worker_unavailable" });
    await vi.advanceTimersByTimeAsync(5000);
    expect(alert.hidden).toBe(false);
    expect(alert.textContent).toContain("server is unavailable");
    await vi.advanceTimersByTimeAsync(10000);
    expect(alert.hidden).toBe(true);
    api.emit("dictation-error", { message: "timeout" });
    api.emit("recording-state-changed", { state: "recording", mode: "continuous" });
    expect(alert.hidden).toBe(true);
    expect(alert.textContent).toBe("");
  });

  it("routes native screen clicks on nested button content without moving DOM focus", async () => {
    const api = await mount();
    api.emit("recording-state-changed", { state: "recording", mode: "continuous" });
    await flush();
    const input = document.createElement("input");
    document.body.append(input);
    input.focus();
    const stop = document.querySelector(".overlay-stop");
    const fromPoint = vi.fn(() => stop.firstElementChild);
    Object.defineProperty(document, "elementFromPoint", { configurable: true, value: fromPoint });
    api.emit("hitbox-click", { screenX: -1100, screenY: 400 });
    await flush();
    expect(api.invoke).toHaveBeenCalledWith("bar_stop_recording");
    expect(fromPoint).toHaveBeenCalledWith(100 / window.devicePixelRatio, 100 / window.devicePixelRatio);
    expect(document.activeElement).toBe(input);
    document.querySelector(".overlay-cancel").click();
    expect(api.invoke).toHaveBeenCalledWith("bar_cancel_recording");
    api.emit("recording-state-changed", { state: "recording", mode: "push-to-talk" });
    api.invoke.mockClear();
    api.emit("hitbox-click", { screenX: -1100, screenY: 400 });
    await flush();
    expect(api.invoke).not.toHaveBeenCalledWith("bar_stop_recording");
  });

  it("prevents a delayed physical position lookup from restoring an idle hitbox", async () => {
    const api = await mount();
    let resolvePosition;
    api.win.innerPosition.mockImplementationOnce(() => new Promise((resolve) => { resolvePosition = resolve; }));
    api.emit("recording-state-changed", { state: "recording", mode: "continuous" });
    await flush();
    await vi.advanceTimersByTimeAsync(350);
    expect(resolvePosition).toBeTypeOf("function");
    api.emit("recording-state-changed", { state: "idle" });
    resolvePosition({ x: 100, y: 100 });
    await flush();
    expect(api.invoke.mock.calls.some(([name]) => name === "update_pill_hitbox")).toBe(false);
    expect(api.win.hide).toHaveBeenCalledTimes(2);
  });

  it("does not show a recording window after an in-flight native resize becomes stale", async () => {
    const api = await mount();
    let resolveSize;
    api.win.setSize.mockImplementationOnce(() => new Promise((resolve) => { resolveSize = resolve; }));
    api.emit("recording-state-changed", { state: "recording", mode: "continuous" });
    await flush();
    expect(resolveSize).toBeTypeOf("function");
    api.emit("recording-state-changed", { state: "idle" });
    resolveSize();
    await flush();
    await vi.advanceTimersByTimeAsync(400);
    expect(api.win.show).not.toHaveBeenCalled();
    expect(api.win.setPosition).not.toHaveBeenCalled();
    expect(api.invoke.mock.calls.some(([name]) => name === "update_pill_hitbox")).toBe(false);
    expect(api.win.hide).toHaveBeenCalledTimes(2);
  });

  it("releases listeners that finish registering after cleanup and cancels timers", async () => {
    const api = fakeApi();
    const registrations = [];
    api.listen.mockImplementation(() => new Promise((resolve) => registrations.push(resolve)));
    await mount(api);
    await cleanup();
    const unlisteners = registrations.map(() => vi.fn());
    registrations.forEach((resolve, index) => resolve(unlisteners[index]));
    await flush();
    expect(unlisteners.every((unlisten) => unlisten.mock.calls.length === 1)).toBe(true);
    expect(document.querySelector(".recording-overlay")).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("keeps active windows large until idle and applies live opacity settings safely", async () => {
    const api = await mount();
    api.emit("dictation-error", { message: "No API key" });
    await flush();
    api.emit("recording-state-changed", { state: "recording", mode: "continuous" });
    await flush();
    expect(api.win.setSize).toHaveBeenLastCalledWith(expect.objectContaining({ width: 420, height: 164 }));
    api.emit("setting-changed", { key: "bar_opacity", value: "0.5" });
    expect(document.querySelector(".recording-overlay").style.getPropertyValue("--bar-opacity")).toBe("0.5");
  });
});
