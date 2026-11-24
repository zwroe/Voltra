import { lazy, Suspense, useEffect, useState } from "react";
import { ArrowDownToLine, LoaderCircle, Search } from "lucide-react";
import { call, errorText } from "../api";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

const ProjectDescription = lazy(() => import("./ProjectDescription"));
export type VersionChoice = { id: string; label: string; date: string; channel: string; changelog?: string };
type VersionList = { current: string; project: string; platform: "modrinth" | "curseforge"; versions: VersionChoice[] };

export function ModVersionChooser({ id, file, title, locked, onClose, onSelect }: {
  id: string; file: string; title?: string; locked: boolean; onClose: () => void; onSelect: (version: VersionChoice) => void;
}) {
  const [result, setResult] = useState<VersionList>();
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [pick, setPick] = useState("");
  const [notes, setNotes] = useState<{ key: string; body: string }>();
  const [retry, setRetry] = useState(0);
  const filtered = result?.versions.filter((version) => version.label.toLowerCase().includes(query.trim().toLowerCase())) ?? [];
  const choice = filtered.find((version) => version.id === pick) ?? filtered[0];
  const changelogKey = result && choice ? result.project + ":" + choice.id : "";

  useEffect(() => {
    let live = true;
    setError("");
    call<VersionList>("list_mod_update_versions", { id, file }).then((data) => {
      if (live) setResult(data);
    }).catch((reason) => { if (live) setError(errorText(reason)); });
    return () => { live = false; };
  }, [id, file, retry]);

  useEffect(() => {
    if (result?.platform !== "curseforge" || !choice) return;
    let live = true;
    call<string>("mod_version_changelog", { project: result.project, version: choice.id })
      .then((body) => { if (live) setNotes({ key: changelogKey, body }); })
      .catch(() => { if (live) setNotes({ key: changelogKey, body: "" }); });
    return () => { live = false; };
  }, [result, choice, changelogKey]);

  const body = choice?.changelog || (notes?.key === changelogKey ? notes.body : "");
  const loadingNotes = result?.platform === "curseforge" && notes?.key !== changelogKey;
  return <Dialog open title={title ? "Update " + title : "Update mod"}
    description="Choose a version for this Minecraft version and loader."
    className="project-details-dialog mod-version-dialog" onOpenChange={(open) => { if (!open) onClose(); }}>
    {error && <div className="project-load-error"><p className="inline-error" role="alert">{error}</p>
      <Button size="small" onClick={() => setRetry((value) => value + 1)}>Try again</Button></div>}
    {!result && !error && <p className="waiting"><LoaderCircle className="spin" /> Loading compatible versions.</p>}
    {result && <div className="mod-version-layout">
      <div className="mod-version-picker">
        <label className="search-field"><Search size={16} /><input type="search" placeholder="Search versions" aria-label="Search versions"
          value={query} onChange={(event) => setQuery(event.target.value)} /></label>
        <ul aria-label="Compatible versions">{filtered.map((version) => <li key={version.id}><button type="button" className="version-option"
          aria-pressed={choice?.id === version.id} onClick={() => setPick(version.id)}>
          <span>{version.label}</span>{version.id === result.current && <span className="status-tag">Installed</span>}
        </button></li>)}</ul>
        {!filtered.length && <p className="muted">{result.versions.length ? "No matching versions." : "No compatible versions found."}</p>}
      </div>
      {choice && <section className="mod-version-notes" aria-label="Changelog">
        <header><h2>{choice.label}</h2><span className="status-tag">{choice.channel}</span>
          {Number.isFinite(Date.parse(choice.date)) && <time dateTime={choice.date}>{new Date(choice.date).toLocaleDateString()}</time>}</header>
        {body ? <Suspense fallback={<p className="muted">Loading changelog…</p>}><ProjectDescription body={body} /></Suspense>
          : <p className="muted">{loadingNotes ? "Loading changelog…" : "No changelog available."}</p>}
      </section>}
    </div>}
    <div className="dialog-actions">
      <Button variant="ghost" onClick={onClose}>Cancel</Button>
      <Button variant="primary" disabled={locked || !choice || choice.id === result?.current} onClick={() => { if (choice) onSelect(choice); }}>
        <ArrowDownToLine /> Review update
      </Button>
    </div>
  </Dialog>;
}
