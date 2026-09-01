import { useState } from "react";
import { revealConfig, setDeviceName } from "./ipc/commands";
import { useStatus } from "./ipc/useStatus";
import type { StartupNotice } from "./ipc/types";
import { Button, Card, Row } from "./components/Card";
import { DisplayMap } from "./components/DisplayMap";
import { PermissionList } from "./components/PermissionList";

export default function App() {
  const { status, error, refresh } = useStatus();
  const [draftName, setDraftName] = useState<string | null>(null);

  if (error && !status) {
    return (
      <Shell>
        <Card title="Cannot reach the core">
          <p className="text-sm text-red-500">{error}</p>
        </Card>
      </Shell>
    );
  }

  if (!status) {
    return (
      <Shell>
        <p className="text-sm text-[var(--color-ink-muted)]">Starting…</p>
      </Shell>
    );
  }

  const name = draftName ?? status.device.name;

  const commitName = () => {
    const trimmed = name.trim();
    setDraftName(null);
    if (trimmed && trimmed !== status.device.name) {
      void setDeviceName(trimmed).then(() => refresh());
    }
  };

  return (
    <Shell>
      {status.notice && <NoticeBanner notice={status.notice} />}

      <div
        className={`flex items-center gap-2 rounded-lg border px-4 py-3 text-sm ${
          status.sharing_ready
            ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
            : "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300"
        }`}
      >
        <span
          aria-hidden
          className={`size-2 rounded-full ${
            status.sharing_ready ? "bg-emerald-500" : "bg-amber-500"
          }`}
        />
        {status.sharing_ready
          ? "Ready to share input. Networking arrives in a later milestone."
          : "Setup incomplete — grant the permissions below before sharing can start."}
      </div>

      <Card title="This Computer">
        <div className="mb-3">
          <label
            htmlFor="device-name"
            className="mb-1 block text-xs text-[var(--color-ink-muted)]"
          >
            Name shown to other computers
          </label>
          <input
            id="device-name"
            value={name}
            onChange={(e) => setDraftName(e.target.value)}
            onBlur={commitName}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            maxLength={64}
            className="w-full cursor-text rounded-lg border border-[var(--color-line)] bg-[var(--color-surface)] px-3 py-2 text-sm select-text focus:border-blue-500 focus:outline-none"
          />
        </div>
        <Row label="Operating system" value={`${status.device.os} ${status.device.os_version}`} />
        <Row label="Architecture" value={status.device.arch} />
        <Row
          label="Device ID"
          value={<code className="text-xs select-text">{status.device.id_short}</code>}
        />
      </Card>

      <Card title="Permissions">
        <PermissionList permissions={status.permissions} onChanged={() => void refresh()} />
      </Card>

      <Card title={`Displays (${status.displays.length})`}>
        {status.display_error ? (
          <p className="text-sm text-red-500">
            Could not read the display list: {status.display_error}
          </p>
        ) : (
          <DisplayMap displays={status.displays} />
        )}
      </Card>

      <Card
        title="Configuration"
        action={<Button onClick={() => void revealConfig()}>Reveal in Finder</Button>}
      >
        <p className="text-xs break-all text-[var(--color-ink-muted)] select-text">
          {status.config_path}
        </p>
      </Card>
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <main className="mx-auto flex max-w-2xl flex-col gap-4 p-6">
      <header className="mb-1 flex items-baseline justify-between">
        <h1 className="text-xl font-semibold tracking-tight">MouseBridge</h1>
        <span className="text-xs text-[var(--color-ink-muted)]">Milestone 1 · foundation</span>
      </header>
      {children}
    </main>
  );
}

function NoticeBanner({ notice }: { notice: StartupNotice }) {
  const text =
    notice.kind === "first-run"
      ? "Welcome — a new configuration was created for this computer."
      : notice.kind === "migrated"
        ? `Settings were upgraded from an older version (v${notice.from_version}).`
        : `Your settings could not be read and have been reset. The previous file was kept at ${notice.backup_path} (${notice.reason}).`;

  const severe = notice.kind === "recovered";

  return (
    <div
      className={`rounded-lg border px-4 py-3 text-sm ${
        severe
          ? "border-red-500/40 bg-red-500/10 text-red-700 dark:text-red-300"
          : "border-blue-500/40 bg-blue-500/10 text-blue-700 dark:text-blue-300"
      }`}
    >
      <span className="select-text">{text}</span>
    </div>
  );
}
