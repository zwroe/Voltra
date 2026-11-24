import { Select } from "./ui/select";
import { useEffect, useRef, useState } from "react";
import { ExternalLink, FolderOpen, Image, Search } from "lucide-react";
import { call, displayDate, errorText, type Screenshot } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";

function ScreenshotImage({ shot, eager = false }: { shot: Screenshot; eager?: boolean }) {
  const [src, setSrc] = useState("");
  const [ready, setReady] = useState(eager);
  const [error, setError] = useState(false);
  const holder = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (ready || !holder.current) return;
    const observer = new IntersectionObserver(([item]) => {
      if (item?.isIntersecting) { setReady(true); observer.disconnect(); }
    }, { rootMargin: "120px" });
    observer.observe(holder.current);
    return () => observer.disconnect();
  }, [ready]);
  useEffect(() => {
    if (!ready) return;
    let live = true;
    let url = "";
    call<ArrayBuffer | number[]>("read_screenshot", { id: shot.instanceId, name: shot.name }).then((bytes) => {
      if (!live) return;
      url = URL.createObjectURL(new Blob([Array.isArray(bytes) ? Uint8Array.from(bytes) : bytes], { type: "image/png" }));
      setSrc(url);
    }).catch(() => { if (live) setError(true); });
    return () => { live = false; if (url) URL.revokeObjectURL(url); };
  }, [ready, shot.instanceId, shot.name]);
  return <span className="shot-image" ref={holder}>{src ? <img src={src} alt={shot.name} /> : error ? <span>Preview unavailable</span> : <Image />}</span>;
}

export function Screenshots({ instanceId }: { instanceId?: string }) {
  const instances = useLauncher((state) => state.instances);
  const [instance, setInstance] = useState(instanceId ?? "");
  const [shots, setShots] = useState<Screenshot[] | null>(null);
  const [search, setSearch] = useState("");
  const [newest, setNewest] = useState(true);
  const [groupBy, setGroupBy] = useState<"date" | "instance">("date");
  const [preview, setPreview] = useState<Screenshot | null>(null);
  const [error, setError] = useState("");
  const [limit, setLimit] = useState(36);
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    let live = true;
    setShots(null);
    setError("");
    call<Screenshot[]>("list_screenshots", { id: instance || null }).then((data) => {
      if (live) setShots(data);
    }).catch((reason) => { if (live) setError(errorText(reason)); });
    return () => { live = false; };
  }, [instance, refresh]);

  useEffect(() => {
    if (!preview) return;
    const close = (event: KeyboardEvent) => { if (event.key === "Escape") setPreview(null); };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [preview]);

  const visible = (shots ?? []).filter((shot) => (shot.name + " " + shot.instance).toLowerCase().includes(search.trim().toLowerCase()));
  if (!newest) visible.reverse();
  const groups = new Map<string, Screenshot[]>();
  for (const shot of visible.slice(0, limit)) {
    const day = new Date(shot.takenAt * 1000);
    const date = day.toLocaleDateString(undefined, { dateStyle: "long" });
    const key = groupBy === "date" ? (day.toDateString() === new Date().toDateString() ? "Today" : date) : shot.instance;
    const group = groups.get(key) ?? [];
    group.push(shot);
    groups.set(key, group);
  }

  return <>
    {!instanceId && <div className="page-heading"><div><h1>Screenshots</h1><p>Browse captures from your local Minecraft instances.</p></div></div>}
    <div className="gallery-toolbar">
      <label className="search-field"><Search size={17} /><input aria-label="Search screenshots" value={search}
        onChange={(event) => { setSearch(event.target.value); setLimit(36); }} placeholder="Search screenshots" /></label>
      {!instanceId && <Select aria-label="Screenshot instance" value={instance} onValueChange={(value) => { setInstance(value); setLimit(36); }}>
        <option value="">All instances</option>{instances.map((item) => <option value={item.id} key={item.id}>{item.name}</option>)}
      </Select>}
      <Select aria-label="Screenshot sorting" value={newest ? "new" : "old"} onValueChange={(value) => setNewest(value === "new")}><option value="new">Newest</option><option value="old">Oldest</option></Select>
      <Select aria-label="Group screenshots by" value={groupBy} onValueChange={(value) => setGroupBy(value as "date" | "instance")}><option value="date">Group by date</option><option value="instance">Group by instance</option></Select>
      <Button size="small" onClick={() => setRefresh((value) => value + 1)}>Refresh</Button>
      {instance && <Button size="small" onClick={() => void runAction(() => call("open_folder", { id: instance, folder: "screenshots" }))}><FolderOpen /> Folder</Button>}
    </div>
    {error ? <p className="inline-error" role="alert">{error}</p> : !shots ? <p className="waiting">Loading screenshots…</p>
      : !visible.length ? <div className="empty"><Image /><h2>{search ? "No matches" : "No screenshots yet"}</h2>
        <p>{search ? "Try another filename or instance." : "Press F2 in Minecraft to take a screenshot."}</p></div>
      : <div className="shot-groups">{Array.from(groups, ([title, photos]) => <section key={title}>
        <h2>{title} <span className="muted">{photos.length}</span></h2>
        <div className="shot-grid">{photos.map((shot) => <button className="shot-card" key={shot.instanceId + shot.name} onClick={() => setPreview(shot)}>
          <ScreenshotImage shot={shot} /><span><strong>{shot.name.replace(/\.png$/i, "")}</strong><span>{shot.instance}</span></span>
        </button>)}</div>
      </section>)}</div>}
    {visible.length > limit && <Button className="gallery-more" onClick={() => setLimit(limit + 36)}>Load more</Button>}
    {preview && <div className="photo-overlay" role="presentation" onClick={() => setPreview(null)}>
      <section role="dialog" aria-modal="true" aria-label={preview.name} className="photo-viewer" onClick={(event) => event.stopPropagation()}>
        <ScreenshotImage key={preview.instanceId + preview.name} shot={preview} eager />
        <div className="photo-caption"><div><strong>{preview.name}</strong><p className="muted">{preview.instance} · {displayDate(preview.takenAt)}</p></div>
          <Button onClick={() => void runAction(() => call("open_screenshot", { id: preview.instanceId, name: preview.name }))}><ExternalLink /> Open file</Button>
          <Button autoFocus onClick={() => setPreview(null)}>Close</Button>
        </div>
      </section>
    </div>}
  </>;
}
