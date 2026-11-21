import { useEffect, useMemo, useState, type FormEvent } from "react";
import { ArrowDown, ArrowUp, ChevronRight, Ellipsis, File, Folder, FolderOpen, FolderPlus, House, Plus, RefreshCw, Search, Trash2, Upload } from "lucide-react";
import { call, displayDate, errorText, type InstanceFile } from "../api";
import { useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

type Order = "name" | "size" | "createdAt" | "modifiedAt";
type Action = { kind: "open" } | { kind: "reveal" | "newFolder"; name: string } | { kind: "rename"; name: string; next: string } | { kind: "remove"; names: string[] };
type Editor = { mode: "newFolder" | "rename" | "remove"; name?: string; names?: string[] };
const pageSize = 60;
const labels: { key: Order; title: string }[] = [
  { key: "name", title: "Name" }, { key: "size", title: "Size" }, { key: "createdAt", title: "Created" }, { key: "modifiedAt", title: "Modified" },
];
const natural = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

function fileSize(file: InstanceFile) {
  if (file.directory) return file.size.toLocaleString() + (file.size === 1 ? " item" : " items");
  const units = ["B", "KB", "MB", "GB", "TB"];
  const power = file.size > 0 ? Math.min(4, Math.floor(Math.log(file.size) / Math.log(1024))) : 0;
  return (file.size / 1024 ** power).toLocaleString(undefined, { maximumFractionDigits: power ? 1 : 0 }) + " " + units[power];
}

export function InstanceFiles({ id, locked }: { id: string; locked: boolean }) {
  const [path, setPath] = useState<string[]>([]);
  const [rows, setRows] = useState<InstanceFile[]>([]);
  const [query, setQuery] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [editor, setEditor] = useState<Editor | null>(null);
  const [name, setName] = useState("");
  const [page, setPage] = useState(0);
  const [sort, setSort] = useState<{ order: Order; ascending: boolean }>({ order: "name", ascending: true });
  const [selected, setSelected] = useState<string[]>([]);
  const [menu, setMenu] = useState<string | null>(null);
  const disabled = locked || busy;
  const notify = useLauncher((state) => state.notify);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError("");
    call<InstanceFile[]>("list_instance_files", { id, path }).then((items) => {
      if (!alive) return;
      setRows(items);
      const names = new Set(items.map((item) => item.name));
      setSelected((previous) => previous.filter((entry) => names.has(entry)));
    }).catch((reason) => {
      if (alive) { setRows([]); setError(errorText(reason)); }
    }).finally(() => {
      if (alive) setLoading(false);
    });
    return () => { alive = false; };
  }, [id, path, refresh]);

  const items = useMemo(() => {
    const term = query.trim().toLocaleLowerCase();
    return rows.filter((item) => item.name.toLocaleLowerCase().includes(term)).sort((a, b) => {
      if (a.directory !== b.directory) return a.directory ? -1 : 1;
      const byName = natural.compare(a.name, b.name);
      const difference = sort.order === "name" ? byName : (a[sort.order] ?? 0) - (b[sort.order] ?? 0);
      return difference * (sort.ascending ? 1 : -1) || byName;
    });
  }, [rows, query, sort]);
  const pageCount = Math.max(1, Math.ceil(items.length / pageSize));
  const currentPage = Math.min(pageCount - 1, page);
  const visible = items.slice(currentPage * pageSize, (currentPage + 1) * pageSize);
  const allSelected = !!visible.length && visible.every((file) => selected.includes(file.name));

  function browse(next: string[]) {
    setPath(next);
    setRows([]);
    setPage(0);
    setQuery("");
    setSelected([]);
    setMenu(null);
  }

  function choose(file: string, checked: boolean) {
    setSelected((previous) => checked ? [...previous, file] : previous.filter((name) => name !== file));
  }

  function edit(next: Editor) {
    setMenu(null);
    setEditor(next);
    setName(next.name ?? "");
    setError("");
  }

  const action = (action: Action) => call<void>("instance_file_action", { id, path, action });

  async function mutate(work: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      const changed = await work();
      setEditor(null);
      if (changed !== false) {
        setRefresh((count) => count + 1);
        await useLauncher.getState().refreshInstances();
      }
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!editor || disabled) return;
    if (editor.mode === "remove") {
      const names = editor.names ?? [];
      void mutate(async () => {
        await action({ kind: "remove", names });
        notify("Items removed. A backup was saved.", "success");
      });
    } else if (editor.mode === "rename") {
      void mutate(() => action({ kind: "rename", name: editor.name ?? "", next: name.trim() }));
    } else {
      void mutate(() => action({ kind: "newFolder", name: name.trim() }));
    }
  }

  function open(actionDetails: Action) {
    setMenu(null);
    void action(actionDetails).catch((reason) => setError(errorText(reason)));
  }

  return <section className="instance-files" aria-label="Instance file explorer">
    <div className="files-toolbar">
      <nav className="file-breadcrumbs" aria-label="Folder path">
        <Button size="icon" aria-label="Game folder root" title="Game folder root" onClick={() => browse([])}><House size={17} /></Button>
        {path.map((segment, index) => <span key={index}><ChevronRight size={13} aria-hidden="true" />
          <button type="button" aria-current={index === path.length - 1 ? "location" : undefined}
            onClick={() => browse(path.slice(0, index + 1))}>{segment}</button></span>)}
      </nav>
      <Button variant="ghost" size="small" onClick={() => open({ kind: "open" })}><FolderOpen /> Open in Explorer</Button>
    </div>
    <div className="files-toolbar">
      <label className="search-field"><Search size={17} /><input aria-label="Search files" placeholder="Search this folder" type="search" value={query}
        onChange={(event) => { setPage(0); setQuery(event.target.value); }} /></label>
      <div className="actions-wrap">
        {selected.length > 0 && <Button size="small" variant="danger" disabled={disabled} onClick={() => edit({ mode: "remove", names: selected })}>
          <Trash2 /> Remove {selected.length}</Button>}
        <Button size="small" disabled={loading || busy} onClick={() => setRefresh((value) => value + 1)}><RefreshCw /> Refresh</Button>
        <div className="file-menu" onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget)) setMenu(null);
        }}>
          <Button size="small" disabled={disabled || loading} aria-haspopup="menu" aria-expanded={menu === "add"}
            onClick={() => setMenu(menu === "add" ? null : "add")}><Plus /> Add</Button>
          {menu === "add" && <div className="file-menu-popup">
            <button type="button" onClick={() => edit({ mode: "newFolder" })}><FolderPlus size={16} /> New folder</button>
            <button type="button" onClick={() => {
              setMenu(null);
              void mutate(() => call<boolean>("import_instance_file", { id, path }));
            }}><Upload size={16} /> Add file</button>
          </div>}
        </div>
      </div>
    </div>
    {!!error && !editor && <p className="inline-error" role="alert">{error}</p>}
    <div className="file-table-panel">
      <table className="file-table" aria-label="Files in this folder">
        <thead><tr><th scope="col" className="file-checkbox"><input type="checkbox" checked={allSelected}
          aria-label="Select all visible files" disabled={!visible.length} onChange={(event) => {
            const names = visible.map((item) => item.name);
            setSelected(event.target.checked ? Array.from(new Set([...selected, ...names])) : selected.filter((item) => !names.includes(item)));
          }} /></th>
          {labels.map(({ key, title }) => <th className={"file-column-" + key} scope="col" key={key}
            aria-sort={sort.order === key ? sort.ascending ? "ascending" : "descending" : "none"}>
            <button type="button" onClick={() => { setPage(0); setSort({ order: key, ascending: key !== sort.order || !sort.ascending }); }}>
              {title}{sort.order === key && (sort.ascending ? <ArrowUp size={13} /> : <ArrowDown size={13} />)}
            </button>
          </th>)}
          <th scope="col" className="file-column-actions"><span className="sr-only">Actions</span></th>
        </tr></thead>
        <tbody>
          {visible.map((file) => {
            const actionKey = "row:" + file.name;
            const Symbol = file.directory ? Folder : File;
            return <tr key={file.name} className={selected.includes(file.name) ? "file-selected" : undefined}>
              <td className="file-checkbox"><input type="checkbox" checked={selected.includes(file.name)}
                aria-label={"Select " + file.name} onChange={(event) => choose(file.name, event.target.checked)} /></td>
              <td className="file-column-name">
                <button className="file-name" type="button" title={file.name}
                  onClick={() => file.directory ? browse([...path, file.name]) : open({ kind: "reveal", name: file.name })}>
                  <Symbol size={17} /> <span>{file.name}</span>
                </button>
              </td>
              <td className="file-column-size">{fileSize(file)}</td>
              <td className="file-column-createdAt">{file.createdAt == null ? "—" : displayDate(file.createdAt)}</td>
              <td className="file-column-modifiedAt">{file.modifiedAt == null ? "—" : displayDate(file.modifiedAt)}</td>
              <td className="file-column-actions"><div className="file-menu" onBlur={(event) => {
                if (!event.currentTarget.contains(event.relatedTarget)) setMenu(null);
              }}>
                <Button size="icon" variant="ghost" aria-haspopup="menu" aria-expanded={menu === actionKey} aria-label={"Actions for " + file.name}
                  onClick={() => setMenu(menu === actionKey ? null : actionKey)}><Ellipsis size={17} /></Button>
                {menu === actionKey && <div className="file-menu-popup">
                  <button type="button" onClick={() => open({ kind: "reveal", name: file.name })}>Show in Explorer</button>
                  <button type="button" disabled={disabled} onClick={() => edit({ mode: "rename", name: file.name })}>Rename</button>
                  <button type="button" disabled={disabled} onClick={() => edit({ mode: "remove", names: [file.name] })}>Remove</button>
                </div>}
              </div></td>
            </tr>;
          })}
          {!visible.length && <tr><td className="file-empty" colSpan={6}>{loading ? "Loading files." : query ? "No files match your search." : "This folder is empty."}</td></tr>}
        </tbody>
      </table>
    </div>
    <div className="files-footer">
      <span>{items.length.toLocaleString()} {items.length === 1 ? "item" : "items"}{selected.length ? " · " + selected.length + " selected" : ""}</span>
      {pageCount > 1 && <div className="actions-wrap">
        <Button size="small" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>Previous</Button>
        <span>{currentPage + 1} / {pageCount}</span>
        <Button size="small" disabled={currentPage + 1 >= pageCount} onClick={() => setPage(currentPage + 1)}>Next</Button>
      </div>}
    </div>
    {editor && <Dialog open title={editor.mode === "remove" ? "Remove selected items?" : editor.mode === "rename" ? "Rename item" : "New folder"}
      description={editor.mode === "remove" ? "Voltra saves a full instance backup before removing these files." : "Enter a name for this folder location."}
      onOpenChange={(open) => { if (!open && !busy) setEditor(null); }}>
      <form className="form-stack" onSubmit={submit}>
        {editor.mode !== "remove" && <label>Name<input required maxLength={255} value={name} onChange={(event) => setName(event.target.value)} /></label>}
        {error && <p className="inline-error" role="alert">{error}</p>}
        <div className="dialog-actions">
          <Button disabled={busy} variant="ghost" onClick={() => setEditor(null)}>Cancel</Button>
          <Button type="submit" disabled={disabled || (editor.mode !== "remove" && !name.trim())} variant={editor.mode === "remove" ? "danger" : "primary"}>
            {busy ? "Working." : editor.mode === "remove" ? "Remove" : editor.mode === "rename" ? "Rename" : "Create folder"}
          </Button>
        </div>
      </form>
    </Dialog>}
  </section>;
}
