import type { PeerStatus } from "../ipc/types";

/**
 * The computers this machine is sharing with.
 *
 * An unreachable peer is shown differently from a disconnected one on purpose:
 * the machine is fine and the route to it is gone, so the fix is to rearrange
 * screens rather than to go looking for a network problem.
 */
export function PeerList({ peers }: { peers: PeerStatus[] }) {
  if (peers.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-[var(--color-line)] px-4 py-6 text-center">
        <p className="text-sm font-medium">No other computers yet</p>
        <p className="mt-1 text-sm text-[var(--color-ink-muted)]">
          Run MouseBridge on another computer on this network, then pair it.
        </p>
      </div>
    );
  }

  return (
    <ul className="space-y-2">
      {peers.map((peer) => (
        <li
          key={peer.id_short}
          className={`flex items-center gap-3 rounded-lg border px-3 py-2.5 ${
            peer.active
              ? "border-blue-500/50 bg-blue-500/5"
              : "border-[var(--color-line)]"
          }`}
        >
          <span
            aria-hidden
            className={`size-2 shrink-0 rounded-full ${
              peer.unreachable
                ? "bg-amber-500"
                : peer.state === "connected"
                  ? "bg-emerald-500"
                  : "bg-amber-500"
            }`}
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-baseline gap-2">
              <span className="truncate text-sm font-medium">{peer.name}</span>
              {peer.active && (
                <span className="rounded bg-blue-500/15 px-1.5 py-0.5 text-[11px] font-semibold text-blue-600 dark:text-blue-400">
                  Receiving input
                </span>
              )}
            </div>
            <p className="mt-0.5 text-xs text-[var(--color-ink-muted)]">
              {peer.unreachable
                ? "Connected, but no screen edge leads to it — check the layout"
                : peer.state === "degraded"
                  ? `Not responding (${peer.missed_heartbeats} missed)`
                  : `${peer.screen_count} screen${peer.screen_count === 1 ? "" : "s"}`}
              {peer.latency_ms !== null && !peer.unreachable && (
                <span className="tabular-nums"> · {peer.latency_ms} ms</span>
              )}
            </p>
          </div>
          <code className="shrink-0 text-[11px] text-[var(--color-ink-muted)] select-text">
            {peer.id_short}
          </code>
        </li>
      ))}
    </ul>
  );
}
