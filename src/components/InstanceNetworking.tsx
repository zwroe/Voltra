import { useEffect, useState } from "react";
import { ArrowDownToLine, Copy } from "lucide-react";
import { call, errorText, isActiveJob } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

type Share = { code: string; jobId: string };
const shares = new Map<string, Share>();

export function InstanceShare({ id, locked }: { id: string; locked: boolean }) {
  const [address, setAddress] = useState("");
  const [consent, setConsent] = useState(false);
  const [share, setShare] = useState(() => shares.get(id));
  const [busy, setBusy] = useState(false);
  const job = useLauncher((state) => state.jobs.find((item) => item.id === share?.jobId));
  const active = !!share && (!job || isActiveJob(job));

  useEffect(() => {
    let live = true;
    void call<string | null>("local_share_address").then((ip) => { if (live && ip) setAddress((before) => before || `${ip}:24842`); }).catch(() => {});
    return () => { live = false; };
  }, []);

  async function start() {
    setBusy(true);
    await runAction(async () => {
      const link = await call<Share>("share_instance", { id, address });
      shares.set(id, link);
      setShare(link);
      await useLauncher.getState().poll();
    });
    setBusy(false);
  }

  return <div className="network-panels">
    <section className="panel detail-panel form-stack">
      <h2>Share by code</h2>
      <p className="muted">Send your installed mods, packs, scripts, and configs directly from this PC. Review your configs for private data before sharing.</p>
      <p className="field-hint">Keep Voltra open until the transfer finishes. Your friend needs a route to this IP and TCP port over a LAN, VPN, or forwarded port.</p>
      <label>Reachable IP and port<input value={address} onChange={(event) => setAddress(event.target.value)} disabled={busy || active} placeholder="192.168.1.25:24842" /></label>
      <label className="inline-checkbox"><input type="checkbox" checked={consent} disabled={active} onChange={(event) => setConsent(event.target.checked)} /> I’m ready to share these pack files with someone I trust</label>
      <div className="actions-wrap">
        <Button variant="primary" disabled={!consent || !address.trim() || locked || busy || active} onClick={() => void start()}>Create share code</Button>
        {active && <Button onClick={() => void runAction(async () => { await call("cancel_job", { id: share.jobId }); await useLauncher.getState().poll(); })}>Cancel share</Button>}
      </div>
      {share && <p role="status" className={job?.error ? "inline-error" : "field-hint"}>{job?.error || job?.stage || "Packing the instance"}</p>}
      {active && <><label>One-time share code<textarea rows={3} readOnly value={share.code} onFocus={(event) => event.currentTarget.select()} /></label>
        <Button onClick={() => void runAction(() => navigator.clipboard.writeText(share.code), "Share code copied.")}><Copy /> Copy code</Button></>}
    </section>
    <section className="panel detail-panel"><h2>Export modpack</h2><p className="muted">Save a Voltra modpack ZIP you can share or import again later.</p>
      <Button disabled={locked} onClick={() => void runAction(async () => {
        if (await call<boolean>("export_pack", { id })) useLauncher.getState().notify("Modpack exported.", "success");
      })}><ArrowDownToLine /> Export modpack</Button>
    </section>
  </div>;
}

export function ReceiveShareDialog({ onClose }: { onClose: () => void }) {
  const [code, setCode] = useState("");
  const [trusted, setTrusted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function receive() {
    setBusy(true);
    setError("");
    try {
      await call("receive_instance", { code });
      useLauncher.getState().go("Downloads");
      onClose();
      await useLauncher.getState().poll();
    } catch (failure) {
      setError(errorText(failure));
    } finally { setBusy(false); }
  }

  return <Dialog open title="Receive an instance" description="Ask the host to keep Voltra open while the pack transfers. Imported mods and scripts can run code when you play."
    onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
    <form className="form-stack" onSubmit={(event) => { event.preventDefault(); void receive(); }}>
      <label>Share code<textarea autoFocus required maxLength={512} rows={3} value={code} onChange={(event) => setCode(event.target.value)} /></label>
      <label className="inline-checkbox"><input type="checkbox" checked={trusted} onChange={(event) => setTrusted(event.target.checked)} /> I trust the person who shared this instance</label>
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="dialog-actions"><Button onClick={onClose} variant="ghost" disabled={busy}>Cancel</Button><Button type="submit" variant="primary" disabled={!trusted || !code.trim() || busy}>Receive instance</Button></div>
    </form>
  </Dialog>;
}
