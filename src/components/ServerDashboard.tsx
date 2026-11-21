import { useEffect, useRef, useState } from "react";
import { Copy, FolderOpen, Link2, Package, Play, Plus, RefreshCw, Server, Settings2, SlidersHorizontal, Square, Terminal, Trash2, Users } from "lucide-react";
import { call, errorText, loaderNames, type Instance, type Loader, type Performance } from "../api";
import { runAction, useLauncher } from "../store";
import { InstanceIcon } from "./InstanceArtwork";
import { LogShareDialog, type LogShareSource } from "./LogShareDialog";
import { ReadState, usePackQuery } from "./PackTools";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

type ServerStatus = {
  initialized: boolean;
  port: number | null;
  ramMib: number | null;
  minecraftVersion: string | null;
  loader: Loader | null;
  skipped: string[];
  log: string;
  jobId: string | null;
  processStarted: boolean;
  stopping: boolean;
  metrics: Performance | null;
};

const serverTabs = [
  ["console", "Console", Terminal],
  ["addons", "Addons", Package],
  ["properties", "Properties", SlidersHorizontal],
  ["players", "Players", Users],
  ["settings", "Settings", Settings2],
] as const;

export function InstanceServer({ instance }: { instance: Instance }) {
  const id = instance.id;
  const [tab, setTab] = useState<typeof serverTabs[number][0]>("console");
  const [status, setStatus] = useState<ServerStatus | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [statusError, setStatusError] = useState("");
  const [busy, setBusy] = useState(false);
  const [port, setPort] = useState(25565);
  const [ram, setRam] = useState(instance.ramMib);
  const [eula, setEula] = useState(false);
  const job = useLauncher((state) => state.jobs.find((item) => item.id === status?.jobId));
  const initialized = !!status?.initialized;
  const supported = initialized || instance.loader === "vanilla" || instance.loader === "fabric";
  const running = !!status?.jobId;
  const canSend = !!status?.processStarted && !status.stopping;
  const address = `localhost:${status?.port ?? port}`;
  const memory = status?.ramMib ?? ram;
  const refreshStatus = () => setRefresh((value) => value + 1);
  const showMetrics = tab === "console";

  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const next = await call<ServerStatus>("local_server_status", { id, metrics: showMetrics });
        if (live) { setStatus(next); setStatusError(""); }
      } catch (cause) {
        if (live) setStatusError(errorText(cause));
      } finally {
        if (live) timer = setTimeout(poll, 1500);
      }
    }
    void poll();
    return () => { live = false; clearTimeout(timer); };
  }, [id, refresh, showMetrics]);

  async function change() {
    setBusy(true);
    try {
      await runAction(async () => {
        if (running) await call("stop_local_server", { id });
        else {
          await call("start_local_server", { id, port: status?.port ?? port, ram: memory, eula: initialized || eula });
          setTab("console");
        }
        refreshStatus();
        await useLauncher.getState().poll();
      });
    } finally {
      setBusy(false);
    }
  }

  const validSetup = Number.isInteger(port) && port >= 1 && port <= 65535 && Number.isInteger(memory) && memory >= 512 && memory <= 65536;
  const state = status?.stopping ? "Stopping" : canSend ? "Running" : running ? "Starting" : initialized ? "Stopped" : "Setup required";
  return <div className="server-dashboard">
    <header className="panel server-heading">
      <div className="server-heading-identity">
        <InstanceIcon loader={status?.loader ?? instance.loader} icon={instance.icon} size={54} />
        <div><h2>{instance.name} server</h2><p>{loaderNames[status?.loader ?? instance.loader]} · Minecraft {status?.minecraftVersion ?? instance.minecraftVersion}{initialized && ` · Port ${status.port}`}</p>
          {!!status && <span className={`status-tag ${canSend ? "green" : running ? "yellow" : ""}`} role="status"><span className="status-dot" />{state}</span>}
        </div>
      </div>
      <div className="actions-wrap">
        {initialized && <Button size="icon" title="Open server folder" aria-label="Open server folder" onClick={() => void runAction(() => call("open_local_server_folder", { id }))}><FolderOpen /></Button>}
        {running || initialized || eula
          ? <Button variant={running ? "danger" : "primary"} disabled={busy || !!status?.stopping || !status || (!running && (!validSetup || !supported))} onClick={() => void change()}>
            {running ? <Square /> : <Play />}{status?.stopping ? "Stopping…" : running ? "Stop server" : "Start server"}
          </Button>
          : <Button variant="primary" disabled={!status || !supported} onClick={() => setTab("settings")}><Settings2 /> Set up server</Button>}
      </div>
    </header>
    <div className="instance-tabs server-tabs" aria-label="Server view">
      {serverTabs.map(([value, label, Icon]) => <button key={value} type="button" aria-pressed={tab === value} onClick={() => setTab(value)}><Icon size={16} /> {label}</button>)}
    </div>
    {statusError && <p className="inline-error" role="alert">{statusError}</p>}
    <div className="tab-content" key={tab}>
      {!status ? <p className="waiting" role="status">Reading server status…</p>
        : tab === "settings" ? <ServerSettings id={id} initialized={initialized} running={running || busy}
          supported={supported} ram={ram} savedMemory={status.ramMib}
          port={port} eula={eula} setRam={setRam} setPort={setPort} setEula={setEula} onSaved={refreshStatus} />
        : tab === "console" ? <ServerConsole id={id} status={status} address={address} memory={memory} stage={running ? job?.stage : undefined}
          canSend={canSend} onSetup={() => setTab("settings")} supported={supported} />
        : !initialized ? <div className="panel empty"><Server /><h3>Set up your server</h3>
          <p>{supported ? "Start this server once to create its files." : "Local hosting supports Vanilla and Fabric instances."}</p>
          {supported && <Button onClick={() => setTab("settings")}><Settings2 /> Server settings</Button>}</div>
        : tab === "addons" ? <ServerAddons id={id} running={running || busy} loader={status.loader} skipped={status.skipped} />
        : tab === "properties" ? <ServerProperties id={id} running={running || busy} onSaved={refreshStatus} />
        : <ServerPlayers id={id} canSend={canSend} log={status.log} />}
    </div>
  </div>;
}

function ServerConsole({ id, status, address, memory, stage, canSend, onSetup, supported }: {
  id: string; status: ServerStatus; address: string; memory: number; stage?: string; canSend: boolean; onSetup: () => void; supported: boolean;
}) {
  const [command, setCommand] = useState("");
  const [sending, setSending] = useState(false);
  const [share, setShare] = useState<LogShareSource | null>(null);
  const log = useRef<HTMLPreElement>(null);
  const follow = useRef(true);

  useEffect(() => {
    if (follow.current && log.current) log.current.scrollTop = log.current.scrollHeight;
  }, [status.log]);

  async function send() {
    setSending(true);
    await runAction(async () => {
      await call("local_server_command", { id, command });
      setCommand("");
    });
    setSending(false);
  }

  return <div className="server-console-layout">
    <section className="panel server-console">
      <header className="server-panel-heading"><h3>Console</h3><div className="actions-wrap">
        <Button size="small" disabled={!status.log.trim()} onClick={() => setShare({
          name: "Server console", load: () => call("local_server_log", { id, crash: false }),
        })}><Link2 /> Share log</Button>
        {status.initialized && <Button size="small" onClick={() => setShare({
          name: "Latest server crash report", load: () => call("local_server_log", { id, crash: true }),
        })}><Link2 /> Share crash report</Button>}
      </div></header>
      <pre className="local-server-log" tabIndex={0} aria-label="Server output" ref={log}
        onScroll={(event) => {
          const view = event.currentTarget;
          follow.current = view.scrollHeight - view.scrollTop - view.clientHeight < 30;
        }}>{status.log || "Server output will appear here."}</pre>
      <form className="server-command" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <input aria-label="Server command" placeholder="Enter a Minecraft command" value={command} maxLength={512}
          disabled={!canSend || sending} onChange={(event) => setCommand(event.target.value)} />
        <Button type="submit" disabled={!canSend || sending || !command.trim()}><Terminal /> Send</Button>
      </form>
    </section>
    <aside className="panel server-summary">
      <h3>Connection</h3>
      {status.initialized ? <>
        <label>Join from this PC<div className="server-address"><code>{address}</code><Button size="icon" variant="ghost" title="Copy local address" aria-label="Copy local address"
          onClick={() => void runAction(() => navigator.clipboard.writeText(address), "Server address copied.")}><Copy /></Button></div></label>
        <p className="field-hint">Friends use your reachable IP and this port. Add them in Players if the allowlist is enabled.</p>
        <dl><dt>Heap limit</dt><dd>{memory.toLocaleString()} MiB</dd></dl>
      </> : <p className="field-hint">{supported ? "Configure and start the server to get its connection details." : "Local hosting supports Vanilla and Fabric instances."}</p>}
      {status.metrics && <dl aria-label="Server process metrics">
        <dt>CPU</dt><dd>{status.metrics.cpuPercent.toFixed(1)}%</dd>
        <dt>Resident memory</dt><dd>{Math.round(status.metrics.memoryBytes / 1048576).toLocaleString()} MiB</dd>
      </dl>}
      {!status.initialized && supported && <Button size="small" onClick={onSetup}>Server settings</Button>}
      {stage && <p className="field-hint" role="status">{stage}</p>}
    </aside>
    {share && <LogShareDialog source={share} onClose={() => setShare(null)} />}
  </div>;
}

function ServerSettings({ id, initialized, running, supported, ram, savedMemory, port, eula, setRam, setPort, setEula, onSaved }: {
  id: string; initialized: boolean; running: boolean; supported: boolean; ram: number; savedMemory: number | null;
  port: number; eula: boolean; setRam: (value: number) => void; setPort: (value: number) => void;
  setEula: (value: boolean) => void; onSaved: () => void;
}) {
  const [value, setValue] = useState(savedMemory ?? ram);
  const [saving, setSaving] = useState(false);
  const [isSaved, setIsSaved] = useState(true);
  useEffect(() => { setValue(savedMemory ?? ram); setIsSaved(true); }, [savedMemory, ram]);

  async function save() {
    setSaving(true);
    await runAction(async () => {
      await call("save_local_server_memory", { id, ram: value });
      setIsSaved(true);
      onSaved();
    }, "Server memory saved.");
    setSaving(false);
  }

  return <section className="panel detail-panel form-stack server-section">
    <h3>Hosting settings</h3>
    <p className="field-hint">The server has its own world and files. First setup copies server-compatible mods and configs from the instance.</p>
    {!supported ? <p className="inline-error">Local hosting supports Vanilla and Fabric instances.</p>
      : <form className="form-stack" onSubmit={(event) => { event.preventDefault(); if (initialized) void save(); }}>
        <div className="form-grid">
          {!initialized && <label>TCP port<input type="number" min={1} max={65535} required value={port || ""} disabled={running} onChange={(event) => setPort(event.target.valueAsNumber)} /></label>}
          <label>Server memory (MiB)<input type="number" min={512} max={65536} required value={(initialized ? value : ram) || ""} disabled={running || saving}
            onChange={(event) => {
              if (initialized) { setValue(event.target.valueAsNumber); setIsSaved(false); }
              else setRam(event.target.valueAsNumber);
            }} /></label>
        </div>
        {initialized ? <div className="actions-wrap"><Button variant="primary" type="submit" disabled={running || saving || isSaved}>{saving ? "Saving…" : "Save memory"}</Button></div>
          : <div className="actions-wrap">
            <label className="inline-checkbox"><input type="checkbox" checked={eula} disabled={running} onChange={(event) => setEula(event.target.checked)} /> I accept the Minecraft EULA</label>
            <Button size="small" variant="ghost" onClick={() => void runAction(() => call("open_minecraft_eula"))}>Read EULA</Button>
          </div>}
        {running && <p className="field-hint">Stop the server to change its memory.</p>}
      </form>}
  </section>;
}

type PropertyField = { key: string; label: string; options?: string[]; min?: number; max?: number; toggle?: boolean };
const propertyFields: PropertyField[] = [
  { key: "motd", label: "Server list message" },
  { key: "server-port", label: "TCP port", min: 1, max: 65535 },
  { key: "max-players", label: "Maximum players", min: 1, max: 1000 },
  { key: "difficulty", label: "Difficulty", options: ["peaceful", "easy", "normal", "hard"] },
  { key: "gamemode", label: "Game mode", options: ["survival", "creative", "adventure", "spectator"] },
  { key: "view-distance", label: "View distance (chunks)", min: 3, max: 32 },
  { key: "simulation-distance", label: "Simulation distance (chunks)", min: 3, max: 32 },
  { key: "spawn-protection", label: "Spawn protection (blocks)", min: 0, max: 1000 },
  { key: "white-list", label: "Require allowlist", toggle: true },
  { key: "pvp", label: "Allow player versus player", toggle: true },
  { key: "allow-flight", label: "Allow flight", toggle: true },
];

function ServerProperties({ id, running, onSaved }: { id: string; running: boolean; onSaved: () => void }) {
  const query = usePackQuery<Record<string, string>>("local_server_properties", { id }, running);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const blocked = running || saving;
  const properties = query.value;
  const values = { ...properties, ...draft };
  const changes = Object.fromEntries(Object.entries(draft).filter(([key, value]) => properties?.[key] !== value));

  async function save() {
    setSaving(true);
    await runAction(async () => {
      await call("save_local_server_properties", { id, values: changes });
      setDraft({});
      query.refresh();
      onSaved();
    }, "Server properties saved.");
    setSaving(false);
  }

  const change = (key: string, value: string) => setDraft((before) => ({ ...before, [key]: value }));
  return <section className="panel detail-panel form-stack server-section">
    <h3>Server properties</h3>
    <p className="field-hint">{running ? "Stop the server to edit its properties." : "Changes take effect on the next server start."}</p>
    <ReadState query={query} label="Reading server.properties…" />
    {properties && <form className="form-stack" onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <div className="form-grid">
        {propertyFields.map(({ key, label, options, min, max, toggle }) => <label key={key} className={toggle ? "inline-checkbox" : undefined}>
          {toggle ? <><input type="checkbox" checked={values[key] === "true"} disabled={blocked}
            onChange={(event) => change(key, String(event.target.checked))} /> {label}</>
            : <>{label}{options ? <select value={values[key]} disabled={blocked} onChange={(event) => change(key, event.target.value)}>
              {!options.includes(values[key]) && <option value={values[key]}>{values[key]}</option>}
              {options.map((option) => <option key={option} value={option}>{option}</option>)}
            </select> : <input type={min === undefined ? "text" : "number"} required={min !== undefined} min={min} max={max}
              maxLength={min === undefined ? 160 : undefined} value={values[key]} disabled={blocked} onChange={(event) => change(key, event.target.value)} />}</>}
        </label>)}
      </div>
      <div className="actions-wrap">
        <Button variant="primary" type="submit" disabled={blocked || !Object.keys(changes).length}>{saving ? "Saving…" : "Save properties"}</Button>
        <Button size="small" disabled={blocked || !Object.keys(draft).length} onClick={() => setDraft({})}>Reset edits</Button>
      </div>
    </form>}
  </section>;
}

type Addon = { file: string; enabled: boolean };
function ServerAddons({ id, running, loader, skipped }: { id: string; running: boolean; loader: Loader | null; skipped: string[] }) {
  const query = usePackQuery<Addon[]>("local_server_addons", { id }, running);
  const [busy, setBusy] = useState(false);
  const [remove, setRemove] = useState<string | null>(null);

  async function modify(file?: string, action?: "toggle" | "remove") {
    setBusy(true);
    await runAction(async () => {
      if (file && action) {
        await call("change_local_server_addon", { id, file, action });
        setRemove(null);
      } else if (!await call<boolean>("import_local_server_addon", { id })) return;
      query.refresh();
    });
    setBusy(false);
  }

  return <section className="panel detail-panel form-stack server-section">
    <header className="server-panel-heading"><h3>Server addons</h3>
      {loader !== "vanilla" && <Button size="small" disabled={running || busy} onClick={() => void modify()}><Plus /> Add mod JAR</Button>}
    </header>
    <p className="field-hint">{running ? "Stop the server to manage its mods." : loader === "vanilla" ? "Vanilla servers don’t load mod JARs." : "Changes apply to this server’s separate mods folder."}</p>
    <ReadState query={query} label="Reading server addons…" />
    {query.value && (query.value.length ? <ul className="rows">{query.value.map(({ file, enabled }) => <li key={file} className="data-row">
      <Package size={17} /><span className="row-main">{file}</span>
      <span className={`status-tag ${enabled ? "green" : ""}`}>{enabled ? "Enabled" : "Disabled"}</span>
      <Button size="small" disabled={running || busy} onClick={() => void modify(file, "toggle")}>{enabled ? "Disable" : "Enable"}</Button>
      <Button size="icon" variant="ghost" disabled={running || busy} aria-label={`Remove ${file}`} onClick={() => setRemove(file)}><Trash2 /></Button>
    </li>)}</ul> : <p className="muted">No server addons installed.</p>)}
    {!!skipped.length && <details><summary>Unclassified mods skipped during setup ({skipped.length})</summary>
      <p className="field-hint">Check server compatibility before adding these mods manually.</p><ul>{skipped.map((file) => <li key={file}>{file}</li>)}</ul>
    </details>}
    {remove && <Dialog open title="Remove server add-on?" description={`${remove} will move to the server’s removed-addons folder.`}
      onOpenChange={(open) => { if (!open && !busy) setRemove(null); }}>
      <div className="dialog-actions"><Button disabled={busy} onClick={() => setRemove(null)}>Cancel</Button>
        <Button variant="danger" disabled={running || busy} onClick={() => void modify(remove, "remove")}>Remove add-on</Button></div>
    </Dialog>}
  </section>;
}

type Players = { whitelist: string[]; operators: string[] };
function ServerPlayers({ id, canSend, log }: { id: string; canSend: boolean; log: string }) {
  const query = usePackQuery<Players>("local_server_players", { id }, canSend);
  const [name, setName] = useState("");
  const [pending, setPending] = useState(false);
  const [remove, setRemove] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const result = canSend ? [...log.matchAll(/There are (\d+) of a max of (\d+) players online:?([^\r\n]*)/gi)].at(-1) : undefined;
  const validName = (value: string) => /^[a-z0-9_]{1,16}$/i.test(value);

  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);

  async function send(command: string) {
    setPending(true);
    await runAction(async () => {
      await call("local_server_command", { id, command });
      setName("");
      setRemove(null);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(query.refresh, 1200);
    }, "Command sent. Check the console for Minecraft’s response.");
    setPending(false);
  }

  return <section className="panel detail-panel form-stack server-section">
    <header className="server-panel-heading"><h3>Online players</h3>
      <Button size="small" disabled={!canSend || pending} onClick={() => void send("list")}><RefreshCw /> Refresh player list</Button>
    </header>
    <p className="field-hint">{!canSend ? "The console is unavailable." : result ? `Last server response: ${result[1]} / ${result[2]} online${result[3].trim() ? " · " + result[3].trim() : ""}` : "Refresh the player list to ask Minecraft who is online."}</p>
    <h3>Allowlist</h3>
    <form className="server-command" onSubmit={(event) => { event.preventDefault(); if (validName(name.trim())) void send(`whitelist add ${name.trim()}`); }}>
      <input value={name} maxLength={16} aria-label="Minecraft username" placeholder="Minecraft username" disabled={!canSend || pending} onChange={(event) => setName(event.target.value)} />
      <Button type="submit" disabled={!canSend || pending || !validName(name.trim())}><Plus /> Add player</Button>
    </form>
    <p className="field-hint">Minecraft resolves usernames while the server is running.</p>
    <ReadState query={query} label="Reading player lists…" />
    {query.value && <>
      {query.value.whitelist.length ? <ul className="rows">{query.value.whitelist.map((player) => <li key={player} className="data-row">
        <span className="row-main">{player}</span><Button size="small" disabled={!canSend || pending || !validName(player)} onClick={() => setRemove(player)}>Remove</Button>
      </li>)}</ul> : <p className="muted">No players on the allowlist.</p>}
      <h3>Operators</h3>
      <p className="muted">{query.value.operators.length ? query.value.operators.join(", ") : "No operators configured."}</p>
    </>}
    {remove && <Dialog open title="Remove player from allowlist?" description={`Minecraft will remove ${remove} from this server’s allowlist.`}
      onOpenChange={(open) => { if (!open && !pending) setRemove(null); }}>
      <div className="dialog-actions"><Button disabled={pending} onClick={() => setRemove(null)}>Cancel</Button>
        <Button variant="danger" disabled={!canSend || pending} onClick={() => void send(`whitelist remove ${remove}`)}>Remove player</Button></div>
    </Dialog>}
  </section>;
}
