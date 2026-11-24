import { Select } from "./ui/select";
import { memo, useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { Box, Braces, Download, ExternalLink, Filter, Image, Layers3, LoaderCircle, Package, Puzzle, RefreshCw, Search, Sparkles, Telescope } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { call, errorText, isActiveJob, loaderNames, platformNames, type Catalog, type Instance, type Job, type Loader, type Platform, type Project, type ProjectKind, type ProjectSort } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";
import { SearchableSelect } from "./MinecraftVersionBrowser";
import { ProjectDetailsDialog, type OpenProjectDetails } from "./ProjectDetailsDialog";

const number = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 });
const sources: readonly Platform[] = ["modrinth", "curseforge"];
const kinds: Record<ProjectKind, string> = { mod: "Mods", modpack: "Modpacks", resourcepack: "Resource packs", shader: "Shaders", datapack: "Data packs" };
const kindIcons = { mod: Puzzle, modpack: Layers3, resourcepack: Image, shader: Sparkles, datapack: Braces };
const categories: Record<ProjectKind, string[]> = {
  mod: ["adventure", "decoration", "equipment", "magic", "optimization", "technology", "utility", "worldgen"],
  modpack: ["adventure", "challenging", "combat", "lightweight", "magic", "optimization", "quests", "technology"],
  resourcepack: ["16x", "32x", "64x", "128x", "realistic", "simplistic", "themed", "vanilla-like"],
  shader: ["cartoon", "fantasy", "low", "medium", "high", "realistic", "vanilla-like"],
  datapack: ["adventure", "decoration", "equipment", "game-mechanics", "technology", "utility", "worldgen"],
};
type Source = Platform | "all";
const label = (item: string) => item.replaceAll("-", " ").replace(/^\w/, (letter) => letter.toUpperCase());

const installKey = (id: string | null, kind: ProjectKind, platform: Platform, project: string, world: string | null) =>
  JSON.stringify([id, kind, platform, project, world]);
const projectKey = (platform: Platform, id: string) => `${platform}:${id}`;

type ResultPage = { key: string; projects: Project[]; next: boolean; errors: string[] };
const noResults: ResultPage = { key: "", projects: [], next: false, errors: [] };

export function ProjectArt({ url, fallbackUrl }: { url?: string | null; fallbackUrl?: string }) {
  const [failed, setFailed] = useState<string[]>([]);
  const image = [url, fallbackUrl].find((source) =>
    source && /^(https:\/\/|data:image\/png;base64,)/.test(source) && !failed.includes(source));
  return <span className="project-art">{image ? <img src={image} alt="" loading="lazy" decoding="async" referrerPolicy="no-referrer"
    onError={() => setFailed((previous) => [...previous, image])} /> : <Package strokeWidth={1.4} />}</span>;
}

export function Discover({ active, onOpenIntegrations }: { active: boolean; onOpenIntegrations: () => void }) {
  const instances = useLauncher((state) => state.instances);
  const selected = useLauncher((state) => state.selectedId);
  const pick = useLauncher((state) => state.pick);
  const hasKey = useLauncher((state) => state.curseForgeConfigured);
  const target = instances.find((instance) => instance.id === selected);
  const targetId = target?.id ?? null;
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);
  const [kind, setKind] = useState<ProjectKind>("mod");
  const [source, setSource] = useState<Source>("all");
  const [sort, setSort] = useState<ProjectSort>("relevance");
  const [limit, setLimit] = useState(20);
  const [chosenVersion, setGame] = useState("");
  const [chosenLoader, setLoader] = useState<Loader | "">("");
  const [category, setCategory] = useState("");
  const [compatible, setCompatible] = useState(Boolean(target));
  const [hide, setHide] = useState(false);
  const [versions, setVersions] = useState<string[] | null>(null);
  const versionsPending = useRef(false);
  const [world, setWorld] = useState("");
  const [worldList, setWorldList] = useState<{ id: string; items: string[] } | null>(null);
  const [keyStatusFailed, setKeyStatusFailed] = useState(false);
  const [installed, setInstalled] = useState<{ instance: Instance; projects: Set<string> } | null>(null);
  const installedRequest = useRef<{ instance: Instance; task: Promise<Set<string>> } | null>(null);
  const [result, setResult] = useState<ResultPage>(noResults);
  const [revision, setRevision] = useState(0);
  const searchRequest = useRef<{ key: string; task: Promise<ResultPage> } | null>(null);
  const [submitting, setSubmitting] = useState<Set<string>>(() => new Set());
  const [openProject, setOpenProject] = useState<OpenProjectDetails | null>(null);
  const targetMatches = compatible && kind !== "modpack" && target;
  const [versionsOpen, setVersionsOpen] = useState(false);
  const versionButton = useRef<HTMLButtonElement>(null);
  const game = targetMatches ? target.minecraftVersion : chosenVersion;
  const showLoaders = kind === "mod" || kind === "modpack";
  const loader = showLoaders ? targetMatches ? target.loader : chosenLoader : "";
  const targetRunning = useLauncher((state) => state.running.some((entry) => entry.instanceId === targetId));
  const installId = kind === "modpack" ? null : targetId;
  const worlds = worldList?.id === targetId ? worldList.items : [];
  const installWorld = kind === "datapack" && worlds.includes(world) ? world : null;
  const activeInstalls = useLauncher(useShallow((state) => Object.fromEntries(state.jobs.flatMap((job) =>
    job.instanceId === installId && job.catalog?.kind === kind && job.catalog.world === installWorld && isActiveJob(job)
      ? [[installKey(job.instanceId, job.catalog.kind, job.catalog.platform, job.catalog.project, job.catalog.world), job.status] as const]
      : [],
  ))));
  const searchReady = hasKey !== null || keyStatusFailed || source === "modrinth";
  const requestKey = JSON.stringify([source, !!hasKey, kind, game, loader, category, sort, limit, query, page, revision]);
  const loading = !searchReady || result.key !== requestKey;
  const { projects, next, errors } = loading ? noResults : result;

  useEffect(() => {
    if (hasKey !== null) return;
    let live = true;
    call<boolean>("curseforge_status").then((enabled) => {
      if (live && useLauncher.getState().curseForgeConfigured === null) useLauncher.setState({ curseForgeConfigured: enabled });
    }).catch(() => { if (live) setKeyStatusFailed(true); });
    return () => { live = false; };
  }, [hasKey]);

  function loadVersions() {
    if (versions || versionsPending.current) return;
    versionsPending.current = true;
    void call<Catalog>("minecraft_catalog", { refresh: false }).then((data) => {
      setVersions(data.versions.filter((item) => item.type === "release").map((item) => item.id));
    }).catch(() => {}).finally(() => { versionsPending.current = false; });
  }

  useEffect(() => {
    if (!active || !targetId || kind !== "datapack") return;
    let live = true;
    call<string[]>("list_worlds", { id: targetId }).then((items) => {
      if (live) {
        setWorldList({ id: targetId, items });
        setWorld((previous) => items.includes(previous) ? previous : items[0] ?? "");
      }
    }).catch(() => {});
    return () => { live = false; };
  }, [active, targetId, kind]);

  useEffect(() => {
    if (!active || !target || kind !== "mod" || installed?.instance === target) return;
    if (installedRequest.current?.instance !== target) {
      const task = call<[Platform, string][]>("installed_mod_projects", { id: target.id })
        .then((entries) => new Set(entries.map(([platform, id]) => projectKey(platform, id))));
      installedRequest.current = { instance: target, task };
    }
    const request = installedRequest.current;
    let live = true;
    request.task.then((projects) => { if (live) setInstalled({ instance: target, projects }); }).catch(() => {
      if (installedRequest.current === request) installedRequest.current = null;
    });
    return () => { live = false; };
  }, [active, target, kind, installed]);

  useEffect(() => {
    if (!active || !searchReady || result.key === requestKey) return;
    if (searchRequest.current?.key !== requestKey) {
      const platforms = sources.filter((item) => (source === "all" || source === item) && (item !== "curseforge" || hasKey));
      const filters = { kind, game: game || null, loader: loader || null, category: category || null, sort, limit };
      const task = Promise.all(platforms.map(async (platform) => {
        try {
          return { data: await call<Project[]>("search_mods", { platform, query, offset: page * limit, filters }), error: null };
        } catch (cause) {
          return { data: [] as Project[], error: platformNames[platform] + ": " + errorText(cause) };
        }
      })).then((results) => {
        const projects = Array.from({ length: limit }, (_, index) => results.flatMap((entry) => entry.data[index] ?? [])).flat();
        if (sort === "downloads") projects.sort((a, b) => b.downloads - a.downloads);
        if (sort === "updated") projects.sort((a, b) => (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""));
        return { key: requestKey, projects, next: results.some((entry) => entry.data.length === limit), errors: results.flatMap((entry) => entry.error ?? []) };
      });
      searchRequest.current = { key: requestKey, task };
    }
    let live = true;
    void searchRequest.current.task.then((data) => { if (live) setResult(data); });
    return () => { live = false; };
  }, [active, searchReady, result.key, requestKey, hasKey, source, kind, game, loader, category, sort, limit, query, page]);

  function submit(event: FormEvent) {
    event.preventDefault();
    setQuery(draft.trim());
    setPage(0);
  }

  const isInstalled = (project: Project) => kind === "mod" && !!installed && installed.instance === target &&
    installed.projects.has(projectKey(project.platform, project.id));
  const visible = projects.filter((project) => !hide || !isInstalled(project));
  const installTarget = kind === "modpack" || Boolean(target && !targetRunning && (kind !== "mod" || target.loader !== "vanilla") && (kind !== "datapack" || installWorld));
  const resetFilters = () => {
    setCompatible(false); setGame(""); setLoader(""); setCategory(""); setHide(false); setPage(0);
  };

  const install = useCallback((project: Project) => {
    const key = installKey(installId, kind, project.platform, project.id, installWorld);
    setSubmitting((previous) => new Set(previous).add(key));
    void runAction(async () => {
      await call("install_project", {
        id: installId, world: installWorld, platform: project.platform, project: project.id, kind, title: project.title,
      });
      await useLauncher.getState().poll();
    }).finally(() => setSubmitting((previous) => {
      const next = new Set(previous);
      next.delete(key);
      return next;
    }));
  }, [installId, installWorld, kind]);

  return <div className="discover-page">
    <div className="page-heading catalog-heading"><div><h1>Discover</h1><p>Browse Minecraft projects from Modrinth and CurseForge.</p></div></div>
    <div className="catalog-tabs" aria-label="Project type">
      {(Object.keys(kinds) as ProjectKind[]).map((value) => {
        const Icon = kindIcons[value];
        return <button key={value} type="button" aria-pressed={kind === value} onClick={() => {
          setKind(value); setCategory(""); setPage(0);
        }}><Icon size={16} /> {kinds[value]}</button>;
      })}
    </div>
    <div className="catalog-layout">
      <section className="catalog-results">
        <form className="section-toolbar discover-form" onSubmit={submit}>
          <label className="search-field"><Search size={18} /><input aria-label={"Search " + kinds[kind]} value={draft} maxLength={140}
            onChange={(event) => setDraft(event.target.value)} placeholder={"Search " + kinds[kind].toLowerCase()} /></label>
          <Button type="submit" variant="primary">Search</Button>
        </form>
        <div className="catalog-tools">
          <label><span className="sr-only">Sort projects</span><Select value={sort} onValueChange={(value) => { setSort(value as ProjectSort); setPage(0); }}>
            <option value="relevance">Relevance</option><option value="downloads">Most downloaded</option><option value="updated">Recently updated</option><option value="newest">Newest</option>
          </Select></label>
          <label><span className="sr-only">Results per catalog</span><Select value={limit} onValueChange={(value) => { setLimit(Number(value)); setPage(0); }}>
            <option value={10}>10 per catalog</option><option value={20}>20 per catalog</option><option value={40}>40 per catalog</option>
          </Select></label>
          <div className="compact-pager">
            <Button size="icon" aria-label="Refresh results" title="Refresh results" disabled={loading} onClick={() => setRevision((value) => value + 1)}><RefreshCw /></Button>
            <Button size="small" disabled={page === 0 || loading} onClick={() => setPage(page - 1)}>Previous</Button>
            <span>{page + 1}</span>
            <Button size="small" disabled={!next || loading || (page + 1) * limit > 1960} onClick={() => setPage(page + 1)}>Next</Button>
          </div>
        </div>
        {!hasKey && hasKey !== null && source !== "modrinth" && <div className="notice-panel">
          <p>Connect CurseForge to include its catalog.</p><Button size="small" onClick={onOpenIntegrations}>Integrations</Button>
        </div>}
        <div className="catalog-feed" key={kind}>
        {errors.map((error) => <p className="inline-error" key={error} role="alert">{error}</p>)}
        {loading ? <p className="waiting"><LoaderCircle className="spin" size={17} /> Searching catalogs…</p> : !visible.length ? <div className="empty">
          <Telescope /><h2>{errors.length ? "Catalog unavailable" : "No matching projects"}</h2>
          <p>{errors.length ? "Try another catalog or search again." : "Try a different search or clear the filters."}</p>
        </div> : <div className="catalog-cards">{visible.map((project) => {
          const existing = isInstalled(project);
          const key = installKey(installId, kind, project.platform, project.id, installWorld);
          const status = activeInstalls[key];
          const pending = submitting.has(key);
          return <CatalogCard key={projectKey(project.platform, project.id)} project={project}
            existing={existing} status={status} pending={pending} canInstall={installTarget} onOpen={setOpenProject} onInstall={install} />;
        })}</div>}
        </div>
      </section>
      <aside className="catalog-filters" aria-label="Discover filters">
        <header><h2><Filter size={17} /> Filters</h2><button type="button" onClick={resetFilters}>Clear</button></header>
        {kind !== "modpack" && <fieldset className="catalog-filter-group">
          <legend>Install target</legend>
          <label>Install to<Select value={target?.id ?? ""} onValueChange={(value) => {
            pick(value || null); setCompatible(Boolean(value)); setPage(0);
          }}><option value="">Select an instance</option>
            {instances.map((instance) => <option value={instance.id} key={instance.id}>{instance.name}</option>)}
          </Select></label>
          {target && <label className="inline-checkbox"><input type="checkbox" checked={compatible} onChange={(event) => { setCompatible(event.target.checked); setPage(0); }} /> Match this instance</label>}
          {kind === "datapack" && target && <label>World<Select value={world} onValueChange={(value) => setWorld(value)}>
            {!worlds.length && <option value="">No local worlds</option>}{worlds.map((name) => <option key={name} value={name}>{name}</option>)}
          </Select></label>}
          {kind === "mod" && target?.loader === "vanilla" && <p className="field-hint"><Box size={15} /> Choose a modded instance to install mods.</p>}
        </fieldset>}
        <fieldset className="catalog-filter-group">
          <legend>Catalog</legend>
          <label>Provider<Select value={source} onValueChange={(value) => { setSource(value as Source); setPage(0); }}>
            <option value="all">All catalogs</option>{sources.map((item) => <option value={item} key={item}>{platformNames[item]}</option>)}
          </Select></label>
          <SearchableSelect label="Game version" options={[{ id: "", label: "Any version" }, ...(versions ?? []).map((id) => ({ id }))]}
            value={game} open={versionsOpen && !targetMatches} button={versionButton} disabled={Boolean(targetMatches)}
            setOpen={(open) => { setVersionsOpen(open); if (open) loadVersions(); }} placeholder="Find a release" allowCustom
            onSelect={(value) => { setGame(value); setPage(0); }} />
          {showLoaders && <label>Loader<Select value={loader} disabled={Boolean(targetMatches)} onValueChange={(value) => { setLoader(value as Loader | ""); setPage(0); }}>
            <option value="">Any loader</option>{Object.entries(loaderNames).map(([id, title]) => <option key={id} value={id}>{title}</option>)}
          </Select></label>}
          <label>Category<Select value={category} onValueChange={(value) => { setCategory(value); setPage(0); }}>
            <option value="">All categories</option>{categories[kind].map((id) => <option key={id} value={id}>{label(id)}</option>)}
          </Select></label>
          {kind === "mod" && target && <label className="inline-checkbox"><input type="checkbox" checked={hide} onChange={(event) => setHide(event.target.checked)} /> Hide installed mods</label>}
        </fieldset>
      </aside>
    </div>
    {active && openProject && <ProjectDetailsDialog project={openProject} onClose={() => setOpenProject(null)} />}
  </div>;
}

const CatalogCard = memo(function CatalogCard({ project, existing, status, pending, canInstall, onOpen, onInstall }: {
  project: Project; existing: boolean; status?: Job["status"]; pending: boolean; canInstall: boolean;
  onOpen: (project: OpenProjectDetails) => void; onInstall: (project: Project) => void;
}) {
  const installLabel = status === "running" ? "Installing…" : status === "queued" ? "Queued"
    : pending ? "Queueing…" : existing ? "Installed" : "Install";
  return <article className="catalog-card">
    <ProjectArt url={project.iconUrl} />
    <div className="catalog-info">
      <h2><button type="button" className="catalog-project-title" aria-haspopup="dialog"
        onClick={(event) => onOpen({ ...project, trigger: event.currentTarget })}>{project.title}</button></h2>
      <p className="catalog-author">{project.author || platformNames[project.platform]}</p>
      <p className="catalog-description">{project.description}</p>
      <div className="catalog-tags"><span className={"provider-tag " + project.platform}>{platformNames[project.platform]}</span>
        {project.categories.slice(0, 4).map((tag, index) => <span key={tag + index}>{label(tag)}</span>)}</div>
    </div>
    <div className="catalog-card-actions">
      {project.canDownload && <Button size="small" variant="primary" disabled={existing || !canInstall || !!status || pending}
        onClick={() => onInstall(project)}>{status === "running" ? <LoaderCircle className="spin" size={14} /> : <Download size={14} />} {installLabel}</Button>}
      {project.url && <Button size="small" variant={project.canDownload ? "ghost" : "secondary"} onClick={() => void runAction(() => call("open_project_url", { url: project.url }))}><ExternalLink size={14} /> Project</Button>}
      <span className="muted" title={project.downloads.toLocaleString() + " downloads"}>{number.format(project.downloads)} downloads</span>
    </div>
  </article>;
});
