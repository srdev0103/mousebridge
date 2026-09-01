import { openPermissionSettings, requestPermission } from "../ipc/commands";
import type { PermissionEntry } from "../ipc/types";
import { Button } from "./Card";

/**
 * Renders the permission gates.
 *
 * The rule this encodes: when a permission is denied, macOS will not show the
 * prompt again, so offering a "Grant" button that appears to do nothing is worse
 * than useless. `can_prompt` comes from Rust and decides which affordance the
 * user sees.
 */
export function PermissionList({
  permissions,
  onChanged,
}: {
  permissions: PermissionEntry[];
  onChanged: () => void;
}) {
  if (permissions.length === 0) {
    return (
      <p className="text-sm text-[var(--color-ink-muted)]">
        This platform needs no additional permissions.
      </p>
    );
  }

  return (
    <ul className="space-y-4">
      {permissions.map((p) => (
        <li key={p.id} className="flex items-start gap-3">
          <span
            aria-hidden
            className={`mt-1.5 size-2 shrink-0 rounded-full ${
              p.granted ? "bg-emerald-500" : "bg-amber-500"
            }`}
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">{p.title}</span>
              <span
                className={`rounded px-1.5 py-0.5 text-[11px] font-semibold ${
                  p.granted
                    ? "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
                    : "bg-amber-500/15 text-amber-600 dark:text-amber-400"
                }`}
              >
                {p.granted ? "Granted" : "Required"}
              </span>
            </div>
            <p className="mt-1 text-sm leading-snug text-[var(--color-ink-muted)]">
              {p.rationale}
            </p>
            {!p.granted && (
              <div className="mt-2 flex gap-2">
                {p.can_prompt && (
                  <Button
                    variant="primary"
                    onClick={() => void requestPermission(p.id).then(onChanged)}
                  >
                    Grant Access
                  </Button>
                )}
                <Button onClick={() => void openPermissionSettings(p.id).then(onChanged)}>
                  Open System Settings
                </Button>
              </div>
            )}
          </div>
        </li>
      ))}
    </ul>
  );
}
