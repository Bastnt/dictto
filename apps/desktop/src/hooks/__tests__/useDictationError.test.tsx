import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { useDictationError } from "../useDictationError";

describe("dictation errors", () => {
  beforeEach(() => vi.clearAllMocks());
  it("shows a server error briefly and clears it on the next recording", async () => {
    const handlers = new Map<string, (event: {payload: unknown}) => void>();
    vi.mocked(listen).mockImplementation(async (name, handler) => {
      handlers.set(name, handler as (event: {payload: unknown}) => void);
      return () => {};
    });
    const {result} = renderHook(useDictationError);
    await act(async () => {});
    act(() => handlers.get("dictation-error")!({payload: {message: "API error: unknown_preset"}}));
    expect(result.current).toContain("model not found");
    act(() => handlers.get("recording-state-changed")!({payload: {state: "recording"}}));
    expect(result.current).toBe("");
  });
  it("does not leave a permanent error overlay", async () => {
    vi.useFakeTimers();
    let error: ((event: {payload: unknown}) => void) | undefined;
    vi.mocked(listen).mockImplementation(async (name, handler) => {
      if (name === "dictation-error") error = handler as typeof error;
      return () => {};
    });
    const {result,unmount} = renderHook(useDictationError);
    act(() => error!({payload: {message: "worker_local_use"}}));
    expect(result.current).toContain("GPU is busy");
    act(() => vi.advanceTimersByTime(15000));
    expect(result.current).toBe("");
    unmount(); vi.useRealTimers();
  });
});
