import { Select } from "./ui/select";
import { useMemo, useState } from "react";
import { Activity, BarChart3, Clock3, RotateCcw } from "lucide-react";
import { displayDate, type Instance } from "../api";
import { useLauncher } from "../store";
import { Button } from "./ui/button";
import { ReadState, usePackQuery } from "./PackTools";

interface Session {
  id: string; startedAt: number; durationSeconds: number; preparationSeconds: number;
  outcome: "interrupted" | "closed" | "crashed" | "launchFailed"; exitCode: number | null;
  configuredRamMib: number; sampleCount: number; meanCpuPercent: number; peakMemoryBytes: number;
  meanFps: number | null; meanFrameMs: number | null; stutters: number | null;
  diagnosticSignals: string[];
}
interface Point { elapsedSeconds: number; cpuPercent: number; memoryBytes: number; fps: number | null; frameMs: number | null; stutters: number | null }
interface SessionDetail extends Session { samples: Point[]; worldSeconds: Record<string, number> }

const statuses: Record<Session["outcome"], string> = { interrupted: "Interrupted", closed: "Closed", crashed: "Nonzero exit", launchFailed: "Launch failed" };
const mib = (bytes: number) => Math.round(bytes / 2 ** 20);
const duration = (seconds: number) => seconds >= 3600 ? `${Math.floor(seconds / 3600)}h ${Math.floor(seconds % 3600 / 60)}m` : seconds >= 60 ? `${Math.floor(seconds / 60)}m` : `${seconds}s`;
const dayKey = (date: Date) => `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`;

export function PackPerformance({ instance, onDiagnostics }: { instance: Instance; onDiagnostics: () => void }) {
  const [selected, select] = useState<string | null>(null);
  const [compare, setCompare] = useState<string | null>(null);
  const [day, setDay] = useState<string | null>(null);
  const [visible, setVisible] = useState(20);
  const pid = useLauncher((state) => state.running.find((game) => game.instanceId === instance.id)?.pid);
  const history = usePackQuery<Session[]>("session_history", { id: instance.id }, pid);
  const rows = history.value ?? [];
  const selectedId = selected ?? rows.find((row) => row.sampleCount > 0)?.id ?? rows[0]?.id;
  const detail = usePackQuery<SessionDetail>("session_details", { id: instance.id, session: selectedId ?? null }, undefined, !!selectedId);
  const previous = compare ? rows.find((row) => row.id === compare) : rows.find((row) => row.id !== selectedId && row.sampleCount > 0 && row.outcome === "closed");
  const played = rows.filter((row) => row.outcome !== "launchFailed");
  const launchedHours = played.reduce((total, row) => total + row.durationSeconds, 0);
  const last = detail.value;
  const fpsChange = last?.meanFps != null && previous?.meanFps != null && previous.meanFps > 0
    ? (last.meanFps - previous.meanFps) / previous.meanFps : null;
  const shortlist = day ? rows.filter((row) => dayKey(new Date(row.startedAt * 1000)) === day) : rows;
  const testMemory = played.filter((row) => row.outcome === "closed" && row.sampleCount >= 100).slice(0, 3);
  const compareMemory = testMemory.length === 3 && testMemory.every((row) => row.configuredRamMib >= 4096 && mib(row.peakMemoryBytes) < row.configuredRamMib * 0.55);

  return <section className="pack-tool">
    <div className="section-toolbar"><h2><Activity size={19} /> Performance and activity</h2><div className="actions-wrap">
      <Button size="small" onClick={onDiagnostics}>Investigate in Diagnostics</Button><Button size="small" onClick={history.refresh}><RotateCcw /> Refresh</Button></div></div>
    <ReadState query={history} />
    {history.value && <>
      <div className="pack-stat-grid">
        <Stat label="Total playtime" value={duration(instance.playtimeSeconds ?? 0)} />
        <Stat label="Last played" value={instance.lastPlayedAt ? displayDate(instance.lastPlayedAt) : "Never"} />
        <Stat label="Recorded launches" value={played.length.toLocaleString()} />
        <Stat label="Nonzero exits" value={rows.filter((row) => row.outcome === "crashed").length.toLocaleString()} />
        <Stat label="Longest session" value={duration(Math.max(0, ...played.map((row) => row.durationSeconds)))} />
        <Stat label="Average session" value={duration(played.length ? Math.round(launchedHours / played.length) : 0)} />
      </div>
      <Heatmap rows={played} day={day} onDay={setDay} />
      <section className="panel detail-panel"><h3>Session history</h3>
        {day && <div className="actions-wrap"><span>{day}</span><Button size="small" onClick={() => setDay(null)}>Clear day</Button></div>}
        {!shortlist.length ? <p className="muted">Launch Minecraft from Voltra to record preparation time, session duration, and usage.</p>
          : <><ul className="rows">{shortlist.slice(0, visible).map((row) => <li key={row.id} className="data-row">
            <div className="row-main"><strong>{displayDate(row.startedAt)}</strong><p className="muted">{statuses[row.outcome]} · {duration(row.durationSeconds)} played · {duration(row.preparationSeconds)} preparing</p></div>
            <Button size="small" variant={selectedId === row.id ? "primary" : "secondary"} onClick={() => select(row.id)}>Inspect</Button>
          </li>)}</ul>{shortlist.length > visible && <Button size="small" onClick={() => setVisible((count) => count + 20)}>Show more sessions</Button>}</>}
      </section>
      {selectedId && <ReadState query={detail} />}
      {last && <section className="panel detail-panel"><h3><BarChart3 size={17} /> {displayDate(last.startedAt)}</h3>
        <p className="muted">{statuses[last.outcome]}{last.exitCode != null && ` · exit code ${last.exitCode}`} · {last.sampleCount} process samples</p>
        {!!last.diagnosticSignals?.length && <details><summary>Signals detected in this session’s logs</summary><ul>{last.diagnosticSignals.map((signal) => <li key={signal}>{signal}</li>)}</ul></details>}
        {last.samples.length ? <div className="pack-charts">
          <Trend title="Java CPU" units="%" points={last.samples} value={(point) => point.cpuPercent} />
          <Trend title="Resident memory" units="MiB" points={last.samples} value={(point) => mib(point.memoryBytes)} />
          <Trend title="Frames per second" units="FPS" points={last.samples} value={(point) => point.fps} />
          <Trend title="Frame time" units="ms" points={last.samples} value={(point) => point.frameMs} />
        </div> : <p className="muted">Process measurements begin after Minecraft has been running for a few seconds.</p>}
        {last.stutters != null && <p>Stutters reported by the companion mod: {last.stutters.toLocaleString()}</p>}
        {Object.keys(last.worldSeconds).length > 0 && <details><summary>World time reported by the companion mod</summary>
          <ul>{Object.entries(last.worldSeconds).map(([name, seconds]) => <li key={name}>{name} · {duration(seconds)}</li>)}</ul>
        </details>}
        <label>Compare with<Select value={previous?.id ?? ""} onValueChange={(value) => setCompare(value || null)}>
          <option value="">Select a recorded session</option>
          {rows.filter((row) => row.id !== last.id).map((row) => <option value={row.id} key={row.id}>{displayDate(row.startedAt)}</option>)}
        </Select></label>
        {previous && <div className="table-scroll"><table className="pack-table"><thead><tr><th>Measurement</th><th>Selected</th><th>Comparison</th></tr></thead><tbody>
          <tr><th>Preparing</th><td>{duration(last.preparationSeconds)}</td><td>{duration(previous.preparationSeconds)}</td></tr>
          <tr><th>Average CPU</th><td>{last.sampleCount ? last.meanCpuPercent.toFixed(1) + "%" : "—"}</td><td>{previous.sampleCount ? previous.meanCpuPercent.toFixed(1) + "%" : "—"}</td></tr>
          <tr><th>Peak resident memory</th><td>{last.sampleCount ? mib(last.peakMemoryBytes) + " MiB" : "—"}</td><td>{previous.sampleCount ? mib(previous.peakMemoryBytes) + " MiB" : "—"}</td></tr>
          <tr><th>Configured Java heap limit</th><td>{last.configuredRamMib ? last.configuredRamMib + " MiB" : "—"}</td><td>{previous.configuredRamMib ? previous.configuredRamMib + " MiB" : "—"}</td></tr>
          <tr><th>FPS</th><td>{last.meanFps?.toFixed(1) ?? "—"}</td><td>{previous.meanFps?.toFixed(1) ?? "—"}</td></tr>
          <tr><th>Frame time</th><td>{last.meanFrameMs?.toFixed(1) ?? "—"}</td><td>{previous.meanFrameMs?.toFixed(1) ?? "—"}</td></tr>
          <tr><th>Reported stutters</th><td>{last.stutters ?? "—"}</td><td>{previous.stutters ?? "—"}</td></tr>
        </tbody></table></div>}
        {fpsChange != null && fpsChange <= -0.1 && <p>Observed average FPS was {Math.round(-fpsChange * 100)}% lower than the comparison session. Repeat the same scene before attributing the change to a mod.</p>}
        <p className="field-hint">Compare sessions from the same world and settings. CPU and memory apply to the launched process as a whole.</p>
      </section>}
      <section className="panel detail-panel"><h3>Memory and JVM guidance</h3>
        {compareMemory ? <p>The last three measured sessions used less than 55% of their configured heap limit in peak resident memory. Try a 512 MiB smaller allocation in a copy and compare an equivalent session. Keep the JVM defaults during the comparison.</p>
          : <p className="muted">Use automatic JVM settings as a baseline. Resident memory includes more than the Java heap. Compare the same workload and review any out-of-memory or GC evidence in Diagnostics before changing flags.</p>}
      </section>
    </>}
  </section>;
}

function Stat({ label, value }: { label: string; value: string }) {
  return <div className="panel pack-stat"><span className="muted">{label}</span><strong>{value}</strong></div>;
}

function Heatmap({ rows, day, onDay }: { rows: Session[]; day: string | null; onDay: (day: string | null) => void }) {
  const days = useMemo(() => {
    const counts = new Map<string, number>();
    for (const row of rows) {
      const key = dayKey(new Date(row.startedAt * 1000));
      counts.set(key, (counts.get(key) ?? 0) + row.durationSeconds);
    }
    const today = new Date();
    return Array.from({ length: 112 }, (_, index) => {
      const date = new Date(today.getFullYear(), today.getMonth(), today.getDate() - (111 - index));
      const key = dayKey(date);
      return { key, seconds: counts.get(key) ?? 0, label: date.toLocaleDateString() };
    });
  }, [rows]);
  return <section className="panel detail-panel"><h3><Clock3 size={17} /> Activity over the last 16 weeks</h3><div className="pack-heatmap" aria-label="Daily playtime by session start date">
    {days.map(({ key, seconds, label }) => <button type="button" key={key} aria-label={`${label}, ${duration(seconds)} played`} title={`${label} · ${duration(seconds)} played`}
      aria-pressed={day === key} data-intensity={seconds > 0 ? Math.min(4, Math.ceil(seconds / 1800)) : 0} onClick={() => onDay(day === key ? null : key)} />)}
  </div></section>;
}

function Trend({ title, units, points, value }: { title: string; units: string; points: Point[]; value: (point: Point) => number | null }) {
  const known = points.flatMap((point) => {
    const amount = value(point);
    return amount == null ? [] : [{ at: point.elapsedSeconds, amount }];
  });
  if (!known.length) return <div><strong>{title}</strong><p className="muted">No samples recorded.</p></div>;
  const max = Math.max(1, ...known.map((point) => point.amount));
  const end = Math.max(1, points[points.length - 1].elapsedSeconds);
  return <div><strong>{title}</strong><svg className="pack-trend" viewBox="0 0 300 90" role="img" aria-label={`${title}. Maximum ${max.toFixed(1)} ${units}.`}>
    <path d="M0 88 H300 M0 45 H300" stroke="var(--line)" fill="none" />
    <polyline points={known.map(({ at, amount }) => `${at / end * 298 + 1},${87 - amount / max * 83}`).join(" ")} fill="none" stroke="var(--accent)" strokeWidth="2" />
    {known.length === 1 && <circle cx={known[0].at / end * 298 + 1} cy={87 - known[0].amount / max * 83} r="3" fill="var(--accent)" />}
  </svg><small className="muted">Peak {max.toFixed(1)} {units} · {duration(end)} sampled</small></div>;
}
