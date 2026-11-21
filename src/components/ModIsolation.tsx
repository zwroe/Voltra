import { useState } from "react";
import { call, errorText, isActiveJob } from "../api";
import { useLauncher } from "../store";
import { ReadState, usePackQuery } from "./PackTools";
import type { HealthReport } from "./PackHealth";
import { Dialog } from "./ui/dialog";
import { Button } from "./ui/button";

interface Trial { suspects: string[]; base: string[]; side: "first" | "second"; round: number; job: string | null }

export function ModIsolation({ id, onClose }: { id: string; onClose: () => void }) {
  const pack = usePackQuery<HealthReport>("inspect_modpack_health", { id });
  const [trial, setTrial] = useState<Trial | null>(null);
  const [done, setDone] = useState<string>("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const canLaunch = useLauncher((state) => !!state.accounts.activeId || state.testLaunchAvailable);
  const active = useLauncher((state) => state.jobs.some((job) => job.instanceId === id && isActiveJob(job)) || state.running.some((item) => item.instanceId === id));
  const status = useLauncher((state) => state.jobs.find((job) => job.id === trial?.job));
  const mods = pack.value?.nodes.filter((node) => node.enabled) ?? [];
  const names = new Map(mods.map((node) => [node.file, node.name]));
  const mid = Math.ceil((trial?.suspects.length ?? 0) / 2);
  const intended = trial?.side === "second" ? trial.suspects.slice(mid) : trial?.suspects.slice(0, mid) ?? [];
  const request = trial ? [...trial.base, ...intended] : [];
  const preview = usePackQuery<string[]>("isolation_preview", { id, files: request }, trial?.round, !!trial && !done);
  const included = new Set(preview.value ?? []);
  const actual = trial?.suspects.filter((file) => included.has(file)) ?? [];

  function start() {
    const suspects = mods.map((item) => item.file).filter((file) => selected.size === 0 || selected.has(file));
    setTrial({ suspects, base: mods.map((item) => item.file).filter((file) => !suspects.includes(file)), side: "first", round: 1, job: null });
  }

  async function launch() {
    if (!trial) return;
    setBusy(true);
    setError("");
    try {
      const job = await call<string>("launch_isolation", { id, files: request });
      setTrial({ ...trial, job });
      await useLauncher.getState().poll();
    } catch (cause) { setError(errorText(cause)); }
    finally { setBusy(false); }
  }

  function record(reproduced: boolean) {
    if (!trial) return;
    if (!reproduced && trial.side === "first" && trial.suspects.length > 1) {
      setTrial({ ...trial, side: "second", job: null });
    } else if (!reproduced) {
      setDone("The issue did not reproduce in the trials. Try a different starting selection or reproduce it with the full pack first.");
    } else if (actual.length === 1) {
      setDone(`${names.get(actual[0]) ?? actual[0]} is the remaining suspect. Other files and configuration in the trial can also contribute.`);
    } else if (actual.length < trial.suspects.length) {
      setTrial({ ...trial, suspects: actual, side: "first", round: trial.round + 1, job: null });
    } else {
      setDone("The required dependencies kept all remaining suspects together. Review their relationships or test a different starting selection.");
    }
  }

  return <Dialog open title="Guided mod isolation" description="Test subsets in a temporary copy. Use a repeatable failure and close Minecraft after each trial."
    className="project-details-dialog" onOpenChange={(open) => { if (!open && !busy && !active) onClose(); }}>
    {done ? <p role="status">{done}</p> : !trial ? <>
      <ReadState query={pack} />
      {pack.value && (mods.length ? <><p>Choose suspects, or start with all {mods.length} enabled mods.</p>
        <input aria-label="Filter suspects" placeholder="Filter suspects" value={filter} onChange={(event) => setFilter(event.target.value)} />
        <div className="pack-review">{mods.filter((node) => node.name.toLowerCase().includes(filter.toLowerCase())).map((node) => <label key={node.file} className="inline-checkbox">
          <input type="checkbox" checked={selected.has(node.file)} onChange={() => setSelected((before) => {
            const next = new Set(before); if (next.has(node.file)) next.delete(node.file); else next.add(node.file); return next;
          })} /> {node.name}
        </label>)}</div>
        <Button disabled={active} onClick={start}>Preview first trial{selected.size ? ` · ${selected.size} suspects` : ""}</Button>
      </> : <p>No enabled mods are available to isolate.</p>)}
    </> : <>
      <h3>Round {trial.round} · {trial.side === "first" ? "First group" : "Complementary group"}</h3>
      <ReadState query={preview} />
      {preview.value && <><p>{actual.length} of {trial.suspects.length} suspects are included. {preview.value.length} total enabled files after including requirements.</p>
        <details><summary>Files in this trial</summary><ul className="pack-review">{preview.value.map((file) => <li key={file}>{names.get(file) ?? file}</li>)}</ul></details></>}
      {status && <p role="status">{status.status === "failed" ? status.error : status.stage}</p>}
      {!canLaunch && <p>Connect your Minecraft account to run an isolation trial.</p>}
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="actions-wrap"><Button disabled={busy || active || !canLaunch || !preview.value} onClick={() => void launch()}>{trial.job ? "Run trial again" : "Run trial"}</Button>
        {trial.job && status && !active && status.status !== "cancelled" && <>
          <Button onClick={() => record(true)}>Problem reproduced</Button><Button onClick={() => record(false)}>Clean trial</Button>
        </>}
      </div>
    </>}
    <div className="dialog-actions"><Button disabled={busy || active} variant="ghost" onClick={onClose}>Close</Button></div>
  </Dialog>;
}
