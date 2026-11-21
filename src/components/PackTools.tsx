import { useEffect, useState } from "react";
import { LoaderCircle } from "lucide-react";
import { call, errorText } from "../api";
import { Button } from "./ui/button";

interface Query<T> { value: T | null; error: string; loading: boolean; refresh: () => void }

export function usePackQuery<T>(command: string, args: Record<string, unknown>, revision?: unknown, enabled = true): Query<T> {
  const [version, setVersion] = useState(0);
  const key = JSON.stringify(args);
  const [result, setResult] = useState<{ key: string; value: T | null; error: string; pending: boolean } | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let current = true;
    setResult({ key, value: null, error: "", pending: true });
    call<T>(command, JSON.parse(key) as Record<string, unknown>).then((value) => {
      if (current) setResult({ key, value, error: "", pending: false });
    }).catch((cause: unknown) => {
      if (current) setResult({ key, value: null, error: errorText(cause), pending: false });
    });
    return () => { current = false; };
  }, [command, key, version, revision, enabled]);
  const data = enabled && result?.key === key ? result : null;
  return { value: data?.value ?? null, error: data?.error ?? "", loading: enabled && (data?.pending ?? true), refresh: () => setVersion((value) => value + 1) };
}

export function ReadState({ query, label = "Loading…" }: { query: Query<unknown>; label?: string }) {
  return query.error ? <div className="actions-wrap"><p className="inline-error" role="alert">{query.error}</p><Button size="small" onClick={query.refresh}>Retry</Button></div>
    : query.loading ? <p className="waiting" role="status"><LoaderCircle className="spin" /> {label}</p> : null;
}

export interface Difference { path: string; before: { bytes: number; sha256: string } | null; after: { bytes: number; sha256: string } | null }

export function formatPackSize(bytes: number) {
  if (bytes >= 2 ** 30) return `${(bytes / 2 ** 30).toFixed(2)} GiB`;
  if (bytes >= 2 ** 20) return `${(bytes / 2 ** 20).toFixed(1)} MiB`;
  return `${Math.ceil(bytes / 1024)} KiB`;
}

export function PackDifferences({ changes, selections, onSelect }: { changes: Difference[]; selections?: Set<string>; onSelect?: (path: string) => void }) {
  return changes.length ? <div className="table-scroll"><table className="pack-table"><thead><tr>
    {onSelect && <th>Copy</th>}<th>File</th><th>Change</th><th>Before</th><th>After</th>
  </tr></thead><tbody>{changes.map((item) => <tr key={item.path}>
    {onSelect && <td>{item.after && <input type="checkbox" aria-label={`Copy ${item.path}`} checked={selections?.has(item.path) ?? false} onChange={() => onSelect(item.path)} />}</td>}
    <td>{item.path}</td><td>{item.before ? item.after ? "Changed" : "Removed" : "Added"}</td>
    <td title={item.before?.sha256}>{item.before ? formatPackSize(item.before.bytes) : "—"}</td>
    <td title={item.after?.sha256}>{item.after ? formatPackSize(item.after.bytes) : "—"}</td>
  </tr>)}</tbody></table></div> : <p className="muted">These pack files match.</p>;
}
