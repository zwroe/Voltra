import { Select } from "./ui/select";
import { useEffect, useState } from "react";
import * as Menu from "@radix-ui/react-dropdown-menu";
import { Activity, Check, ChevronDown, CircleAlert, Download, FileText, Link2, LoaderCircle, RefreshCw, Wrench, X } from "lucide-react";
import { call, errorText, isActiveJob, type Instance, type Job, type Log, type LogKind, type Performance } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { HealthFixDialog } from "./PackHealth";
import { ModIsolation } from "./ModIsolation";
import { LogShareDialog, type LogShareSource } from "./LogShareDialog";
import { ReadState, usePackQuery } from "./PackTools";

const statuses = { queued: "Queued", running: "In progress", done: "Complete", cancelled: "Cancelled", failed: "Failed" } as const;

function size(bytes: number) {
  return bytes >= 1024 ** 2 ? (bytes / 1024 ** 2).toFixed(1) + " MiB" : Math.round(bytes / 1024) + " KiB";
}

export function Downloads() {
  const jobs = useLauncher((state) => state.jobs);
  const instances = useLauncher((state) => state.instances);
  const active = jobs.filter(isActiveJob);
  const finished = jobs.filter((job) => !isActiveJob(job));
  return <>
    <div className="page-heading"><div><h1>Downloads</h1><p>Follow Minecraft, Java, and mod installations in this session.</p></div></div>
    {!jobs.length ? <div className="empty"><Download /><h2>No downloads in this session</h2><p>Prepare an instance or install a mod to see its progress here.</p></div>
      : <div className="download-sections">
        <DownloadGroup title="Queue" jobs={active} instances={instances} />
        <DownloadGroup title="Recent activity" jobs={finished} instances={instances} />
      </div>}
  </>;
}

function DownloadGroup({ title, jobs, instances }: { title: string; jobs: Job[]; instances: Instance[] }) {
  if (!jobs.length) return null;
  return <section className="download-section" aria-label={title}>
      <div className="section-heading"><h2>{title}</h2><span className="muted">{jobs.length}</span></div>
      <ul className="download-list">{jobs.map((job) => {
        const instance = instances.find((item) => item.id === job.instanceId);
        return <li key={job.id} className="panel download-item">
          <span className={"row-icon " + job.status}>
            {job.status === "done" ? <Check /> : job.status === "failed" ? <CircleAlert /> : job.status === "cancelled" ? <X /> : <Download />}
          </span>
          <div className="download-body">
            <strong>{job.title}{instance && " · " + instance.name}</strong>
            <p>{job.status === "failed" ? job.error : job.status === "cancelled" ? "Download cancelled." : job.stage}</p>
            {job.status === "running" && job.total > 0 && <progress aria-label={job.stage} max={job.total} value={job.completed} />}
            {(job.bytes > 0 || (job.status === "running" && job.total > 0)) && <span className="muted">
              {job.status === "running" && job.total > 0 ? job.completed.toLocaleString() + " / " + job.total.toLocaleString() + " files" : ""}
              {job.status === "running" && job.total > 0 && job.bytes > 0 ? " · " : ""}{job.bytes > 0 ? size(job.bytes) + " transferred" : ""}
            </span>}
          </div>
          <div className="download-actions"><span className={"status-tag " + (job.status === "done" ? "green" : job.status === "failed" ? "red" : "")}>{statuses[job.status]}</span>
            {isActiveJob(job) && <Button size="small" disabled={job.stage === "Cancelling…"} onClick={() => void runAction(async () => {
              await call("cancel_job", { id: job.id });
              await useLauncher.getState().poll();
            })}>Cancel</Button>}</div>
        </li>;
      })}</ul>
  </section>;
}

const logNames: Record<LogKind, string> = { console: "Launcher console", game: "Game log", crash: "Crash report", test: "Test console" };

export function Diagnostics({ instanceId }: { instanceId?: string } = {}) {
  const instances = useLauncher((state) => state.instances);
  const selected = useLauncher((state) => state.selectedId);
  const pick = useLauncher((state) => state.pick);
  const running = useLauncher((state) => state.running);
  const [kind, setKind] = useState<LogKind>("console");
  const [log, setLog] = useState<Log | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [refresh, setRefresh] = useState(0);
  const [performance, setPerformance] = useState<Performance | null>(null);
  const [dialog, setDialog] = useState<"report" | "fixes" | "isolation" | null>(null);
  const [share, setShare] = useState<LogShareSource | null>(null);
  const id = instanceId ?? instances.find((item) => item.id === selected)?.id ?? instances[0]?.id;
  const game = running.find((current) => current.instanceId === id);
  const locked = useLauncher((state) => state.jobs.some((job) => job.instanceId === id && isActiveJob(job))) || !!game;

  useEffect(() => {
    setPerformance(null);
    if (!game || !id) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    async function sample() {
      try {
        const value = await call<Performance | null>("game_performance", { id });
        if (live) setPerformance(value);
      } catch {
        if (live) setPerformance(null);
      } finally {
        if (live) timer = setTimeout(sample, 2000);
      }
    }
    void sample();
    return () => { live = false; clearTimeout(timer); };
  }, [id, game?.pid]);

  useEffect(() => {
    if (!id) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    setLoading(true);
    setLog(null);
    setError("");
    async function fetchLog() {
      try {
        const data = await call<Log | null>("read_diagnostics", { id, kind });
        if (live) { setLog(data); setError(""); }
      } catch (cause) {
        if (live) setError(errorText(cause));
      } finally {
        if (live) {
          setLoading(false);
          if (game && kind !== "crash") timer = setTimeout(fetchLog, 2500);
        }
      }
    }
    void fetchLog();
    return () => { live = false; clearTimeout(timer); };
  }, [id, kind, refresh, game?.pid]);

  return <>
    {!instanceId && <div className="page-heading"><div><h1>Diagnostics</h1><p>Inspect game output, crash reports, and repeated errors.</p></div></div>}
    {!id ? <div className="empty"><Activity /><h2>No instances to inspect</h2><p>Create and run an instance to collect its logs.</p></div> : <>
      <div className="section-toolbar">
        {!instanceId && <label className="target-select">
          <span className="sr-only">Instance to inspect</span>
          <Select value={id} onValueChange={(value) => pick(value)}>
            {instances.map((item) => <option value={item.id} key={item.id}>{item.name}</option>)}
          </Select>
        </label>}
        <div className="actions-wrap">{game && <span className="status-tag green"><span className="status-dot" /> Running · PID {game.pid}</span>}
          <Button onClick={() => setRefresh((value) => value + 1)}><RefreshCw /> Refresh log</Button>
          <Menu.Root modal={false}>
            <Menu.Trigger asChild><Button><Wrench /> Troubleshoot <ChevronDown /></Button></Menu.Trigger>
            <Menu.Portal><Menu.Content className="content-menu" align="end" sideOffset={7} aria-label="Diagnostic actions">
              <Menu.Item className="content-menu-item" disabled={locked} onSelect={() => setDialog("fixes")}>Review fixes</Menu.Item>
              <Menu.Item className="content-menu-item" disabled={locked} onSelect={() => setDialog("isolation")}>Isolate mods</Menu.Item>
              <Menu.Item className="content-menu-item" onSelect={() => setDialog("report")}>Share diagnostic summary</Menu.Item>
            </Menu.Content></Menu.Portal>
          </Menu.Root>
        </div>
      </div>
      {game && performance && <p className="performance-line">Minecraft Java process · {performance.cpuPercent.toFixed(1)}% CPU · {size(performance.memoryBytes)} resident memory</p>}
      <div className="tabs" aria-label="Log type">
        {Object.entries(logNames).map(([key, title]) => <button key={key} aria-pressed={kind === key} onClick={() => setKind(key as LogKind)}>{title}</button>)}
      </div>
      <div className="tab-content" key={kind}>
      {error ? <p className="inline-error" role="alert">{error}</p> : loading ? <p className="waiting"><LoaderCircle className="spin" /> Reading log…</p>
        : !log ? <div className="empty"><FileText /><h2>{kind === "crash" ? "No crash report found" : "No log found"}</h2>
          <p>{kind === "crash" ? "Minecraft writes a report when a crash can be captured." : "Launch this instance to generate a log."}</p></div>
        : <>
          {log.findings.length > 0 && <section className="findings"><h2>Findings</h2>
            {log.findings.map((finding, index) => <article className="panel finding" key={index}>
              <span className={"status-tag " + (finding.severity === "error" ? "red" : finding.severity === "warning" ? "yellow" : "")}>{finding.certainty === "confirmed" ? "Observed" : "Possible"} · {finding.confidence}</span>
              <div><strong>{finding.title}</strong><p>{finding.evidence}</p>
                {finding.suspectedMod && <p className="muted">Mod named in this failure: {finding.suspectedMod}</p>}
                <p className="muted">{finding.recommendation}</p></div>
            </article>)}
          </section>}
          <section className="panel log-panel">
            <header><strong>{log.name}</strong>{log.truncated && <span className="muted">Last 512 KiB</span>}
              <div className="actions-wrap log-actions">
                <Button size="small" onClick={() => void runAction(() => navigator.clipboard.writeText(log.text), "Log copied.")}>Copy log</Button>
                <Button size="small" disabled={!log.text.trim()} onClick={() => setShare({ name: log.name, load: async () => [log.text, log.truncated] })}><Link2 /> Share on mclo.gs</Button>
              </div>
            </header>
            <pre tabIndex={0} aria-label={logNames[kind]}>{log.text || "This log is empty."}</pre>
          </section>
        </>}
      </div>
      {dialog === "report" && <ReportPreview id={id} onClose={() => setDialog(null)} />}
      {share && <LogShareDialog source={share} onClose={() => setShare(null)} />}
      {dialog === "fixes" && <HealthFixDialog id={id} locked={locked} onClose={() => setDialog(null)} onApplied={() => setRefresh((value) => value + 1)} />}
      {dialog === "isolation" && <ModIsolation id={id} onClose={() => setDialog(null)} />}
    </>}
  </>;
}

function ReportPreview({ id, onClose }: { id: string; onClose: () => void }) {
  const report = usePackQuery<string>("diagnostic_share_summary", { id });
  return <Dialog open title="Review the diagnostic summary" description="This summary uses Voltra’s findings and recommended steps. Copy it when you’re ready to share it."
    className="project-details-dialog" onOpenChange={(open) => { if (!open) onClose(); }}>
    <ReadState query={report} />
    {report.value && <textarea rows={14} value={report.value} readOnly aria-label="Diagnostic summary" />}
    <div className="dialog-actions"><Button variant="ghost" onClick={onClose}>Close</Button>
      <Button variant="primary" disabled={!report.value} onClick={() => void runAction(() => navigator.clipboard.writeText(report.value ?? ""), "Summary copied.")}>Copy summary</Button></div>
  </Dialog>;
}
