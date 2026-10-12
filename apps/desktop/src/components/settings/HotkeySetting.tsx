import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function HotkeySetting({ value, onSave }: { value: string; onSave: (value: string) => void }) {
  const [capturing, setCapturing] = useState(false);
  const [candidate, setCandidate] = useState("");
  const [error, setError] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!capturing) return;
    let mounted = true;
    void invoke("set_hotkey_capture", { active: true }).then(() => {
      if (mounted) input.current?.focus();
    }).catch(() => { setError("Unable to start shortcut capture"); setCapturing(false); });
    return () => { mounted = false; void invoke("set_hotkey_capture", { active: false }).catch(console.error); };
  }, [capturing]);
  return <div className="space-y-2">
    <div className="flex items-center justify-between">
      <div><p className="text-sm font-medium text-text-secondary">Push-to-talk hotkey</p>
        <p className="text-xs text-text-tertiary">Currently: {value}</p></div>
      <button aria-label="Change hotkey" className="px-3 py-1.5 bg-surface-elevated border border-border-strong rounded-sm text-xs text-text-secondary"
        onClick={() => { setCandidate(""); setError(""); setCapturing(!capturing); }}>{capturing ? "Cancel" : "Change"}</button>
    </div>
    {capturing && <input ref={input} aria-label="Record shortcut" readOnly value={candidate}
      placeholder="Press your shortcut; Escape cancels"
      className="w-full px-3 py-2 bg-surface border border-border rounded-md text-text"
      onKeyDown={(event) => {
        event.preventDefault(); event.stopPropagation();
        if (event.key === "Escape") { setCapturing(false); return; }
        const modifiers = [event.ctrlKey && "Ctrl", event.altKey && "Alt", event.shiftKey && "Shift", event.metaKey && "Win"].filter(Boolean) as string[];
        let key = "";
        if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3);
        else if (/^Digit[0-9]$/.test(event.code)) key = event.code.slice(5);
        else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.code)) key = event.code;
        else if (["Space", "Enter", "Tab", "Backspace"].includes(event.code)) key = event.code;
        else if (!["Control", "Alt", "Shift", "Meta"].includes(event.key)) {
          setCandidate(""); setError("Use a letter, digit, function key, Space, Enter, Tab or Backspace."); return;
        }
        if ((key && (modifiers.length > 0 || /^F\d+$/.test(key))) || (!key && modifiers.length >= 2)) {
          setCandidate([...modifiers, ...(key ? [key] : [])].join("+")); setError("");
        } else { setCandidate(""); setError("Add a modifier, or use a function key."); }
      }}
      onKeyUp={(event) => {
        event.preventDefault(); event.stopPropagation();
        if (candidate) { onSave(candidate); setCapturing(false); }
      }}
      onBlur={() => setCapturing(false)} />}
    {error && <p role="alert" className="text-xs text-danger">{error}</p>}
  </div>;
}
