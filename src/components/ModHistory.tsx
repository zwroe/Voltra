import { useEffect, useState } from "react";
import { ArrowRight, History, LoaderCircle, RotateCcw } from "lucide-react";
import { call, displayDate, errorText, type ModHistoryEntry, type ModUpdatePreview } from "../api";
import { useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

const actions = { install: "Installed mods", update: "Updated mods", remove: "Removed mods" };

export function ModHistory({ id, locked }: { id: string; locked: boolean }) {
  const [entries, setEntries] = useState<ModHistoryEntry[] | null>(null);
  const [error, setError] = useState("");
  const [selected, setSelected] = useState<{ entry: string; index: number; title: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const lastJob = useLauncher((state) => state.jobs.find((job) => job.instanceId === id && job.status === "done")?.id);
  useEffect(() => {
    let live = true;
    setError("");
    call<ModHistoryEntry[]>("mod_history", { id }).then((result) => {
      if (live) setEntries(result);
    }).catch((reason) => {
      if (live) setError(errorText(reason));
    });
    return () => { live = false; };
  }, [id, lastJob, refresh]);

  async function undo() {
    if (!selected || busy || locked) return;
    setBusy(true);
    setError("");
    try {
      await call("undo_mod_change", { id, entry: selected.entry, change: selected.index });
      setSelected(null);
      setRefresh((value) => value + 1);
      useLauncher.getState().notify("Mod change undone.", "success");
      await useLauncher.getState().refreshInstances();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  return <section aria-label="Mod history">
    <div className="section-toolbar"><h2>Mod history</h2>
      <Button size="small" disabled={busy} onClick={() => setRefresh((value) => value + 1)}><RotateCcw /> Refresh</Button>
    </div>
    {error && !selected && <p className="inline-error" role="alert">{error}</p>}
    {entries === null ? !error && <p className="waiting">Loading mod history.</p>
      : !entries.length ? <div className="empty"><History /><h2>No mod changes recorded</h2>
        <p>Installs, updates, and removals made in Voltra will appear here.</p></div>
      : <ol className="mod-history-list">{entries.map((entry) => <li className="panel" key={entry.id}>
        <header><strong>{actions[entry.action]}</strong><time dateTime={new Date(entry.at * 1000).toISOString()}>{displayDate(entry.at)}</time></header>
        <ul>{entry.changes.map((change, index) => <li className="mod-history-change" key={index}>
          <div className="row-main"><strong>{change.title}</strong><p className="muted">
            {change.before && (change.before.version || change.before.file)}
            {change.before && change.after && <ArrowRight size={13} />}
            {change.after && (change.after.version || change.after.file)}
          </p></div>
          {change.undone ? <span className="status-tag">Undone</span>
            : <Button size="small" variant="ghost" disabled={locked || busy || !entry.compatible}
              title={entry.compatible ? undefined : "This change was made for a different Minecraft version or loader."}
              aria-label={"Undo change to " + change.title}
              onClick={() => { setError(""); setSelected({ entry: entry.id, index, title: change.title }); }}><RotateCcw /> Undo</Button>}
        </li>)}</ul>
      </li>)}</ol>}
    {selected && <Dialog open title={"Undo change to " + selected.title + "?"}
      description="Voltra will reverse this mod change and any linked dependency changes from the same update. World saves stay intact."
      onOpenChange={(open) => { if (!open && !busy) { setSelected(null); setError(""); } }}>
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="dialog-actions">
        <Button variant="ghost" disabled={busy} onClick={() => { setSelected(null); setError(""); }}>Cancel</Button>
        <Button variant="primary" disabled={locked || busy} onClick={() => void undo()}>{busy ? "Undoing…" : "Undo change"}</Button>
      </div>
    </Dialog>}
  </section>;
}

export function ModUpdateDialog({ id, file, title, locked, onClose }: {
  id: string; file: string | null; title?: string; locked: boolean; onClose: () => void;
}) {
  const [preview, setPreview] = useState<ModUpdatePreview | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState<"apply" | "test" | null>(null);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let live = true;
    setPreview(null);
    setError("");
    call<ModUpdatePreview>("preview_mod_updates", { id, file }).then((data) => {
      if (live) setPreview(data);
    }).catch((reason) => {
      if (live) setError(errorText(reason));
    });
    return () => { live = false; };
  }, [id, file, retry]);

  async function apply(testCopy: boolean) {
    if (!preview?.changes.length || locked || busy) return;
    setBusy(testCopy ? "test" : "apply");
    setError("");
    try {
      const target = await call<string>("apply_mod_preview", { id, token: preview.token, testCopy });
      const state = useLauncher.getState();
      await state.poll().catch(() => {});
      if (testCopy) {
        await state.refreshInstances();
        state.view(target);
      }
      onClose();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(null);
    }
  }

  return <Dialog open title={title ? "Preview update for " + title : "Preview mod updates"}
    description="Review versions and dependencies before applying changes. Updates can affect saved worlds. Use a test copy for risky changes and back up the original worlds before playing."
    className="project-details-dialog" onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
    {error && <div className="project-load-error"><p className="inline-error" role="alert">{error}</p>
      <Button size="small" disabled={!!busy || locked} onClick={() => setRetry((value) => value + 1)}>Check again</Button></div>}
    {!preview && !error && <p className="waiting" role="status"><LoaderCircle className="spin" /> Checking compatible versions.</p>}
    {preview && (!preview.changes.length ? <p className="muted">No updates are available for the selected channel.</p>
      : <ul className="rows mod-update-list">{preview.changes.map((change) => <li className="mod-history-change" key={change.file}>
        <div className="row-main"><strong>{change.title}</strong><p className="muted" title={change.file}>
          {change.previous ? <>{change.previous} <ArrowRight size={13} /> {change.next}</> : <>Add {change.next}</>}
        </p></div>
      </li>)}</ul>)}
    <div className="dialog-actions">
      <Button variant="ghost" disabled={!!busy} onClick={onClose}>{preview?.changes.length ? "Cancel" : "Close"}</Button>
      {!!preview?.changes.length && <>
        <Button disabled={locked || !!busy || !!error} onClick={() => void apply(true)}>{busy === "test" ? "Creating copy…" : "Test in a copy"}</Button>
        <Button variant="primary" disabled={locked || !!busy || !!error} onClick={() => void apply(false)}>{busy === "apply" ? "Starting…" : "Apply updates"}</Button>
      </>}
    </div>
  </Dialog>;
}
