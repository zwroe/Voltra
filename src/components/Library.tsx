import { Select } from "./ui/select";
import { useEffect, useMemo, useRef, useState } from "react";
import * as Menu from "@radix-ui/react-dropdown-menu";
import { Activity, Archive, ArrowDownToLine, ArrowLeft, Check, ChevronDown, Clock3, FlaskConical, FolderOpen, Globe, HeartPulse, History, Images, Layers3, Play, Plus, RotateCcw, Search, Server, Settings2, Share2, Terminal, Timer, Trash2, Upload, Wrench } from "lucide-react";
import { call, displayDate, errorText, isActiveJob, loaderNames, modSides, platformNames, type Backup, type InstalledMod, type Instance, type ModrinthArtwork, type ModSide, type Running } from "../api";
import { runAction, useLauncher } from "../store";
import { Diagnostics } from "./Activity";
import { ContentFilters, ContentSortAndTypes, emptyFilters, type ContentKind, type ContentSort, type InstalledFilters } from "./ContentFilters";
import { ProjectArt } from "./Discover";
import { ProjectDetailsDialog, type OpenProjectDetails } from "./ProjectDetailsDialog";
import { InstanceIcon } from "./InstanceArtwork";
import { LibraryGrid } from "./LibraryGrid";
import { InstanceShare, ReceiveShareDialog } from "./InstanceNetworking";
import { InstanceServer } from "./ServerDashboard";
import { InstanceFiles } from "./InstanceFiles";
import { ModUpdateDialog } from "./ModHistory";
import { PackHealth } from "./PackHealth";
import { PackHistory } from "./PackHistory";
import { PackPerformance } from "./PackPerformance";
import { Screenshots } from "./Screenshots";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

type Play = (instance: Instance, play?: boolean) => void;
type Confirmation = { title: string; detail: string; label: string; action: () => Promise<void> };

const instanceViews = [
  ["content", "Content", Layers3],
  ["files", "Files", FolderOpen],
  ["worlds", "Worlds", Globe],
  ["server", "Server", Server],
  ["share", "Share", Share2],
] as const;
const instanceTools = [
  ["health", "Health", HeartPulse],
  ["logs", "Diagnostics", Terminal],
  ["performance", "Performance", Activity],
  ["history", "History", History],
  ["screenshots", "Screenshots", Images],
] as const;
type InstanceView = typeof instanceViews[number][0] | typeof instanceTools[number][0];

function InstancePlayStats({ instance, session }: { instance: Instance; session?: Running }) {
  if (session?.test) session = undefined;
  const seconds = session?.playtimeSeconds ?? instance.playtimeSeconds ?? 0;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds / 60) % 60;
  const duration = hours > 0 ? `${hours.toLocaleString()} h${minutes ? ` ${minutes} min` : ""}` : `${minutes} min`;
  const lastPlayedAt = session?.startedAt ?? instance.lastPlayedAt;
  const date = lastPlayedAt == null ? null : new Date(lastPlayedAt * 1000);
  const hasDate = date != null && Number.isFinite(date.getTime());

  return <span className="instance-play-stats muted">
    <span><Timer size={15} aria-hidden="true" />{seconds || session ? `${seconds < 60 ? "<1 min" : duration} played` : "No playtime recorded"}</span>
    <span><Clock3 size={15} aria-hidden="true" />{hasDate
      ? <span>Last played <time dateTime={date.toISOString()}>{displayDate(date.getTime() / 1000)}</time></span>
      : "Never played"}</span>
  </span>;
}

export function Library({ onCreate, onEdit, onPlay }: {
  onCreate: (folder?: string) => void;
  onEdit: (instance: Instance) => void;
  onPlay: Play;
}) {
  const instances = useLauncher((state) => state.instances);
  const id = useLauncher((state) => state.detailId);
  const go = useLauncher((state) => state.go);
  const folder = useLauncher((state) => state.libraryFolder);
  const [receive, setReceive] = useState(false);
  const [tab, setTab] = useState<"instances" | "backups">("instances");
  const selected = instances.find((instance) => instance.id === id);

  if (selected) return <InstanceDetail key={selected.id} instance={selected} onEdit={onEdit} onPlay={onPlay}
    onBack={() => { setTab("instances"); go("Library"); }} />;

  return <>
    <div className="page-heading"><div><h1>Library</h1><p>Your Minecraft instances and backups.</p></div>
      <div className="actions-wrap">
      <Menu.Root modal={false}>
        <Menu.Trigger asChild><Button><ArrowDownToLine /> Add existing <ChevronDown /></Button></Menu.Trigger>
        <Menu.Portal><Menu.Content className="content-menu" align="end" sideOffset={7} aria-label="Add an existing instance">
          <Menu.Item className="content-menu-item" onSelect={() => void runAction(async () => {
            const job = await call<string | null>("import_pack");
            if (job) { go("Downloads"); await useLauncher.getState().poll(); }
          })}><ArrowDownToLine size={16} /> Import modpack</Menu.Item>
          <Menu.Item className="content-menu-item" onSelect={() => setReceive(true)}><Share2 size={16} /> Use share code</Menu.Item>
        </Menu.Content></Menu.Portal>
      </Menu.Root>
      <Button variant="primary" onClick={() => onCreate(folder)}><Plus /> New instance</Button></div></div>
    <div className="instance-tabs library-tabs" aria-label="Library view">
      <button aria-pressed={tab === "instances"} onClick={() => setTab("instances")}><Layers3 size={16} /> Instances</button>
      <button aria-pressed={tab === "backups"} onClick={() => setTab("backups")}><Archive size={16} /> Backups</button>
    </div>
    <div key={tab} className="tab-content">
    {tab === "backups" ? <Backups /> : <LibraryGrid instances={instances} onCreate={onCreate} onPlay={onPlay} />}
    </div>
    {receive && <ReceiveShareDialog onClose={() => setReceive(false)} />}
  </>;
}

function InstanceDetail({ instance, onEdit, onPlay, onBack }: {
  instance: Instance; onEdit: (instance: Instance) => void; onPlay: Play; onBack: () => void;
}) {
  const [tab, setTab] = useState<InstanceView>("content");
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const [testOpen, setTestOpen] = useState(false);
  const [startingTest, setStartingTest] = useState(false);
  const testLaunchAvailable = useLauncher((state) => state.testLaunchAvailable);
  const session = useLauncher((state) => state.running.find((game) => game.instanceId === instance.id));
  const processing = useLauncher((state) => state.jobs.some((job) => job.instanceId === instance.id && isActiveJob(job)));
  const locked = !!session || processing || startingTest;
  const selectedTool = instanceTools.find(([id]) => id === tab);

  async function startTest() {
    setStartingTest(true);
    try {
      await runAction(async () => {
        await call("start_test_game", { id: instance.id });
        setTestOpen(false);
        await useLauncher.getState().poll();
      });
    } finally {
      setStartingTest(false);
    }
  }

  return <>
    <Button variant="ghost" size="small" className="back-link" onClick={onBack}><ArrowLeft /> Library</Button>
    <div className="page-heading detail-heading instance-banner"><div className="instance-heading">
      <InstanceIcon loader={instance.loader} icon={instance.icon} size={27} />
      <div><h1>{instance.name}</h1><p>{loaderNames[instance.loader]} {instance.loaderVersion ?? ""} · Minecraft {instance.minecraftVersion} · {instance.modCount} {instance.modCount === 1 ? "mod" : "mods"}</p>
        <InstancePlayStats instance={instance} session={session} /></div>
    </div>
      <div className="actions-wrap">
        <Button variant="primary" disabled={locked} onClick={() => onPlay(instance)}><Play fill="currentColor" />{session ? (session.test ? "Test running" : "Running") : processing ? "Working…" : "Play"}</Button>
        {testLaunchAvailable && <Button disabled={locked} aria-haspopup="dialog" onClick={() => setTestOpen(true)}><FlaskConical /> Test Play</Button>}
        <Button variant="secondary" size="icon" aria-label="Instance settings" title="Instance settings" disabled={locked} onClick={() => onEdit(instance)}><Settings2 /></Button>
      </div>
    </div>
    <div className="instance-tabs instance-workspace-nav" aria-label="Instance view">
      {instanceViews.map(([value, title, Icon]) => <button key={value} type="button" aria-pressed={tab === value} onClick={() => setTab(value)}>
        <Icon size={16} /> {title}
      </button>)}
      <Menu.Root modal={false}>
        <Menu.Trigger asChild>
          <button type="button" className="instance-tools-trigger" data-current={!!selectedTool}><Wrench size={16} /> {selectedTool?.[1] ?? "Tools"} <ChevronDown size={14} /></button>
        </Menu.Trigger>
        <Menu.Portal><Menu.Content className="content-menu" align="end" sideOffset={7} aria-label="Instance tools">
          <Menu.RadioGroup value={tab} onValueChange={(value) => setTab(value as InstanceView)}>
            {instanceTools.map(([value, title, Icon]) => <Menu.RadioItem key={value} value={value} className="content-menu-item">
              <Icon size={16} /> {title}<Menu.ItemIndicator className="content-menu-marker"><Check size={15} /></Menu.ItemIndicator>
            </Menu.RadioItem>)}
          </Menu.RadioGroup>
        </Menu.Content></Menu.Portal>
      </Menu.Root>
    </div>
    <div key={tab} className="tab-content">
    {tab === "content" ? <InstalledContent instance={instance} locked={locked} confirm={setConfirm} />
      : tab === "files" ? <InstanceFiles id={instance.id} locked={locked} />
      : tab === "history" ? <PackHistory id={instance.id} locked={locked} />
      : tab === "health" ? <PackHealth instance={instance} locked={locked} />
      : tab === "performance" ? <PackPerformance instance={instance} onDiagnostics={() => setTab("logs")} />
      : tab === "worlds" ? <WorldList instance={instance} /> : tab === "logs" ? <Diagnostics instanceId={instance.id} />
      : tab === "screenshots" ? <Screenshots instanceId={instance.id} />
      : tab === "server" ? <InstanceServer instance={instance} /> : <InstanceShare id={instance.id} locked={locked} />}
    </div>
    {confirm && <Confirm confirmation={confirm} onClose={() => setConfirm(null)} />}
    {testOpen && <Dialog open title="Test Play" description="Open Minecraft in single-player demo mode with a temporary copy of this modpack. Test worlds are discarded when Minecraft closes."
      onOpenChange={(open) => { if (!startingTest) setTestOpen(open); }}>
      <div className="dialog-actions">
        <Button variant="ghost" disabled={startingTest} onClick={() => setTestOpen(false)}>Cancel</Button>
        <Button variant="primary" disabled={locked} onClick={() => void startTest()}>{startingTest ? "Starting…" : "Start test"}</Button>
      </div>
    </Dialog>}
  </>;
}

type PackKind = "resourcePacks" | "shaders";
type Pack = { name: string; directory: boolean; addedAt: number | null; iconUrl?: string };
type ContentRow =
  | { kind: "mods"; name: string; mod: InstalledMod; artwork?: ModrinthArtwork }
  | { kind: PackKind; name: string; pack: Pack; artwork?: ModrinthArtwork };

const packNames: Record<PackKind, string> = { resourcePacks: "Resource pack", shaders: "Shader" };
const contentGroups = ["mods", "resourcePacks", "shaders"] as const;
const compareNames = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" }).compare;
const fileName = (row: ContentRow) => row.kind === "mods" ? row.mod.file : row.pack.name;
const rowKey = (row: ContentRow) => row.kind + ":" + fileName(row);
const canSelect = (row: ContentRow) => row.kind === "mods" || !row.pack.directory;

type FilterData = { matches: string[]; environments: Record<string, ModSide>; unchecked: number };
type FilterCheck = { key: string; data?: FilterData; error?: string };

function useFilterData(id: string, key: string, filter: "environment" | "updates" | "openSource", active: boolean) {
  const [check, setCheck] = useState<FilterCheck | null>(null);
  const current = check?.key === key ? check : null;
  useEffect(() => {
    if (!active || current) return;
    let live = true;
    call<FilterData>("mod_filter_data", { id, filter }).then((data) => {
      if (live) setCheck({ key, data });
    }).catch((cause) => {
      if (live) setCheck({ key, error: errorText(cause) });
    });
    return () => { live = false; };
  }, [id, key, filter, active, current]);
  return { data: current?.data, error: active ? current?.error : undefined, pending: active && !current };
}

function compareContent(a: ContentRow, b: ContentRow, sort: ContentSort) {
  if (sort === "az" || sort === "za") return (sort === "az" ? 1 : -1) * compareNames(a.name, b.name);
  const first = a.kind === "mods" ? a.mod.addedAt : a.pack.addedAt;
  const second = b.kind === "mods" ? b.mod.addedAt : b.pack.addedAt;
  if (first == null || second == null) return Number(first == null) - Number(second == null) || compareNames(a.name, b.name);
  return (sort === "newest" ? second - first : first - second) || compareNames(a.name, b.name);
}

function InstalledContent({ instance, locked, confirm }: { instance: Instance; locked: boolean; confirm: (value: Confirmation) => void }) {
  const [kind, setKind] = useState<ContentKind>("all");
  const [filters, setFilters] = useState<InstalledFilters>(emptyFilters);
  const [items, setItems] = useState<ContentRow[] | null>(null);
  const [artwork, setArtwork] = useState<Record<string, ModrinthArtwork>>({});
  const [icons, setIcons] = useState<Record<string, string>>({});
  const checked = useRef({ generation: "", keys: new Set<string>() });
  const [selecting, setSelecting] = useState(false);
  const [selection, setSelection] = useState<Set<string>>(() => new Set());
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);
  const [error, setError] = useState("");
  const [changing, setChanging] = useState(false);
  const mutation = useRef(false);
  const [loadedRequest, setLoadedRequest] = useState("");
  const [exporting, setExporting] = useState(false);
  const [exportPath, setExportPath] = useState("");
  const [sort, setSort] = useState<ContentSort>("az");
  const [refresh, setRefresh] = useState(0);
  const [modRevision, setModRevision] = useState(0);
  const [openProject, setOpenProject] = useState<OpenProjectDetails | null>(null);
  const [update, setUpdate] = useState<{ file: string | null; title?: string } | null>(null);
  const [sideDialog, setSideDialog] = useState<InstalledMod | null>(null);
  const go = useLauncher((state) => state.go);
  const lastJob = useLauncher((state) => state.jobs.find((job) => job.status === "done" && job.instanceId === instance.id)?.id);
  const id = instance.id;
  const listRequest = JSON.stringify([id, refresh, lastJob]);
  const busy = changing || loadedRequest !== listRequest;
  const packKind = kind === "resourcePacks" || kind === "shaders" ? kind : null;
  const hasMods = items?.some((row) => row.kind === "mods");
  const hasManaged = items?.some((row) => row.kind === "mods" && row.mod.managed);
  const filteringMods = !packKind && (filters.environments.length > 0 || filters.states.length > 0 || filters.updates || filters.openSource || filters.external);
  const metadataKey = JSON.stringify([id, refresh, modRevision, lastJob, instance.loader, instance.minecraftVersion, instance.updateChannel]);
  const environments = useFilterData(id, metadataKey, "environment", !packKind && filters.environments.length > 0 && !!hasManaged);
  const updates = useFilterData(id, metadataKey, "updates", !packKind && filters.updates && !!hasManaged);
  const licenses = useFilterData(id, metadataKey, "openSource", !packKind && filters.openSource && !!hasMods);
  const versionSides = environments.data?.environments;
  const checkingFilters = environments.pending || updates.pending || licenses.pending;
  const filterErrors = [environments.error, updates.error, licenses.error].filter(Boolean);
  const unchecked = (filters.updates ? updates.data?.unchecked ?? 0 : 0) + (filters.openSource ? licenses.data?.unchecked ?? 0 : 0);

  useEffect(() => {
    let live = true;
    Promise.all([
      call<InstalledMod[]>("installed_mods", { id }),
      call<Pack[]>("list_content", { id, kind: "resourcePacks" }),
      call<Pack[]>("list_content", { id, kind: "shaders" }),
    ]).then(([mods, resources, shaders]) => {
      if (!live) return;
      const rows: ContentRow[] = [
        ...mods.map((mod): ContentRow => ({ kind: "mods", name: mod.managed?.title ?? mod.file, mod })),
        ...resources.map((pack): ContentRow => ({ kind: "resourcePacks", name: pack.name, pack })),
        ...shaders.map((pack): ContentRow => ({ kind: "shaders", name: pack.name, pack })),
      ];
      setItems(rows);
      setLoadedRequest(listRequest);
      setError("");
    }).catch((cause) => { if (live) setError(errorText(cause)); });
    return () => { live = false; };
  }, [id, listRequest]);

  const filtered = useMemo(() => {
    const term = query.trim().toLocaleLowerCase();
    const outOfDate = new Set(updates.data?.matches);
    const openSource = new Set(licenses.data?.matches);
    return (items ?? []).map((row): ContentRow => {
      const match = artwork[rowKey(row)];
      return match ? { ...row, artwork: match, name: row.kind === "mods" ? row.mod.managed?.title ?? match.title : match.title } : row;
    }).filter((row) => {
      if (kind !== "all" && row.kind !== kind) return false;
      if (row.kind === "mods") {
        const { mod } = row;
        if (filters.environments.length && !filters.environments.includes(mod.sideOverride ?? versionSides?.[mod.file] ?? mod.side ?? "unknown")) return false;
        if (filters.states.length && !filters.states.includes(mod.enabled ? "enabled" : "disabled")) return false;
        if (filters.external && mod.managed) return false;
        if (filters.updates && !outOfDate.has(mod.file)) return false;
        if (filters.openSource && !openSource.has(mod.file)) return false;
        return (row.name + " " + row.mod.file).toLocaleLowerCase().includes(term);
      }
      return !filteringMods && (row.name + " " + row.pack.name).toLocaleLowerCase().includes(term);
    }).sort((a, b) => compareContent(a, b, sort));
  }, [items, artwork, kind, query, filteringMods, filters, sort, versionSides, updates.data, licenses.data]);
  const pageCount = Math.max(1, Math.ceil(filtered.length / 32));
  const visiblePage = Math.min(page, pageCount - 1);
  const visible = useMemo(() => filtered.slice(visiblePage * 32, (visiblePage + 1) * 32), [filtered, visiblePage]);
  const selected = (items ?? []).filter((row) => selection.has(rowKey(row)));
  const selectedMods = selected.flatMap((row) => row.kind === "mods" ? [row.mod] : []);

  useEffect(() => {
    if (loadedRequest !== listRequest) return;
    if (checked.current.generation !== listRequest) {
      checked.current = { generation: listRequest, keys: new Set() };
    }
    const generation = checked.current;
    for (const group of contentGroups) {
      const pending = visible.filter((row) => row.kind === group && !generation.keys.has(rowKey(row)));
      if (!pending.length) continue;
      pending.forEach((row) => generation.keys.add(rowKey(row)));
      const prefix = (file: string) => group + ":" + file;
      function merge<T>(results: Record<string, T>, set: React.Dispatch<React.SetStateAction<Record<string, T>>>, files: string[]) {
        if (checked.current !== generation) return;
        set((previous) => {
          const next = { ...previous };
          for (const file of files) {
            if (results[file]) next[prefix(file)] = results[file];
            else delete next[prefix(file)];
          }
          return next;
        });
      }
      const files = pending.filter((row) => row.kind !== "mods" || !row.mod.managed?.iconUrl).map(fileName);
      if (!files.length) continue;
      void call<Record<string, string>>("installed_content_icons", { id, group, files })
        .then((data) => merge(data, setIcons, files)).catch(() => {});
      const lookup = pending.filter((row) => row.kind === "mods" ? !row.mod.managed?.iconUrl : !row.pack.directory && !row.pack.iconUrl).map(fileName);
      if (lookup.length) {
        void call<Record<string, ModrinthArtwork>>("installed_mod_artwork", { id, group, files: lookup })
          .then((data) => merge(data, setArtwork, lookup)).catch(() => {});
      }
    }
  }, [id, listRequest, loadedRequest, visible]);

  function toggleSelection(row: ContentRow) {
    setSelection((previous) => {
      const next = new Set(previous);
      const key = rowKey(row);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }

  function selectionBox(row: ContentRow) {
    return selecting && canSelect(row) && <input type="checkbox" className="content-select-checkbox"
      aria-label={"Select " + row.name} checked={selection.has(rowKey(row))} onChange={() => toggleSelection(row)} />;
  }

  async function toggleMods(files: string[], enabled: boolean, bulk = false) {
    await mutate(async () => {
      const changes = await call<{ requested: string; file: string; enabled: boolean }[]>("toggle_selected_mods", { id, enabled, files });
      const byFile = new Map(changes.map((change) => [change.requested, change]));
      setItems((previous) => previous?.map((row) => {
        if (row.kind !== "mods") return row;
        const change = byFile.get(row.mod.file);
        if (!change) return row;
        const managed = row.mod.managed && { ...row.mod.managed, file: change.file };
        return { ...row, name: managed?.title ?? change.file,
          mod: { ...row.mod, file: change.file, enabled: change.enabled, managed } };
      }) ?? null);

      function moveArtwork<T>(previous: Record<string, T>) {
        const next = { ...previous };
        for (const { requested, file } of changes) {
          const oldKey = "mods:" + requested;
          const newKey = "mods:" + file;
          if (previous[oldKey] && oldKey !== newKey) {
            next[newKey] = previous[oldKey];
            delete next[oldKey];
          }
        }
        return next;
      }
      setIcons(moveArtwork);
      setArtwork(moveArtwork);
      setSelection((previous) => bulk ? new Set() : new Set([...previous].map((key) => {
        const change = key.startsWith("mods:") && byFile.get(key.slice(5));
        return change ? "mods:" + change.file : key;
      })));
      setModRevision((revision) => revision + 1);
      setError("");
      void useLauncher.getState().refreshInstances().catch(() => {});
      return false;
    });
  }

  function toggleSelectedMods(enabled: boolean) {
    void toggleMods(selectedMods.map((mod) => mod.file), enabled, true);
  }

  function removeSelected() {
    confirm({
      title: "Remove selected content?",
      detail: "Voltra will create a backup before removing " + selected.length + " selected items.",
      label: "Remove selected",
      action: () => mutate(async () => {
        const args = { id, mods: [] as string[], resourcePacks: [] as string[], shaders: [] as string[] };
        selected.forEach((row) => args[row.kind].push(fileName(row)));
        await call("remove_selected_content", args);
        setSelection(new Set());
      }),
    });
  }

  async function mutate(action: () => Promise<unknown>) {
    if (mutation.current || busy || locked) return;
    mutation.current = true;
    setChanging(true);
    try {
      await runAction(async () => {
        let reload = true;
        try {
          reload = await action() !== false;
          if (reload) await useLauncher.getState().refreshInstances();
        } finally {
          if (reload) setRefresh((value) => value + 1);
        }
      });
    } finally {
      mutation.current = false;
      setChanging(false);
    }
  }

  async function exportFolders() {
    setExporting(true);
    try {
      const result = await runAction(() => call<{ path: string; server: number; client: number; unclassified: number; disabled: number } | null>("export_mod_folders", { id }));
      if (result) {
        const summary = result.server + " server, " + result.client + " client, " + result.unclassified + " unclassified" + (result.disabled ? ", " + result.disabled + " disabled skipped" : "");
        setExportPath(result.path);
        useLauncher.getState().notify("Export finished. " + summary + ".", "success");
      }
    } finally {
      setExporting(false);
    }
  }

  return <>
    <div className="section-toolbar instance-content-toolbar">
      <label className="search-field"><Search size={17} /><input placeholder={"Search " + (items?.length ?? 0) + " installed projects"}
        aria-label="Search installed content" value={query} onChange={(event) => { setQuery(event.target.value); setPage(0); }} /></label>
      <div className="actions-wrap">
        {packKind ? <>
          <Button onClick={() => void runAction(() => call("open_folder", { id, folder: packKind === "shaders" ? "shaderPacks" : packKind }))}><FolderOpen /> Open folder</Button>
          <Button disabled={locked || busy} onClick={() => void mutate(() => call<boolean>("add_content", { id, kind: packKind }))}><Upload /> Add ZIP</Button>
        </> : <>
          <Button disabled={locked || busy || instance.loader === "vanilla"} onClick={() => void mutate(() => call<boolean>("add_local_mod", { id }))}><Upload /> Add JAR</Button>
          <Button disabled={locked || busy || exporting || !hasMods}
            title="Copy enabled mods into server and client folders with the instance configs"
            onClick={() => void exportFolders()}><Server /> {exporting ? "Exporting…" : "Export server folders"}</Button>
        </>}
        <Button variant="primary" onClick={() => go("Discover")}><Plus /> Browse content</Button>
      </div>
    </div>
    {exportPath && <p className="field-hint content-export-path">Exported to {exportPath}</p>}
    <div className="section-toolbar instance-content-filters">
      <ContentSortAndTypes kind={kind} sort={sort} onKind={(value) => { setKind(value); setPage(0); }}
        onSort={(value) => { setSort(value); setPage(0); }} />
      <div className="actions-wrap">
        <Button size="small" aria-pressed={selecting} onClick={() => { setSelecting(!selecting); setSelection(new Set()); }}><Check /> {selecting ? "Done selecting" : "Select"}</Button>
        <Button size="small" variant="ghost" onClick={() => setRefresh((value) => value + 1)}><RotateCcw /> Refresh</Button>
        {!packKind && <Button disabled={busy || locked || !hasManaged} onClick={() => setUpdate({ file: null })}><ArrowDownToLine /> Preview updates</Button>}
      </div>
    </div>
    {selecting && <div className="section-toolbar content-selection">
      <span role="status">{selected.length} selected</span>
      <div className="actions-wrap">
        <Button size="small" disabled={!visible.some(canSelect)} onClick={() => setSelection((previous) => new Set([...previous, ...visible.filter(canSelect).map(rowKey)]))}>Select shown</Button>
        <Button size="small" disabled={!selected.length} onClick={() => setSelection(new Set())}>Clear</Button>
        <Button size="small" disabled={busy || locked || !selectedMods.some((mod) => !mod.enabled)} onClick={() => toggleSelectedMods(true)}>Enable mods</Button>
        <Button size="small" disabled={busy || locked || !selectedMods.some((mod) => mod.enabled)} onClick={() => toggleSelectedMods(false)}>Disable mods</Button>
        <Button size="small" variant="danger" disabled={busy || locked || !selected.length} onClick={removeSelected}><Trash2 /> Remove selected</Button>
      </div>
    </div>}
    {!packKind && <ContentFilters value={filters} onChange={(value) => { setFilters(value); setPage(0); }} />}
    {checkingFilters && <p className="field-hint content-filter-status" role="status">Checking catalog metadata.</p>}
    {!packKind && !!filterErrors.length && <p className="inline-error" role="alert">{filterErrors.join(" ")}</p>}
    {!packKind && unchecked > 0 && <p className="field-hint content-filter-status">Some catalog results are unavailable. Refresh to retry.</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {items === null ? !error && <p className="waiting">Loading installed content.</p> : !filtered.length ? !checkingFilters && <div className="empty">
      <Layers3 /><h2>{items.length ? "No matching content" : "No content installed"}</h2>
      <p>{items.length ? "Try another search or filter." : "Browse Discover or add local files to this instance."}</p>
    </div> : <>
      <div className="mod-table">
        <div className="mod-table-head"><span>Project</span><span>Version or file</span><span>Actions</span></div>
        <ul className="rows">{visible.map((row) => {
          if (row.kind !== "mods") {
            const { pack } = row;
            return <li className="data-row mod-table-row" key={rowKey(row)}>
              <div className="content-project-cell">{selectionBox(row)}<div className="mod-project"><ProjectArt
                url={pack.iconUrl ?? row.artwork?.iconUrl} fallbackUrl={icons[rowKey(row)]} /><div className="row-main">
                <strong>{row.name}</strong><p className="muted">{packNames[row.kind]}</p>
              </div></div></div>
              <div className="mod-version">{pack.directory ? "Folder" : "ZIP archive"}</div>
              <div className="mod-actions">{!pack.directory && <Button size="icon" variant="ghost" disabled={locked || busy}
                aria-label={"Remove " + row.name} onClick={() => confirm({
                  title: "Remove this pack?", label: "Remove pack", detail: "Voltra will create a backup before removing " + row.name + ".",
                  action: () => mutate(() => call("remove_content", { id, kind: row.kind, name: pack.name })),
                })}><Trash2 /></Button>}</div>
            </li>;
          }

          const { mod } = row;
          const managed = mod.managed;
          const project = managed?.project ?? row.artwork?.project;
          const platform = managed?.platform ?? (row.artwork ? "modrinth" : undefined);
          const side = mod.sideOverride ?? versionSides?.[mod.file] ?? mod.side ?? "unknown";
          const projectInfo = <><ProjectArt url={managed?.iconUrl || row.artwork?.iconUrl} fallbackUrl={icons[rowKey(row)]} /><span className="row-main"><strong>{row.name}</strong>
            <span className="muted mod-project-meta">{platform ? platformNames[platform] + " · " : ""}{mod.enabled ? "Enabled" : "Disabled"}
              {managed && (managed.loader !== instance.loader || managed.minecraftVersion !== instance.minecraftVersion) && " · Different game configuration"}
            </span></span></>;
          return <li key={rowKey(row)} className="data-row mod-table-row">
            <div className="content-project-cell">{selectionBox(row)}{project && platform ? <button type="button" className="mod-project mod-project-open" aria-haspopup="dialog" onClick={(event) => setOpenProject({
              id: project, title: row.name, platform, trigger: event.currentTarget,
            })}>{projectInfo}</button> : <div className="mod-project">{projectInfo}</div>}</div>
            <div className="mod-version"><strong>{managed?.displayVersion ?? row.artwork?.displayVersion ?? (project ? "Catalog version" : "Local file")}</strong>
              <span title={mod.file}>{mod.file}</span>
              <button type="button" className="status-tag mod-side-button" aria-haspopup="dialog" disabled={locked || busy}
                title={modSides[side] + " · Change mod side"} aria-label={"Change side for " + row.name}
                onClick={() => setSideDialog(mod)}>{modSides[side]}</button>
            </div>
            <div className="mod-actions">
              <label className="toggle-label"><input type="checkbox" className="switch" checked={mod.enabled} disabled={busy || locked} aria-label={"Enable " + row.name}
                onChange={(event) => void toggleMods([mod.file], event.target.checked)} /></label>
              <Button variant="ghost" size="icon" disabled={busy || locked} aria-label={"Update " + row.name} title="Choose an update version"
                onClick={() => setUpdate({ file: mod.file, title: row.name })}><ArrowDownToLine /></Button>
              <Button variant="ghost" size="icon" disabled={busy || locked} aria-label={"Remove " + mod.file} onClick={() => confirm({
                title: "Remove this mod?", detail: "Voltra will create a backup before removing " + row.name + ".",
                label: "Remove mod", action: () => mutate(() => call("remove_mod", { id, file: mod.file })),
              })}><Trash2 /></Button>
            </div>
          </li>;
        })}</ul>
      </div>
      {pageCount > 1 && <div className="pager"><Button disabled={!visiblePage} onClick={() => setPage(visiblePage - 1)}>Previous</Button>
        <span>{visiblePage + 1} / {pageCount}</span>
        <Button disabled={visiblePage + 1 >= pageCount} onClick={() => setPage(visiblePage + 1)}>Next</Button>
      </div>}
    </>}
    {openProject && <ProjectDetailsDialog project={openProject} onClose={() => setOpenProject(null)} />}
    {update && <ModUpdateDialog id={id} file={update.file} title={update.title} locked={locked} onClose={() => setUpdate(null)} />}
    {sideDialog && <ModSideDialog id={id} mod={sideDialog} locked={locked} onClose={() => setSideDialog(null)}
      onSaved={() => setRefresh((value) => value + 1)} />}
  </>;
}

function ModSideDialog({ id, mod, locked, onClose, onSaved }: {
  id: string; mod: InstalledMod; locked: boolean; onClose: () => void; onSaved: () => void;
}) {
  const [side, setSide] = useState<ModSide | "auto">(mod.sideOverride ?? "auto");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function save() {
    setBusy(true);
    setError("");
    try {
      await call("set_mod_side", { id, file: mod.file, side: side === "auto" ? null : side });
      onSaved();
      onClose();
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  }
  return <Dialog open title={mod.managed?.title ?? mod.file} description="Choose where this mod belongs when you export server and client folders."
    onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
    <label>Mod side<Select value={side} disabled={busy} onValueChange={(value) => setSide(value as ModSide | "auto")}>
      <option value="auto">Automatic</option>
      {Object.entries(modSides).map(([value, title]) => <option key={value} value={value}>{title}</option>)}
    </Select></label>
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="dialog-actions">
      <Button variant="ghost" disabled={busy} onClick={onClose}>Cancel</Button>
      <Button variant="primary" disabled={busy || locked} onClick={() => void save()}>{busy ? "Saving…" : "Save side"}</Button>
    </div>
  </Dialog>;
}

function WorldList({ instance }: { instance: Instance }) {
  const [worlds, setWorlds] = useState<string[] | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    call<string[]>("list_worlds", { id: instance.id }).then((items) => { if (live) setWorlds(items); })
      .catch((reason) => { if (live) setError(errorText(reason)); });
    return () => { live = false; };
  }, [instance.id]);
  return <>
    <div className="section-toolbar"><h2>Saved worlds</h2>
      <Button onClick={() => void runAction(() => call("open_folder", { id: instance.id, folder: "worlds" }))}><FolderOpen /> Open saves folder</Button>
    </div>
    {error ? <p className="inline-error" role="alert">{error}</p> : worlds === null ? <p className="waiting">Finding worlds…</p>
      : !worlds.length ? <div className="empty"><Globe /><h2>No worlds yet</h2><p>Start this instance and create a world in Minecraft.</p></div>
      : <ul className="rows">{worlds.map((name) => <li className="data-row" key={name}>
        <span className="row-icon"><Globe /></span><div className="row-main"><strong>{name}</strong></div>
        <Button onClick={() => void runAction(() => call("open_world", { id: instance.id, name }))}><FolderOpen /> Open folder</Button>
      </li>)}</ul>}
  </>;
}

function Backups() {
  const [backups, setBackups] = useState<Backup[] | null>(null);
  const [error, setError] = useState("");
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const instances = useLauncher((state) => state.instances);
  const refresh = async () => { setBackups(await call<Backup[]>("list_backups")); };
  useEffect(() => {
    let live = true;
    call<Backup[]>("list_backups").then((data) => { if (live) setBackups(data); })
      .catch((cause) => { if (live) setError(errorText(cause)); });
    return () => { live = false; };
  }, []);
  if (error) return <p className="inline-error" role="alert">{error}</p>;
  if (!backups) return <p className="waiting">Loading backups…</p>;
  return <>
    {backups.length ? <ul className="rows">{backups.map((backup) => <li key={backup.id} className="data-row">
      <span className="row-icon"><Archive size={20} /></span>
      <div className="row-main"><strong>{backup.name}</strong><p className="muted">{backup.archived ? "Archive" : "Snapshot"} · {backup.minecraftVersion} · {displayDate(backup.createdAt)}</p></div>
      <Button onClick={() => setConfirm({
        title: "Restore this backup?", label: "Restore backup",
        detail: instances.some((instance) => instance.id === backup.instanceId)
          ? "Voltra archives your current instance first, then restores this copy of its worlds, mods, and settings."
          : "Restore the saved instance and its game files to your Library.",
        action: async () => { await runAction(async () => {
          await call("restore_backup", { backupId: backup.id });
          await refresh();
          await useLauncher.getState().refreshInstances();
        }, "Backup restored."); },
      })}><RotateCcw /> Restore</Button>
    </li>)}</ul> : <div className="empty"><Archive /><h2>No backups yet</h2><p>Create one under General in an instance's settings.</p></div>}
    {confirm && <Confirm confirmation={confirm} onClose={() => setConfirm(null)} />}
  </>;
}

function Confirm({ confirmation, onClose }: { confirmation: Confirmation; onClose: () => void }) {
  const [busy, setBusy] = useState(false);
  return <Dialog open title={confirmation.title} description={confirmation.detail} onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
    <div className="dialog-actions">
      <Button variant="ghost" disabled={busy} onClick={onClose}>Cancel</Button>
      <Button variant="primary" disabled={busy} onClick={async () => {
        setBusy(true);
        try { await confirmation.action(); } finally { setBusy(false); onClose(); }
      }}>{busy ? "Working…" : confirmation.label}</Button>
    </div>
  </Dialog>;
}
