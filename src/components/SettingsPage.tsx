import { useEffect, useState, type FormEvent } from "react";
import { Check, Coffee, FolderOpen, HardDrive, KeyRound, LoaderCircle, Monitor, Radio, Search, SlidersHorizontal, Terminal, UserRound, X } from "lucide-react";
import { call, errorText, type Accounts, type JavaVersion, type Settings, type Theme } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { MemorySlider } from "./MemorySlider";
import { JavaRuntimes } from "./JavaRuntimes";
import { AccountAvatar } from "./SkinGallery";
import { argumentsFrom, environmentFrom, environmentText, HookFields, WindowFields } from "./LaunchSettingsFields";

const discordMessages = {
  off: "Starting Rich Presence…",
  waitingForDiscord: "Waiting for Discord. Open the Discord desktop app to share your activity.",
  published: "Activity sent to Discord.",
  updateFailed: "Discord didn’t accept the activity yet. Voltra will retry.",
};
type DiscordStatus = keyof typeof discordMessages;

const settingTabs = [
  ["general", "Appearance", SlidersHorizontal],
  ["accounts", "Minecraft accounts", UserRound],
  ["java", "Java and memory", Coffee],
  ["window", "Game window", Monitor],
  ["hooks", "Launch hooks", Terminal],
  ["storage", "Downloads and storage", HardDrive],
  ["integrations", "Integrations", Radio],
] as const;
export type SettingsTab = typeof settingTabs[number][0];

export function SettingsPage({ onSignIn, focusSection, onFocused }: {
  onSignIn: () => void; focusSection: SettingsTab | null; onFocused: () => void;
}) {
  const settings = useLauncher((state) => state.settings);
  const dataDir = useLauncher((state) => state.dataDir);
  const accounts = useLauncher((state) => state.accounts);
  const notify = useLauncher((state) => state.notify);
  const [tab, setTab] = useState<SettingsTab>(focusSection ?? "general");
  const [draft, setDraft] = useState(settings);
  const [jvm, setJvm] = useState(settings.jvmArgs?.join("\n") ?? "");
  const [environment, setEnvironment] = useState(environmentText(settings.environment));
  const [saveError, setSaveError] = useState("");
  const [saving, setSaving] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [java, setJava] = useState<JavaVersion[] | null>(null);
  const hasKey = useLauncher((state) => state.curseForgeConfigured);
  const [apiKey, setApiKey] = useState("");
  const [keySaving, setKeySaving] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [discordStatus, setDiscordStatus] = useState<DiscordStatus>("off");

  useEffect(() => {
    setDraft(settings);
    setJvm(settings.jvmArgs?.join("\n") ?? "");
    setEnvironment(environmentText(settings.environment));
  }, [settings]);

  useEffect(() => {
    if (!focusSection) return;
    if (tab !== focusSection) {
      setTab(focusSection);
      return;
    }
    document.getElementById("default-" + focusSection)?.focus();
    onFocused();
  }, [focusSection, onFocused, tab]);

  useEffect(() => {
    if (tab !== "integrations" || hasKey !== null) return;
    let live = true;
    call<boolean>("curseforge_status").then((status) => {
      if (live && useLauncher.getState().curseForgeConfigured === null) useLauncher.setState({ curseForgeConfigured: status });
    }).catch(() => {});
    return () => { live = false; };
  }, [tab, hasKey]);

  useEffect(() => {
    if (!settings.discordPresence) {
      setDiscordStatus("off");
      return;
    }
    let live = true;
    function refresh() {
      void call<DiscordStatus>("discord_presence_status").then((status) => {
        if (live) setDiscordStatus(status);
      }).catch(() => {});
    }
    refresh();
    const timer = window.setInterval(refresh, 2500);
    return () => { live = false; window.clearInterval(timer); };
  }, [settings.discordPresence]);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!Number.isInteger(draft.downloadLimit) || draft.downloadLimit < 1 || draft.downloadLimit > 16
      || draft.snapshotKeep < 1 || draft.snapshotKeep > 500
      || draft.snapshotLimitMib < 256 || draft.snapshotLimitMib > 1048576) {
      setTab("storage");
      setSaveError("Review the download limit and snapshot settings.");
      return;
    }
    setSaving(true);
    setSaveError("");
    try {
      const next = await call<Settings>("save_settings", { settings: { ...draft, jvmArgs: argumentsFrom(jvm), environment: environmentFrom(environment) } });
      useLauncher.setState({ settings: next });
      notify("Launcher settings saved.", "success");
    } catch (error) {
      setSaveError(errorText(error));
      notify(errorText(error), "error");
    } finally {
      setSaving(false);
    }
  }

  function saveKey(value: string) {
    setKeySaving(true);
    void runAction(async () => {
      useLauncher.setState({ curseForgeConfigured: await call<boolean>("save_curseforge_key", { value }) });
      setApiKey("");
    }, value ? "CurseForge key saved." : "Saved CurseForge key removed.").finally(() => setKeySaving(false));
  }

  return <>
    <div className="page-heading"><div><h1>Settings</h1><p>Manage accounts, Java, and launcher preferences.</p></div></div>
    <div className="settings-layout launcher-settings-layout">
      <nav className="panel launcher-settings-nav" aria-label="Settings sections">
        {[
          { title: "Launcher", sections: ["general", "storage", "integrations"] },
          { title: "Minecraft", sections: ["accounts", "java", "window", "hooks"] },
        ].map(({ title, sections }) => <div className="launcher-settings-group" key={title} role="group" aria-label={title}>
          <p>{title}</p>
          {settingTabs.filter(([id]) => sections.includes(id)).map(([id, title, Icon]) => <button key={id} type="button" aria-current={tab === id ? "page" : undefined}
            onClick={() => { setTab(id); setSaveError(""); }}><Icon size={17} /> {title}</button>)}
        </div>)}
      </nav>
      <div className="launcher-settings-content tab-content" key={tab}>
        {tab === "accounts" && <section className="panel settings-panel" id="default-accounts" tabIndex={-1}>
          <div className="settings-title"><UserRound /><div><h2>Minecraft accounts</h2><p>Your active account is used when you play.</p></div></div>
          {accounts.accounts.length > 0 && <ul className="rows account-list">{accounts.accounts.map((account) => <li className="data-row" key={account.id}>
            <AccountAvatar account={account} />
            <div className="row-main"><strong>{account.name}</strong><p className="muted">Microsoft account</p></div>
            {account.id === accounts.activeId ? <span className="status-tag green"><Check size={14} /> Active</span> : <Button size="small" onClick={() => void runAction(async () => {
              useLauncher.setState({ accounts: await call<Accounts>("select_account", { id: account.id }) });
            }, "Active account changed.")}>Activate</Button>}
            <Button size="icon" variant="ghost" aria-label={"Remove " + account.name} onClick={() => setRemoving(account.id)}><X /></Button>
          </li>)}</ul>}
          <Button onClick={onSignIn}><UserRound /> Connect Microsoft account</Button>
        </section>}
        {tab !== "accounts" && <form className="settings-form" onSubmit={save}>
          {tab === "general" && <section className="panel settings-panel">
            <div className="settings-title"><Monitor /><div><h2>Appearance</h2></div></div>
            <div className="form-grid">
              <label>Appearance<select value={draft.theme} onChange={(event) => setDraft({ ...draft, theme: event.target.value as Theme })}>
                <option value="dark">Dark</option><option value="light">Light</option><option value="system">Match the system</option>
              </select></label>
              <label>UI scale<select value={draft.uiScale ?? 115} onChange={(event) => setDraft({ ...draft, uiScale: Number(event.target.value) })}>
                {Array.from({ length: 13 }, (_, index) => 90 + index * 5).map((scale) => <option value={scale} key={scale}>{scale}%</option>)}
              </select></label>
            </div>
            <label className="inline-checkbox">
              <input type="checkbox" checked={draft.animations !== false}
                onChange={(event) => setDraft({ ...draft, animations: event.target.checked })} />
              Animate tabs and buttons
            </label>
          </section>}
          {tab === "integrations" && <section className="panel settings-panel" id="default-integrations" tabIndex={-1}>
            <div className="settings-title"><Radio /><div><h2>Discord Rich Presence</h2>
              <p>Show Voltra while the launcher is open, or share your Minecraft version and play time through Discord.</p></div></div>
            <label className="inline-checkbox">
              <input type="checkbox" checked={draft.discordPresence}
                onChange={(event) => setDraft({ ...draft, discordPresence: event.target.checked })} />
              Show Voltra activity on Discord
            </label>
            <label className="inline-checkbox">
              <input type="checkbox" checked={draft.discordShowPack} disabled={!draft.discordPresence}
                onChange={(event) => setDraft({ ...draft, discordShowPack: event.target.checked })} />
              Include the modpack name
            </label>
            {(draft.discordPresence !== settings.discordPresence || draft.discordShowPack !== settings.discordShowPack)
              ? <p className="field-hint" role="status">Save preferences to apply your Discord settings.</p>
              : settings.discordPresence && <p className="field-hint" role="status">{discordMessages[discordStatus]}</p>}
          </section>}
          {tab === "java" && <section className="panel settings-panel default-launch-panel" id="default-java" tabIndex={-1}>
            <div className="settings-title"><Coffee /><div><h2>Java and memory</h2>
              <p>Voltra finds or installs the Java version required by each instance.</p></div></div>
            <MemorySlider label="Default memory" value={draft.ramMib} onChange={(ramMib) => setDraft({ ...draft, ramMib })} />
            <label>Java executable override<input value={draft.javaPath} onChange={(event) => setDraft({ ...draft, javaPath: event.target.value })}
              placeholder="Automatic detection" /></label>
            <div className="actions-wrap">
              <Button disabled={detecting} onClick={() => {
                setDetecting(true);
                void runAction(async () => setJava(await call<JavaVersion[]>("detect_java"))).finally(() => setDetecting(false));
              }}>{detecting ? <LoaderCircle className="spin" /> : <Search />} Detect Java</Button>
            </div>
            {java && (java.length ? <ul className="detected-java">{java.map((item) => <li key={item.path}>
              <span><strong>Java {item.major}</strong><code>{item.path}</code></span><Button size="small" onClick={() => {
                setDraft((previous) => ({ ...previous, javaPath: item.path }));
                notify("Save preferences to use this Java override.");
              }}>Use</Button>
            </li>)}</ul> : <p className="field-hint">No local 64-bit Java installation was found.</p>)}
            <label>Default Java arguments<textarea rows={3} placeholder="One argument per line"
              value={jvm} onChange={(event) => setJvm(event.target.value)} spellCheck={false} /></label>
            <label>Default environment variables<textarea rows={3} placeholder="NAME=value"
              value={environment} onChange={(event) => setEnvironment(event.target.value)} spellCheck={false} /></label>
            <JavaRuntimes />
          </section>}
          {tab === "window" && <section className="panel settings-panel default-launch-panel" id="default-window" tabIndex={-1}>
            <div className="settings-title"><Monitor /><h2>Default game window</h2></div>
            <WindowFields value={draft.window ?? {}} onChange={(window) => setDraft({ ...draft, window })} />
          </section>}
          {tab === "hooks" && <section className="panel settings-panel default-launch-panel" id="default-hooks" tabIndex={-1}>
            <div className="settings-title"><Terminal /><h2>Default launch hooks</h2></div>
            <HookFields value={draft.hooks ?? {}} onChange={(hooks) => setDraft({ ...draft, hooks })} />
          </section>}
          {tab === "storage" && <section className="panel settings-panel">
            <div className="settings-title"><HardDrive /><h2>Downloads and storage</h2></div>
            <div className="form-grid">
              <label>Snapshots to keep per pack<input type="number" min={1} max={500} required value={draft.snapshotKeep}
                onChange={(event) => setDraft({ ...draft, snapshotKeep: Number(event.target.value) })} /></label>
              <label>Snapshot budget per pack (MiB)<input type="number" min={256} max={1048576} required value={draft.snapshotLimitMib}
                onChange={(event) => setDraft({ ...draft, snapshotLimitMib: Number(event.target.value) })} /></label>
              <label>Concurrent downloads<input type="number" min={1} max={16} value={draft.downloadLimit || ""} required
                onChange={(event) => setDraft({ ...draft, downloadLimit: event.target.valueAsNumber })} /></label>
            </div>
            <div className="actions-wrap">
              <Button onClick={() => void runAction(() => call("open_folder", { id: null, folder: "data" }))}><FolderOpen /> Open data folder</Button>
            </div>
            {dataDir && <code className="path-label">{dataDir}</code>}
          </section>}
          {saveError && <p className="inline-error" role="alert">{saveError}</p>}
          <div className="settings-save"><Button variant="primary" type="submit" disabled={saving}>{saving ? "Saving…" : "Save preferences"}</Button></div>
        </form>}
        {tab === "integrations" && <section className="panel settings-panel">
          <div className="settings-title"><KeyRound /><div><h2>CurseForge API</h2><p>{hasKey ? "An API key is configured." : "Connect your authorized key to search and install available mods."}</p></div></div>
          <form className="form-stack" onSubmit={(event) => { event.preventDefault(); saveKey(apiKey); }}>
            <label>API key<input value={apiKey} type="password" onChange={(event) => setApiKey(event.target.value)} autoComplete="off" required /></label>
            <div className="actions-wrap"><Button disabled={keySaving || !apiKey.trim()} type="submit">Save key</Button>
              {hasKey && <Button disabled={keySaving} variant="ghost" onClick={() => saveKey("")}>Remove saved key</Button>}</div>
          </form>
        </section>}
      </div>
    </div>
    {removing && <Dialog open title="Remove this account?" description="Voltra will remove its saved sign-in credential from this computer."
      onOpenChange={(open) => { if (!open) setRemoving(null); }}>
      <div className="dialog-actions"><Button variant="ghost" onClick={() => setRemoving(null)}>Cancel</Button>
        <Button variant="danger" onClick={() => { const id = removing; setRemoving(null); void runAction(async () => {
          useLauncher.setState({ accounts: await call<Accounts>("remove_account", { id }) });
        }, "Account removed."); }}>Remove account</Button></div>
    </Dialog>}
  </>;
}
