import { act, renderHook } from "@testing-library/react";
import { beforeEach, expect, it } from "vitest";
import { useRecording } from "../useRecording";
import { useWaveform } from "../useWaveform";
import { useAppStore } from "../../stores/appStore";

beforeEach(() => {
  useAppStore.setState({ recordingState: "idle", recordingMode: null, waveformData: [] });
});

it("keeps waveform updates from rerendering recording state consumers", () => {
  let renders = 0;
  const { result } = renderHook(() => {
    renders++;
    return useRecording();
  });
  const initialRenders = renders;
  act(() => useAppStore.getState().setWaveformData([0.2, 0.8]));
  expect(renders).toBe(initialRenders);
  act(() => useAppStore.getState().setRecordingState("recording", "hold"));
  expect(result.current).toEqual({ recordingState: "recording", recordingMode: "hold" });
});

it("keeps recording state updates from rerendering waveform consumers", () => {
  let renders = 0;
  const { result } = renderHook(() => {
    renders++;
    return useWaveform();
  });
  const initialRenders = renders;
  act(() => useAppStore.getState().setRecordingState("processing"));
  expect(renders).toBe(initialRenders);
  act(() => useAppStore.getState().setWaveformData([0.4]));
  expect(result.current.waveformData).toEqual([0.4]);
});
