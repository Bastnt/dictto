import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useAppStore } from "../stores/appStore";

interface WaveformPayload {
  amplitudes: number[];
}

export function useWaveform() {
  const waveformData = useAppStore((state) => state.waveformData);
  const setWaveformData = useAppStore((state) => state.setWaveformData);

  useEffect(() => {
    const unlisten = listen<WaveformPayload>("waveform-data", (event) => {
      setWaveformData(event.payload.amplitudes);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [setWaveformData]);

  return { waveformData };
}
