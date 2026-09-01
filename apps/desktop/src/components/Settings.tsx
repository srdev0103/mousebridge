import { useState } from "react";
import { setSwitching } from "../ipc/commands";
import type { CoreStatus } from "../ipc/types";
import { Button } from "./Card";

/**
 * Edge-crossing tuning.
 *
 * These are exposed because the right values genuinely depend on the user's
 * mouse and how fast they move it — the defaults are a starting point, not a
 * measured optimum. The core validates whatever is entered, so a value the
 * engine would refuse cannot be saved here either.
 */
export function Settings({
  status,
  onChanged,
}: {
  status: CoreStatus;
  onChanged: () => void;
}) {
  const [overshoot, setOvershoot] = useState(12);
  const [cooldown, setCooldown] = useState(200);
  const [corner, setCorner] = useState(8);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const save = () => {
    setSwitching(overshoot, cooldown, corner)
      .then(() => {
        setError(null);
        setSaved(true);
        onChanged();
        window.setTimeout(() => setSaved(false), 2000);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  };

  return (
    <div className="space-y-4">
      <Field
        label="Edge push distance"
        hint="How far to keep pushing past a screen edge before switching. Higher values make accidental switches less likely."
        value={overshoot}
        min={0}
        max={100}
        unit="pt"
        onChange={setOvershoot}
      />
      <Field
        label="Switch cooldown"
        hint="Minimum time between switches. Prevents the pointer flipping back and forth at a shared edge."
        value={cooldown}
        min={0}
        max={2000}
        step={50}
        unit="ms"
        onChange={setCooldown}
      />
      <Field
        label="Corner dead zone"
        hint="Screen corners excluded from switching, so reaching for a menu bar or close button does not move you to another computer."
        value={corner}
        min={0}
        max={100}
        unit="pt"
        onChange={setCorner}
      />

      {error && <p className="text-sm text-red-500">{error}</p>}

      <div className="flex items-center gap-3">
        <Button variant="primary" onClick={save}>
          Save
        </Button>
        {saved && (
          <span className="text-sm text-emerald-600 dark:text-emerald-400">Saved</span>
        )}
        <span className="ml-auto text-xs text-[var(--color-ink-muted)]">
          Sharing is {status.sharing_enabled ? "on" : "off"}
        </span>
      </div>
    </div>
  );
}

function Field({
  label,
  hint,
  value,
  min,
  max,
  step = 1,
  unit,
  onChange,
}: {
  label: string;
  hint: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  unit: string;
  onChange: (value: number) => void;
}) {
  return (
    <div>
      <div className="flex items-baseline justify-between gap-3">
        <label className="text-sm font-medium">{label}</label>
        <span className="text-sm tabular-nums text-[var(--color-ink-muted)]">
          {value} {unit}
        </span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="mt-1.5 w-full cursor-pointer accent-blue-600"
      />
      <p className="mt-1 text-xs leading-snug text-[var(--color-ink-muted)]">{hint}</p>
    </div>
  );
}
