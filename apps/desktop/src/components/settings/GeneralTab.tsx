import { HotkeySetting } from "./HotkeySetting";
import { Toggle } from "./Toggle";
import { LANGUAGES, Language } from "../../data/languages";
import { FlagIcon } from "./FlagIcon";

function LanguageInlineDisplay({ codes }: { codes: string[] }) {
  if (codes.length === 1 && codes[0] === "auto") {
    return <span className="text-xs text-text-secondary">Auto-detect</span>;
  }
  const found = codes
    .map((code) => LANGUAGES.find((l) => l.code === code))
    .filter((l): l is Language => l !== undefined);
  if (found.length === 0) return null;

  const MAX_INLINE = 2;
  const visible = found.slice(0, MAX_INLINE);
  const remainder = found.length - MAX_INLINE;

  return (
    <span className="flex items-center gap-1 text-xs text-text-secondary">
      {visible.map((l, i) => (
        <span key={l.code} className="flex items-center gap-1">
          {i > 0 && <span className="mx-0.5">&middot;</span>}
          <FlagIcon countryCode={l.countryCode} className="w-4 h-3 rounded-[2px]" />
          {l.name}
        </span>
      ))}
      {remainder > 0 && <span className="ml-0.5">&middot; +{remainder} more</span>}
    </span>
  );
}

interface Props {
  settings: Record<string, string>;
  onSave: (key: string, value: string) => void;
  onOpenLanguageModal: () => void;
}

export function GeneralTab({ settings, onSave, onOpenLanguageModal }: Props) {
  return (
    <div className="space-y-6">
      {/* Start on boot — functional */}
      <div className="flex items-center justify-between">
        <div>
          <p className="text-sm font-medium text-text-secondary">Start on boot</p>
          <p className="text-xs text-text-tertiary">Launch Dictto when you sign in</p>
        </div>
        <Toggle label="Start on boot"
          checked={settings.auto_start === "true"}
          onChange={(checked) => onSave("auto_start", String(checked))} />
      </div>

      {/* Sound effects — functional */}
      <div className="flex items-center justify-between">
        <div>
          <p className="text-sm font-medium text-text-secondary">Sound effects</p>
          <p className="text-xs text-text-tertiary">Play sounds on recording events</p>
        </div>
        <Toggle label="Sound effects"
          checked={settings.sound_effects_enabled === "true"}
          onChange={(checked) => onSave("sound_effects_enabled", String(checked))} />
      </div>

      {/* Languages — functional */}
      <div className="flex items-center justify-between">
        <div>
          <p className="text-sm font-medium text-text-secondary">Languages</p>
          <p className="text-xs text-text-tertiary">The languages you speak</p>
        </div>
        <div className="flex items-center gap-3">
          <LanguageInlineDisplay codes={(() => {
            try { return JSON.parse(settings.languages || "[]"); }
            catch { return []; }
          })()} />
          <button
            onClick={(e) => {
              const btn = e.currentTarget;
              btn.classList.remove("btn-press");
              void btn.offsetWidth;
              btn.classList.add("btn-press");
              onOpenLanguageModal();
            }}
            className="px-3 py-1.5 bg-surface-elevated border border-border-strong rounded-sm text-xs font-medium text-text-secondary"
          >
            Change
          </button>
        </div>
      </div>

      <HotkeySetting value={settings.hotkey || "Ctrl+Win"} onSave={(value) => onSave("hotkey", value)} />
    </div>
  );
}
