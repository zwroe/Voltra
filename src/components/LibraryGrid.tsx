import { Select } from "./ui/select";
import { useCallback, useState } from "react";
import { createPortal } from "react-dom";
import { Archive, ChevronRight, Folder, FolderOpen, FolderPlus, GripVertical, Layers3, Pencil, Play, Plus, Search, Trash2 } from "lucide-react";
import { call, errorText, isActiveJob, loaderNames, type Instance } from "../api";
import { useLauncher } from "../store";
import { InstanceIcon } from "./InstanceArtwork";
import { useLibraryDrag } from "./useLibraryDrag";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

type Edit = { kind: "new" | "rename" | "remove"; id?: string } | { kind: "move" | "archive"; id: string };

export function LibraryGrid({ instances, onCreate, onPlay }: {
  instances: Instance[]; onCreate: (folder?: string) => void; onPlay: (instance: Instance) => void;
}) {
  const { folders, membership } = useLauncher((state) => state.libraryLayout);
  const folder = useLauncher((state) => state.libraryFolder);
  const setFolder = useLauncher((state) => state.setLibraryFolder);
  const save = useLauncher((state) => state.saveLibraryLayout);
  const move = useLauncher((state) => state.moveInstance);
  const view = useLauncher((state) => state.view);
  const jobs = useLauncher((state) => state.jobs);
  const running = useLauncher((state) => state.running);
  const notify = useLauncher((state) => state.notify);
  const animations = useLauncher((state) => state.settings.animations !== false);
  const [filter, setFilter] = useState("");
  const [edit, setEdit] = useState<Edit | null>(null);
  const [value, setValue] = useState("");
  const [archiving, setArchiving] = useState(false);
  const [archiveError, setArchiveError] = useState("");
  const current = folders.find((item) => item.id === folder);
  const needle = filter.trim().toLocaleLowerCase();
  const busy = new Set([...running.map((entry) => entry.instanceId), ...jobs.filter(isActiveJob).map((job) => job.instanceId)]);
  const counts = new Map<string, number>();
  for (const item of instances) {
    const location = membership[item.id] ?? "";
    counts.set(location, (counts.get(location) ?? 0) + 1);
  }
  const shown = instances.filter((item) => needle
    ? item.name.toLocaleLowerCase().includes(needle)
    : (membership[item.id] ?? "") === (current?.id ?? ""));
  const name = edit?.kind === "move" || edit?.kind === "archive"
    ? instances.find((item) => item.id === edit.id)?.name
    : folders.find((item) => item.id === edit?.id)?.name;
  const collides = folders.some((item) => item.id !== edit?.id && item.name.toLocaleLowerCase() === value.trim().toLocaleLowerCase());

  const openArchive = useCallback((id: string) => {
    setArchiveError("");
    setEdit({ kind: "archive", id });
  }, []);

  const drop = useCallback((id: string, target: string) => {
    if (target === "archive") {
      openArchive(id);
      return;
    }
    const destination = target === "library" ? "" : target.slice("folder:".length);
    if ((membership[id] ?? "") === destination) return;
    if (destination && !folders.some((item) => item.id === destination)) return;
    move(id, destination);
  }, [folders, membership, move, openArchive]);
  const drag = useLibraryDrag(animations, drop);
  const lifted = drag.lifted ? instances.find((item) => item.id === drag.lifted?.id) : undefined;
  const dropName = drag.hover === "archive" ? "Archive instance"
    : drag.hover === "library" ? "Move to Library"
    : drag.hover ? `Move to ${folders.find((item) => `folder:${item.id}` === drag.hover)?.name ?? "folder"}` : null;

  async function apply() {
    if (!edit) return;
    if (edit.kind === "archive") {
      setArchiving(true);
      setArchiveError("");
      try {
        await call("archive_instance", { id: edit.id });
        setEdit(null);
        await useLauncher.getState().refreshInstances();
        notify("Instance archived.", "success");
      } catch (error) {
        setArchiveError(errorText(error));
        notify(errorText(error), "error");
      } finally {
        setArchiving(false);
      }
      return;
    }
    if (edit.kind === "move") move(edit.id, value);
    else if (edit.kind === "remove") {
      save({ folders: folders.filter((item) => item.id !== edit.id), membership: Object.fromEntries(Object.entries(membership).filter(([, id]) => id !== edit.id)) });
      setFolder("");
    } else if (value.trim() && !collides) {
      const label = value.trim();
      save({ membership, folders: edit.kind === "new" ? [...folders, { id: crypto.randomUUID(), name: label }] : folders.map((item) => item.id === edit.id ? { ...item, name: label } : item) });
    } else return;
    setEdit(null);
  }

  function openFolder(id: string) {
    setFilter("");
    setFolder(id);
  }

  return <>
    <div className="library-layout" data-library-dragging={!!lifted}>
      <aside className="library-collections" aria-label="Library folders">
        <div className="library-collections-heading"><h2>Folders</h2>
          <Button size="icon" variant="ghost" title="New folder" aria-label="New folder" onClick={() => { setValue(""); setEdit({ kind: "new" }); }}><FolderPlus /></Button>
        </div>
        <div className="library-collections-list">
          <button type="button" className="library-folder" {...drag.targetProps("library", !!lifted && !membership[lifted.id])}
            aria-current={!current ? "location" : undefined} onClick={() => openFolder("")}>
            <Layers3 size={17} /><strong>Library</strong><span>{counts.get("") ?? 0}</span>
          </button>
          {folders.map((item) => <button key={item.id} type="button" className="library-folder"
            {...drag.targetProps(`folder:${item.id}`, !!lifted && membership[lifted.id] === item.id)}
            aria-current={current?.id === item.id ? "location" : undefined} onClick={() => openFolder(item.id)}>
            {current?.id === item.id ? <FolderOpen size={17} /> : <Folder size={17} />}
            <strong>{item.name}</strong><span>{counts.get(item.id) ?? 0}</span>
          </button>)}
        </div>
        <div className="library-delete-target" {...drag.targetProps("archive", !!lifted && busy.has(lifted.id))}>
          <Archive size={18} /><div><strong>Archive</strong><span>{lifted && busy.has(lifted.id) ? "Instance is busy" : "Drop to move to Backups"}</span></div>
        </div>
      </aside>
      <section className="library-shelf" aria-label={needle ? "Search results" : current?.name ?? "Library instances"}>
        <div className="library-tools">
          <label className="search-field"><Search size={17} /><input type="search" aria-label="Filter instances" placeholder="Search all instances" value={filter} onChange={(event) => setFilter(event.target.value)} /></label>
        </div>
        <div className="library-shelf-heading">
          <div>
            {current && !needle && <button className="library-crumb" type="button" onClick={() => openFolder("")}>Library <ChevronRight size={13} /></button>}
            <h2>{needle ? "Search results" : current?.name ?? "Instances"}</h2>
            <span className="library-instance-count">{shown.length}</span>
          </div>
          {current && !needle && <div className="actions-wrap">
            <Button size="icon" variant="ghost" aria-label="Rename folder" title="Rename folder" onClick={() => { setValue(current.name); setEdit({ kind: "rename", id: current.id }); }}><Pencil /></Button>
            <Button size="icon" variant="ghost" aria-label="Remove folder" title="Remove folder" onClick={() => setEdit({ kind: "remove", id: current.id })}><Trash2 /></Button>
          </div>}
        </div>
        <div className="library-grid">
          {shown.map((item) => {
            const game = running.find((entry) => entry.instanceId === item.id);
            const location = needle ? folders.find((entry) => entry.id === membership[item.id])?.name : undefined;
            return <article key={item.id} className="library-tile library-pack" data-instance-tile data-dragging={lifted?.id === item.id}>
              <button type="button" className="library-pack-open" aria-label={`Open ${item.name}`}
                onDragStart={(event) => event.preventDefault()}
                onPointerDown={(event) => drag.pickUp(event, item.id)}
                onClick={(event) => { if (!drag.consumeClick(event, item.id)) view(item.id); }}>
                <TileFace item={item} location={location} />
              </button>
              <div className="library-pack-actions">
                <button type="button" className="library-grip" title="Drag or choose a folder" aria-label={`Move ${item.name} to folder`}
                  onPointerDown={(event) => drag.pickUp(event, item.id, true)}
                  onClick={(event) => {
                    if (drag.consumeClick(event, item.id)) return;
                    setValue(membership[item.id] ?? "");
                    setEdit({ kind: "move", id: item.id });
                  }}><GripVertical size={17} /></button>
                <button type="button" title="Archive instance" aria-label={`Archive ${item.name}`} disabled={busy.has(item.id)} onClick={() => openArchive(item.id)}><Archive size={15} /></button>
                <button type="button" className="library-play" title={game ? "Minecraft is running" : "Play"} aria-label={`Play ${item.name}`} disabled={busy.has(item.id)} onClick={() => onPlay(item)}><Play size={15} fill="currentColor" /></button>
              </div>
            </article>;
          })}
          {!needle && <button type="button" className="library-tile library-create" onClick={() => onCreate(current?.id)}><Plus size={26} /><span>Create instance</span></button>}
        </div>
        {needle && !shown.length && <p className="muted">No instances match that search.</p>}
      </section>
    </div>
    {lifted && drag.lifted && createPortal(
      <div className="library-drag-preview" ref={drag.preview} aria-hidden="true" style={{ width: drag.lifted.width, height: drag.lifted.height }}>
        {dropName && <span className="library-drag-destination" data-archive={drag.hover === "archive"}>{dropName}</span>}
        <div className="library-tile library-pack"><TileFace item={lifted} /></div>
      </div>,
      document.body,
    )}
    {edit && <Dialog open title={{ new: "New folder", rename: "Rename folder", remove: "Remove folder", move: "Move instance", archive: "Remove instance from Library?" }[edit.kind]}
      description={edit.kind === "archive" ? `${name ?? "This instance"} and its worlds will move to Library Backups, where you can restore them.`
        : edit.kind === "remove" ? `Instances in ${name ?? "this folder"} will move back to the Library.`
        : edit.kind === "move" ? `Choose a folder for ${name ?? "this instance"}.` : "Folders organize instances in your Library."}
      onOpenChange={(open) => { if (!open && !archiving) setEdit(null); }}>
      <form className="form-stack" onSubmit={(event) => { event.preventDefault(); void apply(); }}>
        {edit.kind === "move" ? <label>Folder<Select value={value} onValueChange={(value) => setValue(value)}><option value="">Library</option>{folders.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}</Select></label>
          : edit.kind !== "remove" && edit.kind !== "archive" && <label>Folder name<input autoFocus required value={value} maxLength={60} onChange={(event) => setValue(event.target.value)} /></label>}
        {edit.kind === "archive" && archiveError && <p className="inline-error" role="alert">{archiveError}</p>}
        <div className="dialog-actions"><Button variant="ghost" disabled={archiving} onClick={() => setEdit(null)}>Cancel</Button>
          <Button type="submit" variant={edit.kind === "remove" || edit.kind === "archive" ? "danger" : "primary"} disabled={archiving || ((edit.kind === "new" || edit.kind === "rename") && (!value.trim() || collides))}>{archiving ? "Archiving…" : { move: "Move", remove: "Remove folder", archive: "Move to Backups", new: "Save folder", rename: "Save folder" }[edit.kind]}</Button></div>
      </form>
    </Dialog>}
  </>;
}

function TileFace({ item, location }: { item: Instance; location?: string }) {
  return <>
    <div className="library-pack-art"><InstanceIcon loader={item.loader} icon={item.icon} size={48} /></div>
    <div className="library-pack-label"><strong>{item.name}</strong>
      <span>{loaderNames[item.loader]} · {item.minecraftVersion}</span>
      {location && <span className="library-pack-location"><Folder size={12} /> {location}</span>}
    </div>
  </>;
}
