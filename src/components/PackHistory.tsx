import { Select } from "./ui/select";
import { useState } from "react";
import { Archive, GitCompare, History } from "lucide-react";
import { call, displayDate, errorText, type Backup } from "../api";
import { useLauncher } from "../store";
import { ModHistory } from "./ModHistory";
import { PackDifferences, ReadState, formatPackSize, usePackQuery, type Difference } from "./PackTools";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export function PackHistory({ id, locked }: { id: string; locked: boolean }) {
  const [tab, setTab] = useState<"snapshots" | "changes">("snapshots");
  return <section className="pack-tool"><div className="tabs" aria-label="History type">
    <button aria-pressed={tab === "snapshots"} onClick={() => setTab("snapshots")}>Pack snapshots</button>
    <button aria-pressed={tab === "changes"} onClick={() => setTab("changes")}>Mod changes</button>
  </div>{tab === "snapshots" ? <Snapshots id={id} locked={locked} /> : <ModHistory id={id} locked={locked} />}</section>;
}

function Snapshots({ id, locked }: { id: string; locked: boolean }) {
  const history = usePackQuery<Backup[]>("list_pack_snapshots", { id });
  const [selected, select] = useState<string | null>(null);
  const [other, setOther] = useState<string | null>(null);
  const [showDiff, setShowDiff] = useState(false);
  const [confirmation, confirm] = useState<"restore" | "cleanup" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const items = history.value ?? [];
  const current = items.find((item) => item.id === selected) ?? items[0];
  const comparison = usePackQuery<Difference[]>("compare_pack_snapshot", { id, snapshot: current?.id, other }, undefined, !!current && showDiff);
  const cleanup = usePackQuery<Backup[]>("preview_snapshot_cleanup", { id }, undefined, confirmation === "cleanup");

  async function perform(action: "save" | "restore" | "cleanup") {
    setBusy(true);
    setError("");
    try {
      if (action === "save") {
        select(await call<string>("save_pack_snapshot", { id }));
      } else if (action === "restore" && current) {
        await call("restore_backup", { backupId: current.id });
        await useLauncher.getState().refreshInstances();
      } else if (action === "cleanup" && cleanup.value?.length) {
        await call("cleanup_snapshots", { id, snapshots: cleanup.value.map((item) => item.id) });
      }
      history.refresh();
      comparison.refresh();
      confirm(null);
      useLauncher.getState().notify(action === "restore" ? "Pack snapshot restored." : action === "cleanup" ? "Selected snapshots removed." : "Pack snapshot saved.", "success");
    } catch (cause) { setError(errorText(cause)); }
    finally { setBusy(false); }
  }

  return <>
    <div className="section-toolbar"><h2><History size={19} /> Pack snapshots</h2><div className="actions-wrap">
      <Button disabled={locked || busy} onClick={() => void perform("save")}><Archive /> Save snapshot</Button>
      <Button disabled={locked || busy || !items.length} onClick={() => { setError(""); confirm("cleanup"); }}>Review retention</Button>
    </div></div>
    <p className="field-hint">Snapshots cover modpack files and settings. World backups are available in Library → Backups.</p>
    {error && !confirmation && <p className="inline-error" role="alert">{error}</p>}
    <ReadState query={history} label="Reading pack history…" />
    {history.value && !items.length && <div className="empty"><Archive /><h3>No pack snapshots yet</h3><p>Voltra creates snapshots before changing mods. You can also save one here.</p></div>}
    {current && <section className="panel detail-panel">
      <p>{items.length} snapshots · {formatPackSize(items.reduce((total, item) => total + item.bytes, 0))}</p>
      <div className="form-grid"><label>Snapshot<Select value={current.id} onValueChange={(value) => { select(value); setOther(null); }}>
        {items.map((item) => <option key={item.id} value={item.id}>{displayDate(item.createdAt)} · {item.minecraftVersion} · {formatPackSize(item.bytes)}</option>)}
      </Select></label><label>Compare with<Select value={other ?? ""} onValueChange={(value) => setOther(value || null)}>
        <option value="">Current files</option>{items.filter((item) => item.id !== current.id).map((item) => <option value={item.id} key={item.id}>{displayDate(item.createdAt)}</option>)}
      </Select></label></div>
      <div className="actions-wrap"><Button onClick={() => { if (showDiff) comparison.refresh(); setShowDiff(true); }}><GitCompare /> Compare files</Button>
        <Button disabled={locked || busy} onClick={() => { setError(""); confirm("restore"); }}>Restore snapshot</Button></div>
      {showDiff && <><ReadState query={comparison} label="Comparing file checksums…" />{comparison.value && <PackDifferences changes={comparison.value} />}</>}
    </section>}
    {confirmation && <Dialog open title={confirmation === "cleanup" ? "Review snapshot retention" : "Restore pack snapshot?"}
      description={confirmation === "cleanup" ? "The listed snapshots exceed your count or storage budget. Removing a snapshot can also remove the files needed to undo its mod changes." : "Voltra saves the current pack first, then restores the selected pack files and settings. Worlds stay in their existing folders."}
      onOpenChange={(open) => { if (!open && !busy) confirm(null); }}>
      {confirmation === "cleanup" && <><ReadState query={cleanup} />{cleanup.value && (cleanup.value.length ? <ul>{cleanup.value.map((item) => <li key={item.id}>{displayDate(item.createdAt)} · {formatPackSize(item.bytes)}</li>)}</ul> : <p>Your snapshots are within the configured budget.</p>)}</>}
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="dialog-actions"><Button disabled={busy} variant="ghost" onClick={() => confirm(null)}>Cancel</Button>
        <Button disabled={locked || busy || (confirmation === "cleanup" && !cleanup.value?.length)} variant={confirmation === "cleanup" ? "danger" : "primary"}
          onClick={() => void perform(confirmation)}>{busy ? "Working…" : confirmation === "cleanup" ? "Remove listed snapshots" : "Restore pack"}</Button></div>
    </Dialog>}
  </>;
}
