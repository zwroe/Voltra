import { useState } from "react";
import { call, errorText, platformNames, type Instance, type Project } from "../api";
import { runAction, useLauncher } from "../store";
import { PackDifferences, ReadState, usePackQuery, type Difference } from "./PackTools";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export function ComparePacks({ id, locked }: { id: string; locked: boolean }) {
  const instances = useLauncher((state) => state.instances);
  const [source, setSource] = useState("");
  const [compare, setCompare] = useState(false);
  const [selected, select] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState("");
  const [limit, setLimit] = useState(100);
  const diff = usePackQuery<Difference[]>("compare_instances", { id, source }, undefined, compare && !!source);
  const files = diff.value?.filter((entry) => selected.has(entry.path) && entry.after) ?? [];
  const others = instances.filter((item) => item.id !== id);
  const matching = diff.value?.filter((entry) => entry.path.toLowerCase().includes(filter.toLowerCase())) ?? [];
  async function merge() {
    setBusy(true);
    setError("");
    try {
      await call("merge_instances", { id, source, files });
      select(new Set());
      setConfirm(false);
      diff.refresh();
      await useLauncher.getState().refreshInstances();
      useLauncher.getState().notify("Selected pack files copied.", "success");
    } catch (cause) { setError(errorText(cause)); }
    finally { setBusy(false); }
  }
  return <section className="panel detail-panel"><h2>Compare and merge packs</h2>
    {others.length ? <><label>Source instance<select value={source} onChange={(event) => { setSource(event.target.value); setCompare(false); select(new Set()); }}>
      <option value="">Select another pack</option>{others.map((item) => <option key={item.id} value={item.id}>{item.name} · {item.minecraftVersion}</option>)}
    </select></label><Button disabled={!source} onClick={() => { if (compare) diff.refresh(); select(new Set()); setCompare(true); }}>Compare files</Button>
      <ReadState query={diff} label="Comparing pack checksums…" />{diff.value && <>
        <label>Filter file paths<input value={filter} onChange={(event) => { setFilter(event.target.value); setLimit(100); }} /></label>
        <PackDifferences changes={matching.slice(0, limit)} selections={selected} onSelect={(path) => select((before) => {
          const next = new Set(before); if (next.has(path)) next.delete(path); else next.add(path); return next;
        })} />
        <div className="actions-wrap">{matching.length > limit && <Button onClick={() => setLimit((value) => value + 100)}>Show more files</Button>}
          <Button disabled={!files.length || locked} onClick={() => { setError(""); setConfirm(true); }}>Review copy · {files.length} selected</Button></div>
      </>}
    </> : <p className="muted">Create or import another instance to compare pack files.</p>}
    {confirm && <Dialog open title="Copy selected files into this pack?" description="Voltra saves a destination snapshot and then copies the listed files. Matching destination files will be replaced."
      onOpenChange={(open) => { if (!open && !busy) setConfirm(false); }}>
      <ul className="pack-review">{files.map((item) => <li key={item.path}>{item.before ? "Replace" : "Add"} {item.path}</li>)}</ul>
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="dialog-actions"><Button disabled={busy} variant="ghost" onClick={() => setConfirm(false)}>Cancel</Button>
        <Button disabled={busy || locked} variant="primary" onClick={() => void merge()}>Copy files</Button></div>
    </Dialog>}
  </section>;
}

interface MigrationEntry { file: string; name: string; version: string | null; state: string; reason: string | null }

export function Migration({ instance }: { instance: Instance }) {
  const [game, setGame] = useState("");
  const [target, setTarget] = useState("");
  const [file, setFile] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [filter, setFilter] = useState("");
  const preview = usePackQuery<MigrationEntry[]>("preview_pack_migration", { id: instance.id, game: target }, undefined, !!target);
  const alternatives = usePackQuery<Project[]>("find_migration_candidates", { id: instance.id, game: target, file }, undefined, !!file && !!target);
  const items = preview.value?.filter((item) => (showAll || item.state !== "available") && item.name.toLowerCase().includes(filter.toLowerCase())) ?? [];
  return <section className="panel detail-panel"><h2>Check a Minecraft upgrade</h2>
    <p className="muted">Check which installed mods have usable catalog builds for a target Minecraft version with this pack’s loader.</p>
    <form className="actions-wrap" onSubmit={(event) => { event.preventDefault(); setFile(""); if (target === game.trim()) preview.refresh(); setTarget(game.trim()); }}>
      <label>Target Minecraft version<input value={game} onChange={(event) => setGame(event.target.value)} placeholder="1.21.1" required maxLength={32} /></label>
      <Button type="submit" disabled={!game.trim() || preview.loading}>Check versions</Button>
    </form>
    <ReadState query={preview} label="Checking the catalogs for compatible versions…" />
    {preview.value && <><p>{preview.value.filter((entry) => entry.state === "available").length} of {preview.value.length} installed mods have verified matches for {target}.</p>
      <div className="actions-wrap"><label className="inline-checkbox"><input type="checkbox" checked={showAll} onChange={(event) => setShowAll(event.target.checked)} /> Include verified matches</label>
        <input aria-label="Filter migration results" placeholder="Find a mod" value={filter} onChange={(event) => setFilter(event.target.value)} /></div>
      {items.length ? <ul className="rows">{items.map((item) => <li key={item.file} className="data-row"><div className="row-main"><strong>{item.name}</strong><p className="muted">{item.version ?? item.reason}</p></div>
        {item.state !== "available" && <Button size="small" onClick={() => { if (file === item.file) alternatives.refresh(); setFile(item.file); }}>Find candidates</Button>}
      </li>)}</ul> : <p>No matching results to display.</p>}
    </>}
    {file && <section className="panel detail-panel"><h3>Compatible search results</h3><p className="muted">Review features before treating a search match as a replacement.</p><ReadState query={alternatives} />
      {alternatives.value && (alternatives.value.length ? <ul className="rows">{alternatives.value.map((item) => <li className="data-row" key={`${item.platform}-${item.id}`}><div className="row-main"><strong>{item.title}</strong><p>{item.description}</p><p className="muted">{platformNames[item.platform]}</p></div>
        {item.url && <Button size="small" onClick={() => void runAction(() => call("open_project_url", { url: item.url }))}>View project</Button>}</li>)}</ul> : <p>No verified candidates were found.</p>)}
    </section>}
  </section>;
}
