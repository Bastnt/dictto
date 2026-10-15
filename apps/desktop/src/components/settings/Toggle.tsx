interface Props {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}

export function Toggle({ label, checked, onChange }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-label={label}
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className={`relative w-10 h-5 shrink-0 cursor-pointer rounded-full transition-colors motion-reduce:transition-none focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-accent ${
        checked ? "bg-accent" : "bg-surface-elevated"
      }`}
    >
      <span
        aria-hidden="true"
        className={`absolute top-0.5 left-0.5 w-4 h-4 bg-text rounded-full transition-transform motion-reduce:transition-none ${
          checked ? "translate-x-5" : ""
        }`}
      />
    </button>
  );
}
