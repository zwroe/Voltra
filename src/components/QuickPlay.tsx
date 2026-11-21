import { useEffect, useState } from "react";
import { Box, LoaderCircle, Play, RadioTower } from "lucide-react";
import { call, displayDate, errorText, isActiveJob, loaderNames, type Instance, type QuickDestination, type QuickTarget } from "../api";
import { useLauncher } from "../store";
import { Button } from "./ui/button";

type Open = (instance: Instance, play?: boolean, target?: QuickTarget) => void;

let destinationsRequest: { instances: Instance[]; time: number; task: Promise<QuickDestination[]> } | null = null;

function readDestinations(instances: Instance[], refresh: boolean) {
  const time = Date.now();
  if (!refresh && destinationsRequest?.instances === instances && time - destinationsRequest.time < 30_000) return destinationsRequest.task;
  const task = call<QuickDestination[]>("quick_destinations").catch((reason: unknown) => {
    if (destinationsRequest?.task === task) destinationsRequest = null;
    throw reason;
  });
  destinationsRequest = { instances, time, task };
  return task;
}

export function QuickPlay({ instances, onPlay }: { instances: Instance[]; onPlay: Open }) {
  const [destinations, setDestinations] = useState<QuickDestination[] | null>(null);
  const [error, setError] = useState("");
  const [attempt, retry] = useState(0);
  const [kind, setKind] = useState<QuickTarget["kind"] | null>(null);
  const running = useLauncher((state) => state.running);
  const jobs = useLauncher((state) => state.jobs);
  const hasGame = running.length > 0;

  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    async function load(refresh = false) {
      try {
        if (refresh && document.visibilityState === "hidden") return;
        const entries = await readDestinations(instances, refresh);
        if (live) {
          setDestinations(entries);
          setError("");
        }
      } catch (reason) {
        if (live) setError(errorText(reason));
      } finally {
        if (live && hasGame) timer = setTimeout(() => void load(true), 15_000);
      }
    }
    void load();
    return () => { live = false; clearTimeout(timer); };
  }, [instances, hasGame, attempt]);

  const byId = new Map(instances.map((instance) => [instance.id, instance]));
  const selected = kind ?? (destinations?.some((item) => item.target.kind === "world") ? "world"
    : destinations?.some((item) => item.target.kind === "server") ? "server" : "world");
  const busy = new Set([
    ...running.map((game) => game.instanceId),
    ...jobs.filter(isActiveJob).map((job) => job.instanceId),
  ]);

  return <section className="panel home-jump" aria-labelledby="jump-in-title">
    <header className="home-jump-header">
      <h2 id="jump-in-title">Jump back in</h2>
      <div className="home-jump-tabs" aria-label="Destination type">
        <button type="button" aria-pressed={selected === "world"} onClick={() => setKind("world")}><Box size={15} /> Worlds</button>
        <button type="button" aria-pressed={selected === "server"} onClick={() => setKind("server")}><RadioTower size={15} /> Servers</button>
      </div>
    </header>
    {error ? <div className="jump-error"><p className="inline-error" role="alert">{error}</p>
      <Button size="small" onClick={() => retry((value) => value + 1)}>Retry</Button></div>
      : !destinations ? <p className="waiting"><LoaderCircle className="spin" /> Finding your worlds and servers.</p>
      : <DestinationList key={selected} kind={selected} items={destinations} instances={byId} busy={busy} onPlay={onPlay} />}
  </section>;
}

function DestinationList({ kind, items, instances, busy, onPlay }: {
  kind: QuickTarget["kind"]; items: QuickDestination[];
  instances: Map<string, Instance>; busy: Set<string | null>; onPlay: Open;
}) {
  const [expanded, expand] = useState(false);
  const isWorld = kind === "world";
  const Icon = isWorld ? Box : RadioTower;
  const filtered = items.filter((item) => item.target.kind === kind && instances.has(item.instanceId));
  return <div className="jump-destinations">
    {filtered.length > 0 ? <>
      <ul className="jump-list">{(expanded ? filtered : filtered.slice(0, 3)).map((item) => {
        const instance = instances.get(item.instanceId)!;
        const context = isWorld ? item.mode && item.mode[0].toUpperCase() + item.mode.slice(1) : item.target.id;
        const detail = [context, loaderNames[instance.loader] + " " + instance.minecraftVersion].filter(Boolean).join(" · ");
        return <li className="jump-row" key={item.instanceId + "/" + item.target.id}>
          <div className="jump-glyph" aria-hidden="true"><Icon size={20} /></div>
          <div className="jump-label"><strong title={item.name}>{item.name}</strong>
            <span title={instance.name}>{instance.name}</span><small title={detail}>{detail}</small></div>
          <div className="jump-action">
            <Button size="small" aria-label={"Play " + item.name + " in " + instance.name} disabled={busy.has(instance.id)}
              onClick={() => onPlay(instance, true, item.target)}><Play size={13} fill="currentColor" /> Play</Button>
            {item.playedAt > 0 && <time className="muted" title={displayDate(item.playedAt)}
              dateTime={new Date(item.playedAt * 1000).toISOString()}>{shortDate(item.playedAt)}</time>}
          </div>
        </li>;
      })}</ul>
      {filtered.length > 3 && <Button variant="ghost" size="small" className="jump-more" onClick={() => expand(!expanded)}>
        {expanded ? "Show less" : "Show all " + filtered.length}</Button>}
    </> : <div className="jump-empty"><Icon size={19} /><p>{isWorld ? "Worlds in your Voltra instances will appear here." : "Join a server from Voltra on Minecraft 1.20 or later to see it here."}</p></div>}
  </div>;
}

function shortDate(seconds: number) {
  const date = new Date(seconds * 1000);
  const today = new Date();
  if (date.toDateString() === today.toDateString()) return "Today";
  today.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return "Yesterday";
  return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(date);
}
