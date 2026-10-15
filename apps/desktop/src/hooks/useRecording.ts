import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore, RecordingState, RecordingMode } from "../stores/appStore";

interface RecordingStatePayload {
  state: RecordingState;
  mode: RecordingMode;
}

export function useRecording() {
  const recordingState = useAppStore((state) => state.recordingState);
  const recordingMode = useAppStore((state) => state.recordingMode);
  const setRecordingState = useAppStore((state) => state.setRecordingState);

  useEffect(() => {
    const unlisten = listen<RecordingStatePayload>(
      "recording-state-changed",
      (event) => {
        const { state, mode } = event.payload;
        setRecordingState(state, mode);
      }
    );

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setRecordingState]);

  return { recordingState, recordingMode };
}
