import { useEffect, useState } from "react";
import { ExternalLink, LoaderCircle } from "lucide-react";
import { call, errorText, type Accounts, type BrowserSignIn } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export function SignIn({ onClose }: { onClose: () => void }) {
  const [pendingId, setPendingId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [completed, setCompleted] = useState(false);
  const configured = useLauncher((state) => state.microsoftConfigured);

  useEffect(() => {
    if (!configured) return;
    let live = true;
    let id: string | undefined;
    setError("");
    setPendingId(null);
    call<BrowserSignIn>("begin_sign_in").then(async (session) => {
      id = session.id;
      if (!live) {
        await call("cancel_sign_in", { id });
        return;
      }
      setPendingId(id);
      await call("open_sign_in", { id });
      const accounts = await call<Accounts>("complete_sign_in", { id });
      if (live) {
        useLauncher.setState({ accounts });
        useLauncher.getState().notify("Microsoft account connected.", "success");
        setCompleted(true);
      }
    }).catch((reason) => {
      if (id) void call("cancel_sign_in", { id }).catch(() => {});
      if (live) setError(errorText(reason));
    });
    return () => {
      live = false;
      if (id) void call("cancel_sign_in", { id }).catch(() => {});
    };
  }, [attempt, configured]);

  return <Dialog open title="Connect Microsoft account"
    description="Sign in with the Microsoft account that owns Minecraft Java Edition."
    onOpenChange={(open) => { if (!open) onClose(); }}>
    {!configured ? <div className="form-stack">
      <p className="muted">This build needs an approved Voltra registration before it can sign in.</p>
      <div className="dialog-actions"><Button variant="primary" onClick={onClose}>Close</Button></div>
    </div>
      : completed ? <div className="dialog-actions"><Button variant="primary" onClick={onClose}>Done</Button></div>
      : error ? <div className="form-stack">
        <p className="inline-error" role="alert">{error}</p>
        <div className="dialog-actions">
          <Button variant="primary" onClick={() => setAttempt((value) => value + 1)}>Try again</Button>
        </div>
      </div> : <div className="form-stack">
        <p className="waiting"><LoaderCircle className="spin" />{pendingId ? "Finish signing in in your browser." : "Opening Microsoft sign-in…"}</p>
        {pendingId && <div className="dialog-actions">
          <Button onClick={() => void runAction(() => call("open_sign_in", { id: pendingId }))}><ExternalLink /> Reopen browser</Button>
        </div>}
      </div>}
  </Dialog>;
}
