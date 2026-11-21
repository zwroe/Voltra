import { useEffect, useState } from "react";
import { Copy, ExternalLink, LoaderCircle, Upload } from "lucide-react";
import { call, errorText } from "../api";
import { runAction } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export type LogShareSource = {
  name: string;
  load: () => Promise<readonly [string, boolean]>;
};

export function LogShareDialog({ source, onClose }: { source: LogShareSource; onClose: () => void }) {
  const [text, setText] = useState("");
  const [truncated, setTruncated] = useState(false);
  const [loading, setLoading] = useState(true);
  const [uploading, setUploading] = useState(false);
  const [url, setUrl] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    async function load() {
      try {
        const [content, shortened] = await source.load();
        const clean = await call<string>("prepare_log_share", { content });
        if (live) {
          setTruncated(shortened);
          setText(clean);
        }
      } catch (cause) {
        if (live) setError(errorText(cause));
      } finally {
        if (live) setLoading(false);
      }
    }
    void load();
    return () => { live = false; };
  }, [source]);

  async function upload() {
    setUploading(true);
    setError("");
    try {
      setUrl(await call<string>("upload_log_share", { content: text }));
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      setUploading(false);
    }
  }

  return <Dialog open title={url ? "Log ready to share" : "Review log for mclo.gs"}
    description={url ? "Anyone with this link can read the published log." : "Voltra masks common secrets. Review the text and remove any remaining private details before uploading. Anyone with the link will be able to read it."}
    className="project-details-dialog" onOpenChange={(open) => { if (!open && !uploading) onClose(); }}>
    {error && <p className="inline-error" role="alert">{error}</p>}
    {loading ? <p className="waiting" role="status"><LoaderCircle className="spin" /> Preparing log preview…</p>
      : url ? <label className="log-share-field">Share link<input value={url} readOnly onFocus={(event) => event.currentTarget.select()} /></label>
      : <>
        {truncated && <p className="field-hint">This preview starts with the last 512 KiB of the log.</p>}
        <label className="log-share-field">{source.name}
          <textarea rows={13} value={text} onChange={(event) => setText(event.target.value)} disabled={uploading} spellCheck={false} />
        </label>
      </>}
    <div className="dialog-actions">
      <Button variant="ghost" disabled={uploading} onClick={onClose}>{url ? "Close" : "Cancel"}</Button>
      {url ? <>
        <Button onClick={() => void runAction(() => call("open_log_share", { url }))}><ExternalLink /> Open log</Button>
        <Button variant="primary" onClick={() => void runAction(() => navigator.clipboard.writeText(url), "Log link copied.")}><Copy /> Copy link</Button>
      </> : <Button variant="primary" disabled={loading || uploading || !text.trim()} onClick={() => void upload()}>
        {uploading ? <LoaderCircle className="spin" /> : <Upload />} {uploading ? "Uploading…" : "Upload to mclo.gs"}
      </Button>}
    </div>
  </Dialog>;
}
