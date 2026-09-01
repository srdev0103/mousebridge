import type { SharingBlocker } from "../ipc/types";

/** What the user has to do, phrased as an action rather than a fault. */
function describe(blocker: SharingBlocker): { title: string; detail: string } {
  switch (blocker.kind) {
    case "missing-permission":
      return {
        title:
          blocker.permission === "accessibility"
            ? "Grant Accessibility"
            : "Grant Input Monitoring",
        detail: "Required before MouseBridge can read or move your pointer.",
      };
    case "no-peers":
      return {
        title: "Pair another computer",
        detail: "There is nothing to share input with yet.",
      };
    case "disabled-by-user":
      return {
        title: "Turn sharing on",
        detail: "Sharing is currently switched off.",
      };
    case "invalid-layout":
      return {
        title: "Fix the screen layout",
        detail: blocker.detail,
      };
  }
}

/**
 * Lists every reason sharing cannot happen.
 *
 * All of them at once, deliberately. Showing one at a time sends the user round
 * a loop of fixing something to be told about the next thing.
 */
export function Blockers({ blockers }: { blockers: SharingBlocker[] }) {
  if (blockers.length === 0) return null;

  return (
    <ul className="space-y-1.5">
      {blockers.map((blocker, index) => {
        const { title, detail } = describe(blocker);
        return (
          <li key={`${blocker.kind}-${index}`} className="flex gap-2.5 text-sm">
            <span aria-hidden className="mt-1.5 size-1.5 shrink-0 rounded-full bg-amber-500" />
            <span>
              <span className="font-medium">{title}</span>
              <span className="text-[var(--color-ink-muted)]"> — {detail}</span>
            </span>
          </li>
        );
      })}
    </ul>
  );
}
