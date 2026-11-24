import { Select } from "./ui/select";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { Archive, ArrowDownToLine, Box, Coffee, Copy, ImagePlus, Monitor, RotateCcw, Search, Terminal, Wrench } from "lucide-react";
import { call, errorText, loaderNames, type Catalog, type Instance, type JavaVersion, type Loader, type SyncItem, type UpdateChannel } from "../api";
import { useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { InstanceArtwork } from "./InstanceArtwork";
import { argumentsFrom, environmentFrom, environmentText, HookFields, ResetField, WindowFields } from "./LaunchSettingsFields";
import { MemorySlider } from "./MemorySlider";
import { MinecraftVersionBrowser, SearchableSelect } from "./MinecraftVersionBrowser";

const pages = [
  { id: "general", label: "General", Icon: ImagePlus },
  { id: "installation", label: "Installation", Icon: Wrench },
  { id: "java", label: "Java & memory", Icon: Coffee },
  { id: "window", label: "Game window", Icon: Monitor },
  { id: "hooks", label: "Launch hooks", Icon: Terminal },
  { id: "sync", label: "Syncing", Icon: RotateCcw },
] as const;

type Panel = typeof pages[number]["id"];
export type DefaultsPanel = "java" | "window" | "hooks";

export function InstanceDialog({ instance, onClose, onDefaults, onCreated }: {
  instance?: Instance; onClose: () => void; onDefaults: (panel: DefaultsPanel) => void; onCreated?: (id: string) => void;
}) {
  const defaults = useLauncher((state) => state.settings);
  const notify = useLauncher((state) => state.notify);
  const [panel, setPanel] = useState<Panel>("general");
  const [name, setName] = useState(instance?.name ?? "");
  const [icon, setIcon] = useState(instance?.icon ?? null);
  const [channel, setChannel] = useState<UpdateChannel>(instance?.updateChannel ?? "release");
  const [version, setVersion] = useState(instance?.minecraftVersion ?? "");
  const [loader, setLoader] = useState<Loader>(instance?.loader ?? "vanilla");
  const [loaderVersion, setLoaderVersion] = useState(instance?.loaderVersion ?? "");
  const [loaderResult, setLoaderResult] = useState<{ key: string; options: string[] | null }>();
  const [loaderRetry, setLoaderRetry] = useState(0);
  const gameVersion = version.trim();
  const loaderKey = JSON.stringify([loader, gameVersion, loaderRetry]);
  const currentResult = loaderResult?.key === loaderKey ? loaderResult : undefined;
  const loaderOptions = currentResult?.options;
  const loaderError = currentResult?.options === null;
  const changedInstallation = !instance || gameVersion !== instance.minecraftVersion || loader !== instance.loader ||
    loaderVersion.trim() !== (instance.loaderVersion ?? "");
  const loaderProblem = loader !== "vanilla" && changedInstallation && gameVersion
    ? loaderError ? "Couldn't check loader compatibility. Try again."
      : !loaderOptions ? "Checking loader compatibility…"
      : !loaderOptions.length ? `${loaderNames[loader]} has no published builds for Minecraft ${gameVersion}.`
      : loaderVersion.trim() && !loaderOptions.includes(loaderVersion.trim()) ? "Choose a compatible loader version."
      : ""
    : "";
  const [launch, setLaunch] = useState(instance?.launch ?? { inheritMemory: !instance });
  const [ram, setRam] = useState(instance?.ramMib ?? defaults.ramMib);
  const [jvm, setJvm] = useState(instance?.jvmArgs.join("\n") ?? "");
  const [environment, setEnvironment] = useState<string | null>(() => instance?.launch?.environment == null
    ? null : environmentText(instance.launch.environment));
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [fetching, setFetching] = useState(true);
  const [catalogError, setCatalogError] = useState("");
  const [pickerOpen, setPickerOpen] = useState(false);
  const versionButton = useRef<HTMLButtonElement>(null);
  const [loaderPickerOpen, setLoaderPickerOpen] = useState(false);
  const loaderButton = useRef<HTMLButtonElement>(null);
  const [loaderTypeOpen, setLoaderTypeOpen] = useState(false);
  const loaderTypeButton = useRef<HTMLButtonElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirm, setConfirm] = useState<"archive" | null>(null);

  useEffect(() => {
    let live = true;
    call<Catalog>("minecraft_catalog", { refresh: false }).then((value) => {
      if (live) {
        setCatalog(value);
        setVersion((previous) => previous || value.latest.release);
      }
    }).catch(() => {
      if (live) setCatalogError("Couldn't retrieve Minecraft releases. Enter the version manually.");
    }).finally(() => {
      if (live) setFetching(false);
    });
    return () => { live = false; };
  }, []);

  useEffect(() => {
    if ((instance && panel !== "installation" && !changedInstallation) || currentResult) return;
    let live = true;
    if (loader === "vanilla" || !gameVersion) return;
    const timer = setTimeout(() => {
      call<string[]>("loader_versions", { loader, game: gameVersion }).then((options) => {
        if (live) setLoaderResult({ key: loaderKey, options: [...new Set(options)] });
      }).catch(() => { if (live) setLoaderResult({ key: loaderKey, options: null }); });
    }, 180);
    return () => { live = false; clearTimeout(timer); };
  }, [instance, panel, loader, gameVersion, loaderKey, changedInstallation, currentResult]);

  async function perform(work: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await work();
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setBusy(false);
    }
  }

  function save(event: FormEvent) {
    event.preventDefault();
    if (busy || loaderProblem || !gameVersion || !name.trim()) return;
    void perform(async () => {
      const options = {
        name, icon, updateChannel: channel, minecraftVersion: gameVersion, loader, loaderVersion: loaderVersion.trim() || null, ramMib: ram,
        jvmArgs: argumentsFrom(jvm), launch: { ...launch, environment: environment === null ? null : environmentFrom(environment) },
      };
      const saved = await call<Instance>(instance ? "update_instance" : "create_instance", { id: instance?.id, options });
      const state = useLauncher.getState();
      await state.refreshInstances();
      if (!instance) onCreated?.(saved.id);
      state.view(saved.id);
      notify(instance ? "Instance settings saved." : "Instance created.", "success");
      onClose();
    });
  }

  function changeVersion(next: string) {
    if (next !== version) setLoaderVersion("");
    setVersion(next);
  }

  const fields = <div className="form-stack">
    <div className="form-grid">
      {catalog ? <MinecraftVersionBrowser versions={catalog.versions} value={version} onSelect={changeVersion}
        open={pickerOpen} setOpen={setPickerOpen} button={versionButton} />
        : <label>Minecraft version<input value={version} onChange={(event) => changeVersion(event.target.value)}
          maxLength={96} placeholder={fetching ? "Loading versions." : "e.g. 1.21.1"} required disabled={fetching} /></label>}
      <SearchableSelect label="Mod loader" value={loader} open={loaderTypeOpen} setOpen={setLoaderTypeOpen}
        button={loaderTypeButton} placeholder="" searchable={false}
        options={Object.entries(loaderNames).map(([id, label]) => ({ id, label }))} onSelect={(value) => {
          if (value !== loader) {
            setLoader(value as Loader);
            setLoaderVersion("");
          }
        }} />
    </div>
    {catalogError && <p className="field-hint">{catalogError}</p>}
    {loader !== "vanilla" && <>
      <SearchableSelect label="Loader version" value={loaderVersion} onSelect={setLoaderVersion}
        open={loaderPickerOpen} setOpen={setLoaderPickerOpen} button={loaderButton} placeholder="Find a loader version"
        disabled={!gameVersion || !loaderOptions?.length} options={[
          { id: "", label: !gameVersion ? "Choose a Minecraft version" : "Automatic (recommended)" },
          ...(loaderOptions ?? []).map((id) => ({ id })),
        ]} />
      {loaderProblem && <p className="field-hint" role="status">{loaderProblem}</p>}
      {loaderError && <Button size="small" onClick={() => setLoaderRetry((value) => value + 1)}>Retry loader check</Button>}
    </>}
  </div>;

  const artwork = <InstanceArtwork loader={loader} icon={icon} change={setIcon} busy={busy} setBusy={setBusy} onError={setError} />;
  const nameField = <label>Instance name<input value={name} onChange={(event) => setName(event.target.value)} required maxLength={64} placeholder="Survival" /></label>;

  function openDefaults(target: DefaultsPanel) {
    onClose();
    onDefaults(target);
  }

  return <Dialog open className={instance ? "instance-dialog instance-settings-dialog" : "instance-dialog"}
    title={instance ? instance.name + " settings" : "New instance"}
    description={instance ? "Configure this Minecraft instance." : "Choose a Minecraft release, a loader, and an icon."}
    onOpenChange={(open) => { if (!open && !busy) onClose(); }}
    onEscapeKeyDown={(event) => {
      if (pickerOpen || loaderPickerOpen || loaderTypeOpen) {
        event.preventDefault();
        setPickerOpen(false);
        setLoaderPickerOpen(false);
        setLoaderTypeOpen(false);
        (loaderPickerOpen ? loaderButton : loaderTypeOpen ? loaderTypeButton : versionButton).current?.focus();
      }
    }}>
    <form className={instance ? "instance-settings-form" : "form-stack"} onSubmit={save}>
      {instance ? <div className="instance-settings-layout">
        <nav className="instance-settings-nav" aria-label="Instance settings sections">
          {pages.map(({ id, label, Icon }) => <button type="button" key={id} aria-pressed={panel === id}
            onClick={() => { setPanel(id); setPickerOpen(false); setLoaderPickerOpen(false); setLoaderTypeOpen(false); }}><Icon size={17} /> {label}</button>)}
        </nav>
        <div className="instance-settings-scroll">
          <section className="tab-content instance-settings-section" key={panel}>
            <h2>{pages.find((page) => page.id === panel)?.label}</h2>
            {panel === "general" && <>
              <div className="instance-identity">{artwork}{nameField}</div>
              <div className="instance-setting-card">
                <label>Mod update channel<Select value={channel} onValueChange={(value) => setChannel(value as UpdateChannel)}>
                  <option value="release">Release</option><option value="beta">Beta and release</option><option value="alpha">Alpha, beta, and release</option>
                </Select></label>
                <p className="field-hint">Applies when checking your installed mods for updates.</p>
              </div>
              <div className="instance-setting-card">
                <h3>Instance management</h3>
                <div className="actions-wrap">
                  <Button disabled={busy} onClick={() => void perform(async () => {
                    const copy = await call<Instance>("duplicate_instance", { id: instance.id });
                    await useLauncher.getState().refreshInstances();
                    useLauncher.getState().view(copy.id);
                    notify("Instance duplicated.", "success");
                    onClose();
                  })}><Copy /> Duplicate</Button>
                  <Button disabled={busy} onClick={() => void perform(async () => {
                    await call("backup_instance", { id: instance.id });
                    notify("Backup created.", "success");
                  })}><Archive /> Back up</Button>
                  <Button disabled={busy} onClick={() => void perform(async () => {
                    if (await call<boolean>("export_pack", { id: instance.id })) notify("Modpack exported.", "success");
                  })}><ArrowDownToLine /> Export modpack</Button>
                  <Button variant="danger" disabled={busy} onClick={() => setConfirm("archive")}><Archive /> Archive instance</Button>
                </div>
              </div>
            </>}
            {panel === "installation" && <>
              {fields}
              <p className="field-hint">Voltra creates a backup when you save a change to the Minecraft or loader version.</p>
              <div className="instance-setting-card">
                <h3>Repair installation</h3>
                <p className="field-hint">Verifies the game installation and downloads missing or invalid dependencies.</p>
                <Button disabled={busy || !!changedInstallation} onClick={() => void perform(async () => {
                  await call("start_game", { id: instance.id, play: false, target: null });
                  useLauncher.getState().go("Downloads");
                  onClose();
                  await useLauncher.getState().poll();
                })}><Wrench /> Repair</Button>
                {changedInstallation && <p className="field-hint">Save installation changes before running a repair.</p>}
              </div>
              <details className="advanced"><summary>Installation location</summary><Button size="small" variant="ghost"
                onClick={() => void perform(() => call("open_folder", { id: instance.id, folder: "game" }))}>Open game folder</Button></details>
            </>}
            {panel === "java" && <>
              <DefaultsLink onClick={() => openDefaults("java")} />
              <label className="inline-checkbox"><input type="checkbox" checked={!!launch.inheritMemory}
                onChange={(event) => setLaunch({ ...launch, inheritMemory: event.target.checked })} />
                Use default memory ({defaults.ramMib.toLocaleString()} MiB)</label>
              {!launch.inheritMemory && <div className="instance-ram">
                <MemorySlider value={ram} onChange={setRam} />
                <label>Memory in MiB<input type="number" min={512} max={65536} value={ram || ""}
                  onChange={(event) => setRam(event.target.valueAsNumber || 0)} /></label>
              </div>}
              <JavaOverride version={version} value={launch.javaPath} defaultPath={defaults.javaPath}
                onChange={(javaPath) => setLaunch({ ...launch, javaPath })} />
              <div className="launch-field-block">
                <div className="launch-control"><label htmlFor="instance-jvm-args">Java arguments</label>
                  <ResetField label="Java arguments" disabled={!jvm} onReset={() => setJvm("")} /></div>
                <textarea id="instance-jvm-args" value={jvm} onChange={(event) => setJvm(event.target.value)} rows={3}
                  placeholder={defaults.jvmArgs?.length ? "Use default arguments" : "One argument per line"} spellCheck={false} />
              </div>
              <div className="launch-field-block">
                <div className="launch-control"><label htmlFor="instance-environment">Environment variables</label>
                  <ResetField label="environment variables" disabled={environment === null} onReset={() => setEnvironment(null)} /></div>
                <textarea id="instance-environment" value={environment ?? ""} onChange={(event) => setEnvironment(event.target.value)}
                  rows={3} placeholder={defaults.environment?.length ? "Use default environment" : "NAME=value"} spellCheck={false} />
              </div>
            </>}
            {panel === "window" && <>
              <DefaultsLink onClick={() => openDefaults("window")} />
              <WindowFields value={launch.window ?? {}} defaults={defaults.window ?? {}} onChange={(window) => setLaunch({ ...launch, window })} />
            </>}
            {panel === "hooks" && <>
              <DefaultsLink onClick={() => openDefaults("hooks")} />
              <HookFields value={launch.hooks ?? {}} instance onChange={(hooks) => setLaunch({ ...launch, hooks })} />
            </>}
            {panel === "sync" && <InstanceSync instance={instance} busy={busy} save={(source, items) => perform(async () => {
              const count = await call<number>("sync_instance_files", { id: instance.id, source, items });
              notify("Synced " + count + " " + (count === 1 ? "item" : "items") + ".", "success");
            })} />}
          </section>
        </div>
      </div> : <>
        <div className="instance-identity">{artwork}{nameField}</div>
        {fields}
        <label className="inline-checkbox"><input type="checkbox" checked={!!launch.inheritMemory}
          onChange={(event) => setLaunch({ ...launch, inheritMemory: event.target.checked })} /> Use default memory</label>
        {!launch.inheritMemory && <MemorySlider value={ram} onChange={setRam} />}
        <details className="advanced"><summary>Advanced JVM arguments</summary><label className="advanced-field">One argument per line
          <textarea rows={3} value={jvm} onChange={(event) => setJvm(event.target.value)} placeholder="-XX:+UseG1GC" />
        </label></details>
      </>}
      <div className="instance-settings-footer">
        {error && <p className="inline-error" role="alert">{error}</p>}
        <div className="dialog-actions">
          <Button variant="ghost" onClick={onClose} disabled={busy}>Cancel</Button>
          <Button variant="primary" type="submit" disabled={busy || (!instance && fetching) || !gameVersion || !name.trim() || Boolean(loaderProblem)}>{busy ? "Working." : instance ? "Save changes" : <><Box /> Create instance</>}</Button>
        </div>
      </div>
    </form>
    {confirm === "archive" && instance && <Dialog open title="Archive this instance?" description="Voltra moves the instance to Backups so it can be restored later."
      onOpenChange={(open) => { if (!open && !busy) setConfirm(null); }}>
      <div className="dialog-actions"><Button disabled={busy} onClick={() => setConfirm(null)}>Cancel</Button>
        <Button disabled={busy} variant="danger" onClick={() => {
          setConfirm(null);
          void perform(async () => {
            await call("archive_instance", { id: instance.id });
            await useLauncher.getState().refreshInstances();
            useLauncher.getState().go("Library");
            notify("Instance archived.", "success");
            onClose();
          });
        }}>Archive</Button></div>
    </Dialog>}
  </Dialog>;
}

function DefaultsLink({ onClick }: { onClick: () => void }) {
  return <div className="instance-defaults-link"><p className="field-hint">Unset overrides use your launcher defaults.</p>
    <Button size="small" onClick={onClick}>Manage defaults</Button>
  </div>;
}

function JavaOverride({ version, value, defaultPath, onChange }: {
  version: string; value?: string | null; defaultPath: string; onChange: (value: string | null) => void;
}) {
  const [required, setRequired] = useState<number | null>(null);
  const [detected, setDetected] = useState<JavaVersion[] | null>(null);
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    if (version) call<number>("required_java", { game: version }).then((major) => {
      if (live) setRequired(major);
    }).catch(() => {});
    return () => { live = false; };
  }, [version]);

  async function check(detect: boolean) {
    setBusy(true);
    setMessage("");
    try {
      if (detect) {
        setDetected(await call<JavaVersion[]>("detect_java"));
      } else {
        const result = await call<JavaVersion>("inspect_java", { path: (value ?? defaultPath).trim() });
        setMessage(required && result.major !== required ? "Found Java " + result.major + ". This release requires Java " + required + "." : "Valid Java " + result.major + " installation.");
      }
    } catch (error) {
      setMessage(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  return <div className="instance-setting-card launch-field-block">
    <div className="launch-control"><label htmlFor="instance-java">Java installation</label>
      <ResetField label="Java installation" disabled={value == null} onReset={() => { onChange(null); setMessage(""); }} /></div>
    {required && <p className="field-hint">Minecraft {version} uses Java {required}.</p>}
    <input id="instance-java" value={value ?? ""} spellCheck={false}
      onChange={(event) => { onChange(event.target.value); setMessage(""); }}
      placeholder={value == null ? defaultPath || "Use launcher default" : "Automatic detection"} />
    <div className="actions-wrap">
      <Button size="small" disabled={busy} onClick={() => void check(true)}><Search /> Detect Java</Button>
      <Button size="small" disabled={busy || !(value ?? defaultPath).trim()} onClick={() => void check(false)}>Check path</Button>
      <Button size="small" variant="ghost" onClick={() => { onChange(""); setMessage(""); }}>Use automatic Java</Button>
    </div>
    {message && <p className="field-hint" role="status">{message}</p>}
    {detected && (detected.length ? <ul className="detected-java">{detected.map((java) => <li key={java.path}>
      <span><strong>Java {java.major}</strong><code>{java.path}</code></span>
      <Button size="small" onClick={() => { onChange(java.path); setMessage(""); }}>Use</Button>
    </li>)}</ul> : <p className="field-hint">No local Java installations found.</p>)}
  </div>;
}

const syncItems: { id: SyncItem; label: string }[] = [
  { id: "options", label: "Game options and keybindings" },
  { id: "servers", label: "Multiplayer server list" },
  { id: "resourcePacks", label: "Resource packs" },
  { id: "commandHistory", label: "Command history" },
  { id: "hotbars", label: "Saved creative hotbars" },
];

function InstanceSync({ instance, busy, save }: { instance: Instance; busy: boolean; save: (source: string, items: SyncItem[]) => Promise<void> }) {
  const instances = useLauncher((state) => state.instances);
  const others = instances.filter((item) => item.id !== instance.id);
  const [source, setSource] = useState("");
  const [items, setItems] = useState<SyncItem[]>([]);
  const [confirm, setConfirm] = useState(false);
  return <>
    <p className="muted">Copy selected Minecraft data from another local instance. Voltra backs up this instance first.</p>
    <label>Source instance<Select value={source} onValueChange={(value) => setSource(value)}>
      <option value="">{others.length ? "Choose an instance" : "No other instances"}</option>
      {others.map((item) => <option key={item.id} value={item.id}>{item.name} ({item.minecraftVersion})</option>)}
    </Select></label>
    <div className="instance-setting-card">
      {syncItems.map(({ id, label }) => <label className="inline-checkbox" key={id}>
        <input type="checkbox" checked={items.includes(id)}
          onChange={(event) => setItems(event.target.checked ? [...items, id] : items.filter((item) => item !== id))} />{label}
      </label>)}
    </div>
    <Button disabled={busy || !source || !items.length} onClick={() => setConfirm(true)}><RotateCcw /> Sync selected items</Button>
    {confirm && <Dialog open title="Sync selected files?" description="Files that exist in the source replace the corresponding files in this instance. A backup is created before any changes."
      onOpenChange={(open) => { if (!open) setConfirm(false); }}>
      <div className="dialog-actions"><Button onClick={() => setConfirm(false)}>Cancel</Button>
        <Button variant="primary" onClick={() => { setConfirm(false); void save(source, items); }}>Sync</Button></div>
    </Dialog>}
  </>;
}
