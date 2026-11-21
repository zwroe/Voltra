import { useEffect, useState } from "react";
import { Activity, Check, ChevronLeft, CircleAlert, Download, House, Images, Layers3, LoaderCircle, Plus, Settings, Shirt, Telescope, X } from "lucide-react";
import { call, isActiveJob, type Instance, type Page, type QuickTarget } from "./api";
import { runAction, useLauncher } from "./store";
import { Diagnostics, Downloads } from "./components/Activity";
import { Discover } from "./components/Discover";
import { InstanceDialog } from "./components/InstanceDialog";
import { InstanceIcon } from "./components/InstanceArtwork";
import { Library } from "./components/Library";
import { Home, recentInstances } from "./components/Home";
import { WorldRiskDetails, type WorldRisk } from "./components/PackHealth";
import { SettingsPage, type SettingsTab } from "./components/SettingsPage";
import { SignIn } from "./components/SignIn";
import { AccountAvatar, SkinGallery } from "./components/SkinGallery";
import { Screenshots } from "./components/Screenshots";
import { WindowBar } from "./components/WindowBar";
import { Button } from "./components/ui/button";
import { Dialog } from "./components/ui/dialog";

const navigation = [
  { title: "Play", items: [
    { label: "Home", Icon: House },
    { label: "Library", Icon: Layers3 },
    { label: "Discover", Icon: Telescope },
  ] },
  { title: "Personalize", items: [
    { label: "Skins", Icon: Shirt },
    { label: "Screenshots", Icon: Images },
  ] },
  { title: "Tools", items: [
    { label: "Downloads", Icon: Download },
    { label: "Diagnostics", Icon: Activity },
    { label: "Settings", Icon: Settings },
  ] },
] as const;

function activeCount() {
  return useLauncher.getState().jobs.filter(isActiveJob).length;
}

export function App() {
  const page = useLauncher((state) => state.page);
  const theme = useLauncher((state) => state.settings.theme);
  const animations = useLauncher((state) => state.settings.animations !== false);
  const accounts = useLauncher((state) => state.accounts);
  const loading = useLauncher((state) => state.loading);
  const error = useLauncher((state) => state.error);
  const load = useLauncher((state) => state.load);
  const go = useLauncher((state) => state.go);
  const notify = useLauncher((state) => state.notice);
  const [collapsed, setCollapsed] = useState(false);
  const [visitedDiscover, setVisitedDiscover] = useState(false);
  const [dialog, setDialog] = useState<"create" | "signin" | Instance | null>(null);
  const [createFolder, setCreateFolder] = useState("");
  const [settingsPanel, setSettingsPanel] = useState<SettingsTab | null>(null);
  const [worldWarning, setWorldWarning] = useState<{ instance: Instance; target?: QuickTarget; risk: WorldRisk } | null>(null);
  const [checkingWorld, setCheckingWorld] = useState(false);
  const [worldBackedUp, setWorldBackedUp] = useState(false);
  const [savingBackup, setSavingBackup] = useState(false);
  const newInstance = (folder = "") => { setCreateFolder(folder); setDialog("create"); };
  const signIn = () => setDialog("signin");
  const openSettings = (section: SettingsTab) => { setSettingsPanel(section); go("Settings"); };

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    if (page === "Discover") setVisitedDiscover(true);
  }, [page]);
  useEffect(() => {
    document.documentElement.dataset.animations = animations ? "on" : "off";
  }, [animations]);
  useEffect(() => {
    if (loading || error) return;
    let timer: ReturnType<typeof setTimeout>;
    let live = true;
    async function poll() {
      try {
        await useLauncher.getState().poll();
      } catch {
        // Failed IPC can recover when the next poll runs.
      } finally {
        if (live) timer = setTimeout(poll, activeCount() ? 550 : 2000);
      }
    }
    void poll();
    return () => { live = false; clearTimeout(timer); };
  }, [loading, error]);

  useEffect(() => {
    if (!notify) return;
    const timer = setTimeout(() => useLauncher.setState({ notice: null }), notify.tone === "error" ? 10000 : 4500);
    return () => clearTimeout(timer);
  }, [notify]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => { document.documentElement.dataset.theme = theme === "system" ? media.matches ? "light" : "dark" : theme; };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);

  function start(instance: Instance, play = true, target?: QuickTarget) {
    if (play && !accounts.activeId) {
      signIn();
      return;
    }
    if (checkingWorld || worldWarning) return;
    void runAction(async () => {
      setCheckingWorld(true);
      try {
        if (play && (!target || target.kind === "world")) {
          const risk = await call<WorldRisk>("inspect_world_risk", { id: instance.id, world: target?.kind === "world" ? target.id : null });
          if (risk.hasWorlds && (!risk.baselineAvailable || risk.removed.length || risk.changed.length)) {
            setWorldBackedUp(false);
            setWorldWarning({ instance, target, risk });
            return;
          }
        }
        await launch(instance, play, target);
      } finally { setCheckingWorld(false); }
    });
  }

  async function launch(instance: Instance, play: boolean, target?: QuickTarget) {
    await call("start_game", { id: instance.id, play, target: target ?? null });
    setWorldWarning(null);
    go("Downloads");
    await useLauncher.getState().poll();
  }

  const pages: Record<Exclude<Page, "Discover">, React.ReactNode> = {
    Home: <Home onCreate={() => newInstance()} onPlay={start} onSignIn={signIn} />,
    Library: <Library onCreate={newInstance} onEdit={setDialog} onPlay={start} />,
    Skins: <SkinGallery onSignIn={signIn} />,
    Screenshots: <Screenshots />,
    Downloads: <Downloads />,
    Diagnostics: <Diagnostics />,
    Settings: <SettingsPage onSignIn={signIn} focusSection={settingsPanel} onFocused={() => setSettingsPanel(null)} />,
  };

  return <>
    <WindowBar />
    <div className={"shell" + (collapsed ? " collapsed" : "")}>
      <Sidebar collapsed={collapsed} onCollapse={() => setCollapsed((value) => !value)} onSignIn={signIn}
        onManageAccount={() => openSettings("accounts")} onCreate={() => newInstance()} />
      <div className="workspace">
        <main id="main">
          {loading ? <p role="status" className="waiting startup"><LoaderCircle className="spin" /> Loading your launcher…</p> : error
            ? <section className="empty startup"><CircleAlert /><h1>Voltra couldn’t load</h1><p>{error}</p><Button onClick={() => void load()}>Retry</Button></section>
            : <>
              {page !== "Discover" && <div key={page} className="page page-enter">{pages[page]}</div>}
              {(page === "Discover" || visitedDiscover) && <div key="discover" className="page" hidden={page !== "Discover"}>
                <Discover active={page === "Discover"} onOpenIntegrations={() => openSettings("integrations")} />
              </div>}
            </>}
        </main>
      </div>
    </div>
    {dialog === "signin" && <SignIn onClose={() => setDialog(null)} />}
    {worldWarning && <Dialog open title="Review world compatibility" description={worldWarning.instance.name}
      onOpenChange={(open) => { if (!open && !savingBackup) setWorldWarning(null); }}>
      <WorldRiskDetails risk={worldWarning.risk} />
      <div className="dialog-actions"><Button disabled={savingBackup} variant="ghost" onClick={() => setWorldWarning(null)}>Cancel</Button>
        <Button disabled={savingBackup || worldBackedUp} onClick={() => {
          setSavingBackup(true);
          void runAction(async () => {
            await call("backup_instance", { id: worldWarning.instance.id });
            setWorldBackedUp(true);
          }, "World backup saved.").finally(() => setSavingBackup(false));
        }}>{worldBackedUp ? "Backup saved" : savingBackup ? "Backing up…" : "Back up worlds"}</Button>
        <Button disabled={savingBackup} variant="primary" onClick={() => void runAction(() => launch(worldWarning.instance, true, worldWarning.target))}>Continue to Minecraft</Button>
      </div>
    </Dialog>}
    {dialog && dialog !== "signin" && <InstanceDialog instance={dialog === "create" ? undefined : dialog} onClose={() => setDialog(null)}
      onCreated={(id) => { if (createFolder) useLauncher.getState().moveInstance(id, createFolder); }}
      onDefaults={openSettings} />}
    {notify && <div key={notify.time}
      className={"toast " + notify.tone} role={notify.tone === "error" ? "alert" : "status"}>
      {notify.tone === "error" ? <CircleAlert size={19} /> : <Check size={19} />}<span>{notify.message}</span>
      <Button variant="ghost" aria-label="Dismiss notification" size="icon" onClick={() => useLauncher.setState({ notice: null })}><X size={15} /></Button>
    </div>}
  </>;
}

function DownloadBadge() {
  const count = useLauncher((state) => state.jobs.filter(isActiveJob).length);
  return count > 0 ? <span className="download-badge">{count}</span> : null;
}

function Sidebar({ collapsed, onCollapse, onSignIn, onManageAccount, onCreate }: {
  collapsed: boolean; onCollapse: () => void; onSignIn: () => void; onManageAccount: () => void; onCreate: () => void;
}) {
  const page = useLauncher((state) => state.page);
  const go = useLauncher((state) => state.go);
  const accounts = useLauncher((state) => state.accounts);
  const user = accounts.accounts.find((account) => account.id === accounts.activeId);
  const instances = useLauncher((state) => state.instances);
  const recentIds = useLauncher((state) => state.recentIds);
  const view = useLauncher((state) => state.view);
  const id = useLauncher((state) => state.detailId);
  const recents = recentInstances(instances, recentIds).slice(0, 6);

  return <aside className="sidebar">
    <nav className="primary-nav" aria-label="Main navigation">
      {navigation.map(({ title, items }) => <div className="primary-nav-group" key={title} role="group" aria-label={title}>
        <p className="sidebar-label">{title}</p>
        {items.map(({ label, Icon }) => <button key={label} onClick={() => go(label)}
          aria-current={page === label ? "page" : undefined} aria-label={label} title={label} className="nav-button">
          <Icon size={18} strokeWidth={1.8} /><span className="sidebar-label">{label}</span>
          {label === "Downloads" && <DownloadBadge />}
        </button>)}
      </div>)}
    </nav>
    <nav className="recent-nav" aria-label="Instances">
      {recents.length > 0 && <p className="sidebar-label">Recent instances</p>}
      {recents.map((instance) => <button title={instance.name} aria-label={"Open " + instance.name}
        key={instance.id} className="recent-button" aria-current={id === instance.id ? "page" : undefined}
        onClick={() => view(instance.id)}><InstanceIcon loader={instance.loader} icon={instance.icon} size={16} /><span className="sidebar-label">{instance.name}</span></button>)}
      <button className="recent-button sidebar-create" onClick={onCreate} aria-label="Create instance" title="Create instance">
        <Plus size={18} /><span className="sidebar-label">Create instance</span>
      </button>
    </nav>
    <div className="sidebar-footer">
      <button className="account-button" onClick={user ? onManageAccount : onSignIn}
        aria-label={user ? "Manage account " + user.name : "Connect Microsoft account"} title={user ? user.name : "Connect Microsoft account"}>
        <AccountAvatar account={user} />
        <span className="sidebar-label account-copy"><strong>{user?.name ?? "Connect account"}</strong><span>Microsoft</span></span>
      </button>
      <button className="collapse-button" aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"} aria-expanded={!collapsed}
        title={collapsed ? "Expand sidebar" : "Collapse sidebar"} onClick={onCollapse}><ChevronLeft size={17} /><span className="sidebar-label">Collapse</span></button>
    </div>
  </aside>;
}
