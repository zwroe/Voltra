import { useMemo, useState } from "react";
import { ArrowRight, GitCompare, GitFork, HeartPulse, Search } from "lucide-react";
import { call, errorText, type Instance } from "../api";
import { useLauncher } from "../store";
import { ModUpdateDialog } from "./ModHistory";
import { ComparePacks, Migration } from "./PackTransfers";
import { ReadState, usePackQuery } from "./PackTools";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export interface HealthNode { file: string; name: string; modId: string | null; version: string | null; enabled: boolean; edges: Array<{ target: string | null; requirement: string; incompatible: boolean }> }
export interface HealthReport { nodes: HealthNode[]; issues: Array<{ level: string; title: string; file: string; evidence: string; recommendation: string }>; metadataCount: number; enableCandidates: string[] }
export interface WorldRisk { hasWorlds: boolean; baselineAvailable: boolean; removed: string[]; changed: string[] }

export function PackHealth({ instance, locked }: { instance: Instance; locked: boolean }) {
  const [tab, select] = useState<"health" | "dependencies" | "compare" | "migration">("health");
  return <section className="pack-tool"><div className="tabs" aria-label="Pack tools">
    <button aria-pressed={tab === "health"} onClick={() => select("health")}><HeartPulse size={15} /> Health check</button>
    <button aria-pressed={tab === "dependencies"} onClick={() => select("dependencies")}><GitFork size={15} /> Dependencies</button>
    <button aria-pressed={tab === "compare"} onClick={() => select("compare")}><GitCompare size={15} /> Compare packs</button>
    <button aria-pressed={tab === "migration"} onClick={() => select("migration")}>Migration</button>
  </div>{tab === "compare" ? <ComparePacks id={instance.id} locked={locked} />
    : tab === "migration" ? <Migration instance={instance} /> : <HealthScan id={instance.id} locked={locked} graph={tab === "dependencies"} />}</section>;
}

function HealthScan({ id, locked, graph }: { id: string; locked: boolean; graph: boolean }) {
  const [fix, setFix] = useState(false);
  const [checking, check] = useState(false);
  const [risk, setRisk] = useState(false);
  const [update, setUpdate] = useState(false);
  const settled = useLauncher((state) => state.jobs.find((job) => job.instanceId === id && job.status === "done")?.id);
  const query = usePackQuery<HealthReport>("inspect_modpack_health", { id }, settled);
  const updates = usePackQuery<{ matches: string[]; unchecked: number }>("mod_filter_data", { id, filter: "updates" }, settled, checking && !graph);
  const worlds = usePackQuery<WorldRisk>("inspect_world_risk", { id }, undefined, risk && !graph);
  const value = query.value;
  return <>
    <div className="section-toolbar"><h2>{graph ? "Mod dependency explorer" : "Modpack health"}</h2><Button size="small" onClick={query.refresh}>Rescan installed files</Button></div>
    <ReadState query={query} label="Reading installed mod metadata…" />
    {value && (graph ? <DependencyGraph nodes={value.nodes} /> : <>
      <section className="panel detail-panel"><h3>{value.issues.length ? `${value.issues.length} findings to review` : "No issues found in the available metadata"}</h3>
        <p className="muted">{value.metadataCount} of {value.nodes.length} installed mods exposed dependency or catalog metadata.</p>
        <div className="actions-wrap">
          <Button disabled={locked || !value.enableCandidates.length} onClick={() => setFix(true)}>Preview verified fixes</Button>
          <Button onClick={() => { if (checking) updates.refresh(); check(true); }}>Check for updates</Button>
          <Button onClick={() => { if (risk) worlds.refresh(); setRisk(true); }}>Check world compatibility</Button>
        </div>
        {value.issues.length > 0 && <ul className="rows">{value.issues.map((issue, index) => <li className="data-row" key={`${issue.file}-${index}`}>
          <span className={`status-tag ${issue.level === "error" ? "red" : "yellow"}`}>{issue.level === "error" ? "Declared issue" : "Review"}</span>
          <div className="row-main"><strong>{issue.title}</strong><p>{issue.file} · {issue.evidence}</p><p className="muted">{issue.recommendation}</p></div>
        </li>)}</ul>}
      </section>
      {checking && <section className="panel detail-panel"><h3>Catalog updates</h3><ReadState query={updates} label="Checking compatible catalog versions…" />
        {updates.value && <><p>{updates.value.matches.length} compatible updates found.{updates.value.unchecked > 0 && ` ${updates.value.unchecked} projects could not be checked.`}</p>
          {!!updates.value.matches.length && <><details><summary>Files with updates</summary><ul>{updates.value.matches.map((file) => <li key={file}>{file}</li>)}</ul></details>
            <Button disabled={locked} onClick={() => setUpdate(true)}>Preview updates</Button></>}
        </>}
      </section>}
      {risk && <section className="panel detail-panel"><h3>World compatibility</h3><ReadState query={worlds} />{worlds.value && <WorldRiskDetails risk={worlds.value} />}</section>}
    </>)}
    {fix && <HealthFixDialog id={id} locked={locked} onClose={() => setFix(false)} onApplied={() => query.refresh()} />}
    {update && <ModUpdateDialog id={id} file={null} locked={locked} onClose={() => setUpdate(false)} />}
  </>;
}

export function WorldRiskDetails({ risk }: { risk: WorldRisk }) {
  return !risk.hasWorlds ? <p>No local Minecraft worlds were detected.</p> : !risk.baselineAvailable || risk.removed.length || risk.changed.length ? <>
      <p>Mods and game versions can affect blocks, items, entities, structures, and dimensions. Back up your worlds before opening them with this pack.</p>
      {!risk.baselineAvailable && <p className="muted">At least one world has no recorded baseline. Voltra records one when Quick Play observes that world in a successful session.</p>}
      {!!risk.removed.length && <details open><summary>{risk.removed.length} previously enabled JARs removed or disabled</summary><ul>{risk.removed.map((name) => <li key={name}>{name}</li>)}</ul></details>}
      {!!risk.changed.length && <details><summary>{risk.changed.length} changed files or versions</summary><ul>{risk.changed.map((name) => <li key={name}>{name}</li>)}</ul></details>}
    </> : <p>Enabled JARs, loader, and Minecraft version match the most recent successful session.</p>;
}

export function HealthFixDialog({ id, locked, onClose, onApplied }: { id: string; locked: boolean; onClose: () => void; onApplied?: () => void }) {
  const fixes = usePackQuery<string[]>("preview_health_fixes", { id });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function apply() {
    setBusy(true);
    setError("");
    try {
      await call("apply_health_fixes", { id, files: fixes.value });
      onApplied?.();
      onClose();
      useLauncher.getState().notify("Required mods enabled.", "success");
    } catch (cause) { setError(errorText(cause)); fixes.refresh(); }
    finally { setBusy(false); }
  }
  return <Dialog open title="Review verified fixes" description="Voltra can enable installed catalog dependencies after checking the proposed change. It saves a pack snapshot first."
    onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
    <ReadState query={fixes} />
    {fixes.value?.length ? <ul>{fixes.value.map((file) => <li key={file}>Enable {file}</li>)}</ul> : fixes.value && <p>No eligible dependency fixes were found. Review the findings for manual steps.</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="dialog-actions"><Button disabled={busy} variant="ghost" onClick={onClose}>Cancel</Button>
      <Button disabled={busy || locked || !fixes.value?.length} variant="primary" onClick={() => void apply()}>Apply fixes</Button></div>
  </Dialog>;
}

function DependencyGraph({ nodes }: { nodes: HealthNode[] }) {
  const [search, setSearch] = useState("");
  const [selected, select] = useState("");
  const [limit, setLimit] = useState(40);
  const result = nodes.filter((node) => `${node.name} ${node.modId ?? ""} ${node.file}`.toLowerCase().includes(search.trim().toLowerCase()));
  const current = nodes.find((node) => node.file === selected) ?? result[0];
  const byFile = useMemo(() => new Map(nodes.map((node) => [node.file, node])), [nodes]);
  if (!nodes.length) return <div className="empty"><GitFork /><h3>No installed mods</h3><p>Install a mod to explore its dependency metadata.</p></div>;
  return <div className="pack-graph">
    <section className="panel detail-panel"><label className="search-field"><Search size={16} /><input aria-label="Find a mod" value={search} placeholder="Find a mod" onChange={(event) => { setSearch(event.target.value); setLimit(40); }} /></label>
      <div className="graph-nodes">{result.slice(0, limit).map((item) => <button key={item.file} aria-pressed={item.file === current?.file} onClick={() => select(item.file)}>{item.name}</button>)}</div>
      {!result.length && <p>No mods match your search.</p>}{result.length > limit && <Button size="small" onClick={() => setLimit((value) => value + 40)}>Show more mods</Button>}
    </section>
    {current && <section className="panel detail-panel"><h3>{current.name}</h3><p className="muted">{[current.modId, current.version, current.enabled ? "Enabled" : "Disabled"].filter(Boolean).join(" · ")}</p>
      <h4>Requirements and conflicts</h4>
      {!current.edges.length && <p className="muted">No relationships available for this mod.</p>}
      {current.edges.map((edge, index) => <div className="graph-edge" key={`${edge.requirement}-${index}`}>
        <ArrowRight size={15} /><span>{edge.incompatible ? "Conflicts with" : "Requires"}</span>
        {edge.target ? <Button size="small" onClick={() => { select(edge.target!); setSearch(""); }}>{byFile.get(edge.target)?.name ?? edge.target}</Button> : <span className="status-tag">{edge.requirement} · unresolved</span>}
      </div>)}
      <h4>Used by</h4>
      <div className="actions-wrap">{nodes.filter((node) => node.edges.some((edge) => edge.target === current.file && !edge.incompatible)).map((node) =>
        <Button key={node.file} size="small" onClick={() => { select(node.file); setSearch(""); }}>{node.name}</Button>)}</div>
    </section>}
  </div>;
}
