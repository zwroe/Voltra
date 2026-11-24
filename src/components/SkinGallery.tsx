import { Select } from "./ui/select";
import { useCallback, useEffect, useRef, useState } from "react";
import { Check, LoaderCircle, Plus, RotateCcw, Shirt, X } from "lucide-react";
import { call, errorText, type Account, type Accounts, type Cape, type Skin, type SkinVariant } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";
import { prepareSkinTexture } from "./skinTexture";
import { drawSkin, loadCapeTexture } from "./skinRenderer";

export function AccountAvatar({ account }: { account?: Account }) {
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [account?.skinUrl]);
  return <span className="account-avatar pixel-avatar">
    {account?.skinUrl && !failed ? <>
      <img className="skin-face" src={account.skinUrl} alt="" onError={() => setFailed(true)} />
      <img className="skin-face overlay" src={account.skinUrl} alt="" />
    </> : <img className="voltra-avatar" src="/voltra-avatar.png" alt="" />}
  </span>;
}

function SkinImage({ skin, source, large = false, angle = -23, capeUrl, dragging = false }: {
  skin: Skin; source?: string; large?: boolean; angle?: number; capeUrl?: string; dragging?: boolean;
}) {
  const animations = useLauncher((state) => state.settings.animations);
  const [texture, setTexture] = useState<ImageData>();
  const [cape, setCape] = useState<{ url: string; texture: ImageData }>();
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    let alive = true;
    setTexture(undefined);
    async function load() {
      if (source || skin.builtin) return prepareSkinTexture(source ?? "/skins/" + skin.id + ".png");
      const bytes = await call<ArrayBuffer | number[]>("read_skin", { id: skin.id });
      const objectUrl = URL.createObjectURL(new Blob([Array.isArray(bytes) ? Uint8Array.from(bytes) : bytes], { type: "image/png" }));
      try { return await prepareSkinTexture(objectUrl); } finally { URL.revokeObjectURL(objectUrl); }
    }
    void load().then((image) => { if (alive) setTexture(image); }).catch(() => {});
    return () => { alive = false; };
  }, [skin.id, skin.builtin, source]);
  useEffect(() => {
    if (!capeUrl) return;
    let live = true;
    void loadCapeTexture(capeUrl).then((texture) => { if (live) setCape({ url: capeUrl, texture }); }).catch(() => {});
    return () => { live = false; };
  }, [capeUrl]);
  useEffect(() => {
    const target = canvas.current;
    if (!target || !texture) return;
    const skinTexture = texture;
    const capeTexture = cape?.url === capeUrl ? cape?.texture : undefined;
    const slim = skin.variant === "slim" && skin.height === 64;
    const legacy = skin.height === 32;
    drawSkin(target, skinTexture, angle, slim, legacy, capeTexture);
    if (!large || !animations || dragging) return;

    const preference = matchMedia("(prefers-reduced-motion: reduce)");
    let frame = 0;
    let lastDraw = -Infinity;
    const start = performance.now();
    function animate(now: number) {
      if (now - lastDraw >= 32) {
        drawSkin(target!, skinTexture, angle, slim, legacy, capeTexture, now - start);
        lastDraw = now;
      }
      frame = requestAnimationFrame(animate);
    }
    function syncMotion() {
      cancelAnimationFrame(frame);
      if (preference.matches) drawSkin(target!, skinTexture, angle, slim, legacy, capeTexture);
      else frame = requestAnimationFrame(animate);
    }
    syncMotion();
    preference.addEventListener("change", syncMotion);
    return () => {
      cancelAnimationFrame(frame);
      preference.removeEventListener("change", syncMotion);
    };
  }, [texture, angle, skin.variant, skin.height, cape, capeUrl, large, animations, dragging]);

  return texture ? <canvas ref={canvas} className="skin-model" width={large ? 156 : 64} height={large ? 256 : 110}
    aria-label={skin.name + " 3D preview"} role="img" /> : <span className="empty-skin"><Shirt /></span>;
}

type AccountSkin = { png: string; height: number; variant: SkinVariant };

export function SkinGallery({ onSignIn }: { onSignIn: () => void }) {
  const accounts = useLauncher((state) => state.accounts);
  const user = accounts.accounts.find((account) => account.id === accounts.activeId);
  const [skins, setSkins] = useState<Skin[]>([]);
  const [selection, choose] = useState<{ accountId?: string; id: string }>();
  const [angle, setAngle] = useState(-23);
  const [dragging, setDragging] = useState(false);
  const [current, setCurrent] = useState<{ accountId: string; skin?: AccountSkin; error?: string }>();
  const [capePreview, previewCape] = useState<{ accountId: string; url?: string } | null>(null);
  const accountId = user?.id;
  const showCape = useCallback((url?: string) => {
    if (accountId) previewCape({ accountId, url });
  }, [accountId]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const drag = useRef<{ x: number; angle: number } | null>(null);
  function endDrag() {
    drag.current = null;
    setDragging(false);
  }
  const selected = selection?.accountId === accountId ? selection?.id : undefined;
  const showingCurrent = !!user && (!selected || selected === "current");
  const accountSkin = current?.accountId === accountId ? current : undefined;
  const active = showingCurrent
    ? accountSkin?.skin && { ...accountSkin.skin, id: "current", name: "Current Minecraft skin", builtin: false }
    : skins.find((skin) => skin.id === (selected ?? "rift")) ?? skins[0];
  const equipped = !!active && user?.skinChoice === active.id && user.skinVariant === active.variant;
  const selectSkin = (id: string) => choose({ accountId, id });

  useEffect(() => {
    let alive = true;
    call<Skin[]>("list_skins").then((items) => { if (alive) setSkins(items); }).catch((reason) => {
      if (alive) setError(errorText(reason));
    });
    return () => { alive = false; };
  }, []);

  useEffect(() => {
    if (!accountId) return;
    let alive = true;
    call<AccountSkin | null>("current_account_skin", { accountId }).then((skin) => {
      if (alive) setCurrent({ accountId, skin: skin ?? undefined, error: skin ? undefined : "This profile has no custom skin." });
    }).catch((reason) => {
      if (alive) setCurrent({ accountId, error: errorText(reason) });
    });
    return () => { alive = false; };
  }, [accountId, user?.skinUrl, user?.skinVariant]);

  function add() {
    setBusy(true);
    void runAction(async () => {
      const skin = await call<Skin | null>("import_skin", { variant: "classic" });
      if (skin) { setSkins((previous) => [skin, ...previous]); selectSkin(skin.id); }
    }).finally(() => setBusy(false));
  }

  function changeVariant(skin: Skin, variant: SkinVariant) {
    setBusy(true);
    void runAction(async () => {
      const updated = await call<Skin>("set_skin_variant", { id: skin.id, variant });
      setSkins((previous) => previous.map((item) => item.id === updated.id ? updated : item));
    }).finally(() => setBusy(false));
  }

  function apply() {
    if (!user) { onSignIn(); return; }
    if (!active || showingCurrent) return;
    setBusy(true);
    void runAction(async () => {
      const result = await call<Accounts>("apply_skin", { id: active.id });
      useLauncher.setState({ accounts: result });
    }, "Minecraft skin updated.").finally(() => setBusy(false));
  }

  const saved = skins.filter((skin) => !skin.builtin);
  const presets = skins.filter((skin) => skin.builtin);
  const card = (skin: Skin) => <button className="skin-card" key={skin.id} aria-pressed={active?.id === skin.id} onClick={() => selectSkin(skin.id)}>
    {user?.skinChoice === skin.id && user.skinVariant === skin.variant && <span className="skin-equipped" title="Active Minecraft skin"><Check size={14} /></span>}
    <SkinImage skin={skin} /><strong>{skin.name}</strong><span>{skin.variant === "slim" ? "Slim" : "Classic"}</span>
  </button>;

  return <>
    <div className="page-heading"><div><h1>Skins</h1><p>Preview and change your Minecraft Java Edition skin.</p></div></div>
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="skin-layout">
      <section className="skin-stage">
        <div className="skin-stage-heading"><span>{user?.name ?? "Skin preview"}</span>
          <div className="actions-wrap">{user && !showingCurrent && <Button size="small" variant="ghost" onClick={() => selectSkin("current")}>Current skin</Button>}
          <Button size="icon" variant="ghost" title="Reset rotation" aria-label="Reset rotation" onClick={() => setAngle(-23)}><RotateCcw size={16} /></Button></div>
        </div>
        <div className="skin-turntable" tabIndex={0} aria-label="Skin preview. Use left and right arrow keys to rotate."
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
              event.preventDefault(); setAngle((old) => old + (event.key === "ArrowLeft" ? -15 : 15));
            }
          }}
          onPointerDown={(event) => {
            drag.current = { x: event.clientX, angle };
            setDragging(true);
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => { if (drag.current) setAngle(drag.current.angle + (event.clientX - drag.current.x) * 1.2); }}
          onLostPointerCapture={endDrag} onPointerUp={endDrag} onPointerCancel={endDrag}>
          {active && <SkinImage key={active.id} skin={active} large angle={angle} dragging={dragging}
            source={showingCurrent && accountSkin?.skin ? "data:image/png;base64," + accountSkin.skin.png : undefined}
            capeUrl={capePreview?.accountId === user?.id ? capePreview?.url : undefined} />}
          {showingCurrent && !active && <p className="muted">{accountSkin?.error ?? "Loading current skin…"}</p>}
        </div>
        <div className="skin-stage-footer">{active && <><strong>{active.name}</strong><p className="muted">Drag to rotate</p></>}
          {active && !showingCurrent && !active.builtin && active.height === 64 &&
            <Select aria-label="Skin arm style" value={active.variant} disabled={busy} onValueChange={(value) => changeVariant(active, value as SkinVariant)}>
              <option value="classic">Classic arms</option><option value="slim">Slim arms</option>
            </Select>}
          {!showingCurrent && <Button variant="primary" onClick={apply} disabled={!active || busy || equipped}>
            {equipped ? "Equipped" : user ? "Apply to Minecraft" : "Sign in to apply"}
          </Button>}
        </div>
      </section>
      <div className="skin-collections">
        {user ? <CapePicker key={user.id} accountId={user.id} onPreview={showCape} />
          : <section className="cape-panel"><header className="section-heading"><h2>Capes</h2></header>
            <div className="cape-signin"><span className="muted">Connect a Minecraft account to choose one of your capes.</span>
              <Button size="small" onClick={onSignIn}>Connect account</Button></div></section>}
        <section><header className="section-heading"><h2>Saved skins</h2>
          <Button disabled={busy} onClick={add}><Plus /> Import PNG</Button>
        </header>
          {saved.length ? <div className="skin-cards">{saved.map(card)}</div> : <p className="skin-empty muted">Import a 64×64 or 64×32 Minecraft skin to add it here.</p>}
        </section>
        <section><header className="section-heading"><h2>Voltra presets</h2></header><div className="skin-cards">{presets.map(card)}</div></section>
      </div>
    </div>
  </>;
}

function CapeTexture({ url }: { url: string }) {
  return <span className="cape-texture"><img src={url} alt="" draggable={false} /></span>;
}

function CapePicker({ accountId, onPreview }: { accountId: string; onPreview: (url?: string) => void }) {
  const [capes, setCapes] = useState<Cape[] | null>(null);
  const [selected, select] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [attempt, retry] = useState(0);
  const [busy, setBusy] = useState(false);
  const active = capes?.find((cape) => cape.active)?.id ?? null;

  useEffect(() => {
    let live = true;
    setError("");
    call<Cape[]>("list_capes", { accountId }).then((items) => {
      if (!live) return;
      const equipped = items.find((item) => item.active);
      setCapes(items);
      select(equipped?.id ?? null);
      onPreview(equipped?.url);
    }).catch((reason) => { if (live) setError(errorText(reason)); });
    return () => { live = false; };
  }, [accountId, attempt, onPreview]);

  function apply() {
    setBusy(true);
    void runAction(async () => {
      const next = await call<Cape[]>("set_cape", { accountId, capeId: selected });
      const equipped = next.find((item) => item.active);
      setCapes(next);
      select(equipped?.id ?? null);
      onPreview(equipped?.url);
    }, selected ? "Minecraft cape updated." : "Minecraft cape hidden.").finally(() => setBusy(false));
  }

  function pick(cape: Cape | null) {
    select(cape?.id ?? null);
    onPreview(cape?.url);
  }

  return <section className="cape-panel">
    <header className="section-heading"><h2>Capes</h2>
      {capes && capes.length > 0 && <Button size="small" variant="primary" disabled={busy || selected === active}
        onClick={apply}>{busy ? "Saving…" : selected ? "Wear cape" : "Hide cape"}</Button>}
    </header>
    {error ? <div className="cape-signin"><p className="muted" role="alert">{error}</p>
      <Button size="small" onClick={() => retry((value) => value + 1)}>Retry</Button></div>
      : capes === null ? <p className="waiting"><LoaderCircle className="spin" /> Loading account capes.</p>
      : capes.length === 0 ? <p className="muted">This Minecraft account has no capes.</p>
      : <div className="cape-cards">
        <button type="button" className="cape-card" aria-pressed={!selected} disabled={busy} onClick={() => pick(null)}>
          <span className="cape-none"><X size={20} /></span><span>None</span>
        </button>
        {capes.map((cape) => <button type="button" className="cape-card" aria-label={cape.name + (cape.active ? ", currently worn" : "")}
          aria-pressed={selected === cape.id} disabled={busy} key={cape.id} onClick={() => pick(cape)}>
          <CapeTexture url={cape.url} /><span title={cape.name}>{cape.name}</span>
          {cape.active && <span className="cape-equipped" title="Currently worn"><Check size={12} /></span>}
        </button>)}
      </div>}
  </section>;
}
