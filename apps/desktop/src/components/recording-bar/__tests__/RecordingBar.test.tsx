import { act, render } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({recordingState: "idle", recordingMode: null, error: "",
  win: {show: vi.fn().mockResolvedValue(null), hide: vi.fn().mockResolvedValue(null)}}));
vi.mock("@tauri-apps/api/window", () => ({getCurrentWindow: () => state.win, currentMonitor: async () => null}));
vi.mock("@tauri-apps/api/webview", () => ({getCurrentWebview: () => ({setBackgroundColor: async () => {}})}));
vi.mock("../../../hooks/useRecording", () => ({useRecording: () => state}));
vi.mock("../../../hooks/useDictationError", () => ({useDictationError: () => state.error}));
vi.mock("../ContinuousControls", () => ({ContinuousControls: () => null}));
import { RecordingBar } from "../RecordingBar";

beforeEach(() => {state.recordingState = "idle"; state.error = ""; vi.clearAllMocks();});
it("hides the native overlay at idle and shows it only while active", async () => {
  const view = render(<RecordingBar barOpacity={0.9} />);
  await act(async () => {});
  expect(view.container.firstChild).toBeNull();
  expect(state.win.hide).toHaveBeenCalled();
  state.recordingState = "processing";
  view.rerender(<RecordingBar barOpacity={0.9} />);
  expect(view.container.firstChild).not.toBeNull();
  expect(state.win.show).toHaveBeenCalled();
  state.recordingState = "idle";
  view.rerender(<RecordingBar barOpacity={0.9} />);
  expect(view.container.firstChild).toBeNull();
  expect(state.win.hide).toHaveBeenCalledTimes(2);
});
