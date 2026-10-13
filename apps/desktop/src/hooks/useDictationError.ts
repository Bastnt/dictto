import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

function errorMessage(message: string): string {
  if (/unknown_preset/.test(message)) return "Transcription model not found. Check Settings > API.";
  if (/worker_local_use|gpu_occupied|worker_preempted/.test(message)) return "Transcription server GPU is busy. Try again shortly.";
  if (/worker_unavailable/.test(message)) return "Transcription server is unavailable. Try again shortly.";
  if (/No API key/.test(message)) return "Set an API key in Settings > API.";
  if (/timeout/i.test(message)) return "Transcription timed out. Try again.";
  return "Dictation failed. Check the API settings and application log.";
}

export function useDictationError() {
  const [message, setMessage] = useState("");
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const clear = () => { clearTimeout(timer); setMessage(""); };
    const error = listen<{message: string}>("dictation-error", (event) => {
      clearTimeout(timer);
      setMessage(errorMessage(event.payload.message));
      timer = setTimeout(clear, 15000);
    });
    const recording = listen<{state: string}>("recording-state-changed", (event) => {
      if (event.payload.state === "recording") clear();
    });
    return () => {
      clearTimeout(timer);
      void error.then((unlisten) => unlisten());
      void recording.then((unlisten) => unlisten());
    };
  }, []);
  return message;
}
