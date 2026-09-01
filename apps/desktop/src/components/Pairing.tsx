import { confirmPairing, rejectPairing } from "../ipc/engine";
import type { PairingView } from "../ipc/engine";

/**
 * The verification code, shown on both machines.
 *
 * The wording matters as much as the code. A user who confirms without comparing
 * gets no protection at all, so the prompt asks for the comparison explicitly
 * rather than presenting a code and an OK button.
 */
export function Pairing({
  pairing,
  onChanged,
}: {
  pairing: PairingView;
  onChanged: () => void;
}) {
  return (
    <div className="rounded-lg border-2 border-blue-500 bg-blue-500/5 p-5 text-center">
      <p className="text-sm text-[var(--color-ink-muted)]">Pairing with</p>
      <p className="mt-0.5 text-base font-semibold">{pairing.peer_name}</p>

      <p className="mt-4 font-mono text-4xl font-bold tracking-[0.15em] tabular-nums select-text">
        {pairing.code}
      </p>

      <p className="mx-auto mt-4 max-w-xs text-sm leading-snug">
        Check that <strong>the same six digits</strong> are showing on the other
        computer's screen.
      </p>
      <p className="mx-auto mt-1 max-w-xs text-xs text-[var(--color-ink-muted)]">
        If they differ, something is intercepting the connection. Cancel.
      </p>

      <div className="mt-4 flex justify-center gap-2">
        <button
          type="button"
          onClick={() => void rejectPairing().then(onChanged)}
          className="cursor-pointer rounded-lg border border-[var(--color-line)] px-4 py-2 text-sm font-medium"
        >
          Cancel
        </button>
        <button
          type="button"
          disabled={pairing.confirmed_here}
          onClick={() => void confirmPairing().then(onChanged)}
          className="cursor-pointer rounded-lg bg-blue-600 px-4 py-2 text-sm font-medium text-white disabled:cursor-default disabled:opacity-50"
        >
          {pairing.confirmed_here ? "Waiting for the other computer…" : "They match"}
        </button>
      </div>

      {pairing.confirmed_there && !pairing.confirmed_here && (
        <p className="mt-3 text-xs text-[var(--color-ink-muted)]">
          The other computer has confirmed. Your turn.
        </p>
      )}
    </div>
  );
}
