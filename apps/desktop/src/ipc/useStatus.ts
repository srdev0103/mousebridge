import { useCallback, useEffect, useRef, useState } from "react";
import { getStatus } from "./commands";
import type { CoreStatus } from "./types";

/**
 * Polling interval for the status snapshot.
 *
 * Neither macOS nor Windows notifies an application when a privacy grant
 * changes, so a permission that the user has just switched on in System Settings
 * only becomes visible by asking again. Two seconds is fast enough that the
 * dashboard updates while the user is still looking at it, and slow enough that
 * the cost is irrelevant — the query is a handful of cheap OS calls.
 */
const POLL_MS = 2000;

export interface StatusState {
  status: CoreStatus | null;
  error: string | null;
  refresh: () => Promise<void>;
}

export function useStatus(): StatusState {
  const [status, setStatus] = useState<CoreStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Avoids a state update after unmount when a poll is in flight.
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await getStatus();
      if (!alive.current) return;
      setStatus(next);
      setError(null);
    } catch (e) {
      if (!alive.current) return;
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
    };
  }, [refresh]);

  return { status, error, refresh };
}
