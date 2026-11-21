import { ArrowRight, Clock3, Layers3, Play, Plus, Telescope, UserRound } from "lucide-react";
import { displayDate, isActiveJob, loaderNames, type Instance, type QuickTarget } from "../api";
import { useLauncher } from "../store";
import { InstanceIcon } from "./InstanceArtwork";
import { QuickPlay } from "./QuickPlay";
import { Button } from "./ui/button";

type PlayInstance = (instance: Instance, play?: boolean, target?: QuickTarget) => void;

export function recentInstances(instances: Instance[], recentIds: string[]): Instance[] {
  const remaining = new Map(instances.map((instance) => [instance.id, instance]));
  const recent: Instance[] = [];
  for (const id of recentIds) {
    const instance = remaining.get(id);
    if (instance) {
      recent.push(instance);
      remaining.delete(id);
    }
  }
  return [...recent, ...remaining.values()];
}

export function Home({ onCreate, onPlay, onSignIn }: {
  onCreate: () => void; onPlay: PlayInstance; onSignIn: () => void;
}) {
  const instances = useLauncher((state) => state.instances);
  const recents = useLauncher((state) => state.recentIds);
  const hasAccount = useLauncher((state) => !!state.accounts.activeId);
  const go = useLauncher((state) => state.go);
  const ordered = recentInstances(instances, recents);
  const featured = ordered.reduce<Instance | undefined>((latest, candidate) =>
    !latest || (candidate.lastPlayedAt ?? 0) > (latest.lastPlayedAt ?? 0) ? candidate : latest, undefined);
  const others = ordered.filter((instance) => instance.id !== featured?.id).slice(0, 4);
  const running = useLauncher((state) => state.running.some((game) => game.instanceId === featured?.id));
  const preparing = useLauncher((state) => state.jobs.some((job) => job.instanceId === featured?.id && isActiveJob(job)));

  return <div className="home">
    <header className="page-heading home-heading">
      <h1>Home</h1>
      <div className="actions-wrap">
        <Button onClick={() => go("Discover")}><Telescope /> Discover</Button>
        {featured && <Button onClick={onCreate}><Plus /> New instance</Button>}
      </div>
    </header>

    <div className="home-top" data-has-instances={!!featured}>
      <section className="home-feature" aria-label={featured ? "Featured instance" : "Create an instance"}>
        <img className="home-feature-art" src="/voltra-rift.png" alt="" decoding="async" />
        <div className="home-feature-copy">
          {featured ? <>
            <div className="home-feature-icon"><InstanceIcon loader={featured.loader} icon={featured.icon} size={29} /></div>
            <h2>{featured.name}</h2>
            <p className="home-feature-meta">{loaderNames[featured.loader]} · Minecraft {featured.minecraftVersion} · {featured.modCount} {featured.modCount === 1 ? "mod" : "mods"}</p>
            {featured.lastPlayedAt != null && <p className="home-feature-activity"><Clock3 size={14} /> Last played {displayDate(featured.lastPlayedAt)}</p>}
            <div className="actions-wrap home-feature-actions">
              <Button variant="primary" onClick={() => onPlay(featured)} disabled={running || preparing}><Play fill="currentColor" />{running ? "Running" : preparing ? "Preparing…" : "Play Minecraft"}</Button>
              <Button onClick={() => useLauncher.getState().view(featured.id)}>Manage instance <ArrowRight /></Button>
            </div>
          </> : <>
            <h2>Start a new instance</h2>
            <p className="home-feature-meta">Choose a Minecraft version and a mod loader, then add the mods you want.</p>
            <Button variant="primary" className="home-feature-actions" onClick={onCreate}><Plus /> New instance</Button>
          </>}
        </div>
      </section>
      {featured && <QuickPlay instances={instances} onPlay={onPlay} />}
    </div>

    {!hasAccount && <div className="home-account">
      <UserRound size={19} /><p>Connect a Microsoft account to play Minecraft.</p>
      <Button size="small" onClick={onSignIn}>Connect account</Button>
    </div>}

    {featured && <section className="home-library" aria-labelledby="home-library-title">
      <div className="section-heading"><h2 id="home-library-title">Your instances</h2>
        <Button variant="ghost" size="small" onClick={() => go("Library")}>Open Library <ArrowRight /></Button>
      </div>
      <div className="home-instance-grid">
        {others.map((instance) => <InstanceCard key={instance.id} instance={instance} onPlay={onPlay} />)}
        {!others.length && <button type="button" className="home-new-instance" onClick={onCreate}><Layers3 size={21} /> Create another instance <Plus size={17} /></button>}
      </div>
    </section>}
  </div>;
}

function InstanceCard({ instance, onPlay }: { instance: Instance; onPlay: PlayInstance }) {
  const busy = useLauncher((state) => state.running.some((game) => game.instanceId === instance.id) ||
    state.jobs.some((job) => job.instanceId === instance.id && isActiveJob(job)));
  return <article className="panel home-instance">
    <button type="button" className="home-instance-link" onClick={() => useLauncher.getState().view(instance.id)} aria-label={`Manage ${instance.name}`}>
      <InstanceIcon loader={instance.loader} icon={instance.icon} />
      <span><strong>{instance.name}</strong><small>{loaderNames[instance.loader]} · {instance.minecraftVersion}</small></span>
    </button>
    <Button size="icon" variant="ghost" disabled={busy} aria-label={`Play ${instance.name}`} title={busy ? "Instance is busy" : `Play ${instance.name}`} onClick={() => onPlay(instance)}>
      <Play fill="currentColor" />
    </Button>
  </article>;
}
