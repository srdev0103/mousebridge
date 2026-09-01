import { beginPairing, forgetDevice } from "../ipc/engine";
import type { EngineSnapshot } from "../ipc/engine";

/** Machines seen on the local network, paired or not. */
export function Network({
  snapshot,
  onChanged,
}: {
  snapshot: EngineSnapshot;
  onChanged: () => void;
}) {
  if (snapshot.discovered.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-[var(--color-line)] px-4 py-6 text-center">
        <p className="text-sm font-medium">Looking for other computers…</p>
        <p className="mt-1 text-sm text-[var(--color-ink-muted)]">
          Run MouseBridge on your other computer, on the same network. It should
          appear here within a few seconds.
        </p>
      </div>
    );
  }

  return (
    <ul className="space-y-2">
      {snapshot.discovered.map((peer) => (
        <li
          key={peer.id_short}
          className="flex items-center gap-3 rounded-lg border border-[var(--color-line)] px-3 py-2.5"
        >
          <span
            aria-hidden
            className={`size-2 shrink-0 rounded-full ${
              peer.connected
                ? "bg-emerald-500"
                : peer.paired
                  ? "bg-amber-500"
                  : "bg-[var(--color-ink-muted)]"
            }`}
          />
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-medium">{peer.name}</p>
            <p className="text-xs text-[var(--color-ink-muted)]">
              {peer.connected
                ? "Connected"
                : peer.paired
                  ? "Paired — connecting…"
                  : "Not paired"}
              <span className="select-text"> · {peer.address}</span>
            </p>
          </div>
          {peer.paired ? (
            <button
              type="button"
              onClick={() => void forgetDevice(peer.id_short).then(onChanged)}
              className="shrink-0 cursor-pointer rounded-md border border-[var(--color-line)] px-2.5 py-1 text-xs font-medium"
            >
              Forget
            </button>
          ) : (
            <button
              type="button"
              disabled={snapshot.pairing !== null}
              onClick={() => void beginPairing(peer.id_short).then(onChanged)}
              className="shrink-0 cursor-pointer rounded-md bg-blue-600 px-2.5 py-1 text-xs font-medium text-white disabled:opacity-40"
            >
              Pair
            </button>
          )}
        </li>
      ))}
    </ul>
  );
}
