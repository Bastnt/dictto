import { useState, useEffect } from "react";

interface ApiTabProps {
  settings: Record<string, string>;
  onSave: (key: string, value: string) => void;
  hasApiKey: boolean;
  apiKeyHint: string;
  onSaveApiKey: (key: string) => void;
  onRemoveApiKey: () => Promise<void>;
}

type KeyView = "display" | "input" | "update";

export function ApiTab({
  settings,
  onSave,
  ...keyProps
}: ApiTabProps) {
  return <div className="space-y-6">
    <div className="space-y-3">
      <label className="flex items-center gap-3 text-sm text-text-secondary">
        <input type="checkbox" role="switch" aria-label="Text cleanup"
          checked={settings.cleanup_enabled !== "false"}
          onChange={(event) => onSave("cleanup_enabled", String(event.target.checked))} />
        Text cleanup
      </label>
      <p className="text-[11px] text-text-tertiary">Turn off to paste the original transcript without an extra model call.</p>
      {([
        ["api_base_url", "API base URL", "https://api.openai.com/v1"],
        ["transcription_model", "Transcription model", "gpt-4o-mini-transcribe"],
        ["llm_model", "Cleanup model", "gpt-4.1-nano"],
      ] as const).map(([key, label, fallback]) => <label key={key} className="block text-xs text-text-secondary">
        {label}
        <input className="mt-1 w-full px-3 py-2 bg-surface border border-border rounded-md text-text"
          disabled={key === "llm_model" && settings.cleanup_enabled === "false"}
          key={`${key}:${settings[key]}`} defaultValue={settings[key] || fallback}
          onBlur={(event) => {
            const value = event.target.value.trim();
            if (value && value !== settings[key]) onSave(key, value);
          }} />
      </label>)}
      <p className="text-[11px] text-text-tertiary">Include /v1 in the URL. Both transcription and cleanup use this endpoint.</p>
    </div>
    <ApiKeyEditor {...keyProps} />
  </div>;
}

function ApiKeyEditor({
  hasApiKey,
  apiKeyHint,
  onSaveApiKey,
  onRemoveApiKey,
}: Omit<ApiTabProps, "settings" | "onSave">) {
  const [view, setView] = useState<KeyView>(hasApiKey ? "display" : "input");
  const [apiKey, setApiKey] = useState("");
  const [showRemoveConfirm, setShowRemoveConfirm] = useState(false);

  useEffect(() => {
    setView(hasApiKey ? "display" : "input");
  }, [hasApiKey]);

  const handlePress = (e: React.MouseEvent<HTMLButtonElement>) => {
    const btn = e.currentTarget;
    btn.classList.remove("btn-press");
    void btn.offsetWidth;
    btn.classList.add("btn-press");
  };

  // No key — input with Save Key button
  if (view === "input") {
    return (
      <div>
        <div className="flex items-center gap-2 mb-2">
          <span className="text-xs font-semibold text-text-secondary">
            API Key
          </span>
          <span className="inline-flex items-center px-2 py-0.5 rounded-xs text-[10px] font-bold leading-none bg-danger/15 text-danger border border-danger/25">
            Required
          </span>
        </div>
        <div className="flex items-center gap-2.5">
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="API key for the configured endpoint"
            className="flex-1 px-3.5 py-2.5 bg-surface border border-border rounded-sm text-[13px] text-text placeholder:text-text-tertiary"
          />
          <button
            onClick={(e) => {
              handlePress(e);
              if (apiKey.trim()) {
                onSaveApiKey(apiKey.trim());
                setApiKey("");
              }
            }}
            className="px-4 py-2.5 bg-text text-bg rounded-md text-xs font-semibold shrink-0"
          >
            Save Key
          </button>
        </div>
        <p className="text-[11px] text-text-tertiary mt-2 leading-relaxed">
          Required for transcription and text cleanup.
        </p>
      </div>
    );
  }

  // Update mode — blank input with Save Key + Cancel
  if (view === "update") {
    return (
      <div>
        <div className="text-xs font-semibold text-text-secondary mb-2">
          API Key
        </div>
        <div className="flex items-center gap-2.5">
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="API key for the configured endpoint"
            className="flex-1 px-3.5 py-2.5 bg-surface border border-border rounded-sm text-[13px] text-text placeholder:text-text-tertiary"
            autoFocus
          />
          <button
            onClick={(e) => {
              handlePress(e);
              if (apiKey.trim()) {
                onSaveApiKey(apiKey.trim());
                setApiKey("");
              }
            }}
            className="px-4 py-2.5 bg-text text-bg rounded-md text-xs font-semibold shrink-0"
          >
            Save Key
          </button>
        </div>
        <div className="mt-3 flex gap-2">
          <button
            onClick={(e) => {
              handlePress(e);
              setApiKey("");
              setView("display");
            }}
            className="px-4 py-1.5 bg-surface-elevated border border-border-strong rounded-md text-xs font-semibold text-text"
          >
            Cancel
          </button>
        </div>
      </div>
    );
  }

  // Configured — read-only input with masked key + status badge, buttons below
  return (
    <div>
      <div className="text-xs font-semibold text-text-secondary mb-2">
        API Key
      </div>
      <div className="flex items-center gap-2.5">
        <input
          type="text"
          value={apiKeyHint}
          readOnly
          className="flex-1 px-3.5 py-2.5 bg-surface border border-border rounded-sm text-[13px] text-text"
        />
        <div className="inline-flex items-center gap-1.5 text-[11px] font-semibold text-accent bg-accent-bg border border-accent-border px-3 py-1 rounded-xs shrink-0">
          <div className="w-[5px] h-[5px] rounded-full bg-accent" />
          Configured
        </div>
      </div>
      <div className="mt-3 flex gap-2">
        <button
          onClick={(e) => {
            handlePress(e);
            setView("update");
          }}
          className="px-4 py-1.5 bg-surface-elevated border border-border-strong rounded-md text-xs font-semibold text-text"
        >
          Update Key
        </button>
        <button
          onClick={(e) => {
            handlePress(e);
            setShowRemoveConfirm(true);
          }}
          className="px-4 py-1.5 bg-transparent rounded-md text-xs font-semibold text-danger"
        >
          Remove
        </button>
      </div>

      {showRemoveConfirm && (
        <div className="mt-4 p-3 bg-surface border border-border-strong rounded-sm">
          <p className="text-sm text-text-secondary mb-3">
            Are you sure? Dictto won't work without an API key.
          </p>
          <div className="flex gap-2">
            <button
              onClick={async (e) => {
                handlePress(e);
                await onRemoveApiKey();
                setShowRemoveConfirm(false);
              }}
              className="px-4 py-1.5 bg-danger text-white rounded-md text-xs font-semibold"
            >
              Yes, remove
            </button>
            <button
              onClick={(e) => {
                handlePress(e);
                setShowRemoveConfirm(false);
              }}
              className="px-4 py-1.5 bg-surface-elevated border border-border-strong rounded-md text-xs font-semibold text-text"
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
