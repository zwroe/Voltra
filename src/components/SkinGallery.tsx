import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { Check, LoaderCircle, Plus, RotateCcw, Shirt, X } from "lucide-react";
import { call, errorText, type Account, type Accounts, type Cape, type Skin, type SkinVariant } from "../api";
import { runAction, useLauncher } from "../store";
import { Button } from "./ui/button";

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

function SkinImage({ skin, large = false, angle = -23, capeUrl }: { skin: Skin; large?: boolean; angle?: number; capeUrl?: string }) {
  const [url, setUrl] = useState(skin.builtin ? "/skins/" + skin.id + ".png" : "");
  useEffect(() => {
    if (skin.builtin) {
      setUrl("/skins/" + skin.id + ".png");
      return;
    }
    let alive = true;
    let objectUrl = "";
    setUrl("");
    call<ArrayBuffer | number[]>("read_skin", { id: skin.id }).then((bytes) => {
      if (!alive) return;
      objectUrl = URL.createObjectURL(new Blob([Array.isArray(bytes) ? Uint8Array.from(bytes) : bytes], { type: "image/png" }));
      setUrl(objectUrl);
    }).catch(() => {});
    return () => { alive = false; if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [skin.id, skin.builtin]);
  if (!url) return <span className="empty-skin"><Shirt /></span>;
  const unit = large ? 7.1 : 3.1;
  const slim = skin.variant === "slim" && skin.height === 64;
  const arm = slim ? 3 : 4;
  const rightArm = 4 - arm;
  const legacy = skin.height === 32;
  const blocks = [
    { x: 4, y: 0, w: 8, h: 8, d: 8, u: 0, v: 0, layer: [32, 0] },
    { x: 4, y: 8, w: 8, h: 12, d: 4, u: 16, v: 16, layer: [16, 32] },
    { x: rightArm, y: 8, w: arm, h: 12, d: 4, u: 40, v: 16, layer: [40, 32] },
    { x: 12, y: 8, w: arm, h: 12, d: 4, u: legacy ? 40 : 32, v: legacy ? 16 : 48, layer: [48, 48] },
    { x: 4, y: 20, w: 4, h: 12, d: 4, u: 0, v: 16, layer: [0, 32] },
    { x: 8, y: 20, w: 4, h: 12, d: 4, u: legacy ? 0 : 16, v: legacy ? 16 : 48, layer: [0, 48] },
  ];
  return <div className={"skin-model" + (large ? " skin-model-idle" : "")} aria-label={skin.name + " 3D preview"} role="img"
    style={{ width: 16 * unit, height: 32 * unit, transform: "rotateX(-7deg) rotateY(" + angle + "deg)" }}>
    {blocks.map((block, index) => (
      <div key={index} className={"skin-box " + (["skin-head", "", "skin-arm skin-arm-right", "skin-arm skin-arm-left"][index] ?? "")}
        style={{ left: block.x * unit, top: block.y * unit, width: block.w * unit, height: block.h * unit }}>
        {faces(block.w, block.h, block.d, block.u, block.v, 1)}
        {(!legacy || index === 0) && faces(block.w, block.h, block.d, block.layer[0], block.layer[1], 1.08)}
      </div>
    ))}
    {capeUrl && <span className="skin-cape-plane" style={{ left: 3 * unit, top: 8 * unit, width: 10 * unit, height: 16 * unit,
      transform: "translateZ(" + (-2.35 * unit) + "px) rotateX(13deg)" }}><CapeTexture url={capeUrl} /></span>}
  </div>;

  function faces(w: number, h: number, d: number, u: number, v: number, spread: number) {
    const textures = [
      [w, h, u + d, v + d, "rotateY(0deg)", d, 1],
      [w, h, u + w + 2 * d, v + d, "rotateY(180deg)", d, 0.82],
      [d, h, u, v + d, "rotateY(90deg)", w, 0.76],
      [d, h, u + w + d, v + d, "rotateY(-90deg)", w, 0.86],
      [w, d, u + d, v, "rotateX(90deg)", h, 1.08],
      [w, d, u + d + w, v, "rotateX(-90deg)", h, 0.65],
    ] as const;
    const pixel = unit * spread;
    return textures.map(([width, height, x, y, rotation, depth, shade], face) => {
      const style: CSSProperties = {
        width: width * pixel, height: height * pixel,
        transform: "translate(-50%, -50%) " + rotation + " translateZ(" + depth * pixel / 2 + "px)",
        backgroundImage: "url(" + url + ")",
        backgroundSize: 64 * pixel + "px " + skin.height * pixel + "px",
        backgroundPosition: "-" + x * pixel + "px -" + y * pixel + "px",
        filter: "brightness(" + shade + ")",
      };
      return <i key={spread + ":" + face} className="skin-face-plane" style={style} />;
    });
  }
}

export function SkinGallery({ onSignIn }: { onSignIn: () => void }) {
  const accounts = useLauncher((state) => state.accounts);
  const user = accounts.accounts.find((account) => account.id === accounts.activeId);
  const [skins, setSkins] = useState<Skin[]>([]);
  const [selected, choose] = useState(user?.skinChoice ?? "rift");
  const [angle, setAngle] = useState(-23);
  const [variant, setVariant] = useState<SkinVariant>("classic");
  const [capePreview, previewCape] = useState<{ accountId: string; url?: string } | null>(null);
  const accountId = user?.id;
  const showCape = useCallback((url?: string) => {
    if (accountId) previewCape({ accountId, url });
  }, [accountId]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const drag = useRef<{ x: number; angle: number } | null>(null);
  const active = skins.find((skin) => skin.id === selected) ?? skins[0];

  useEffect(() => {
    let alive = true;
    call<Skin[]>("list_skins").then((items) => { if (alive) setSkins(items); }).catch((reason) => {
      if (alive) setError(errorText(reason));
    });
    return () => { alive = false; };
  }, []);

  function add() {
    setBusy(true);
    void runAction(async () => {
      const skin = await call<Skin | null>("import_skin", { variant });
      if (skin) { setSkins((previous) => [skin, ...previous]); choose(skin.id); }
    }).finally(() => setBusy(false));
  }

  function apply() {
    if (!user) { onSignIn(); return; }
    if (!active) return;
    setBusy(true);
    void runAction(async () => {
      const result = await call<Accounts>("apply_skin", { id: active.id });
      useLauncher.setState({ accounts: result });
    }, "Minecraft skin updated.").finally(() => setBusy(false));
  }

  const saved = skins.filter((skin) => !skin.builtin);
  const presets = skins.filter((skin) => skin.builtin);
  const card = (skin: Skin) => <button className="skin-card" key={skin.id} aria-pressed={active?.id === skin.id} onClick={() => choose(skin.id)}>
    {user?.skinChoice === skin.id && <span className="skin-equipped" title="Active Minecraft skin"><Check size={14} /></span>}
    <SkinImage skin={skin} /><strong>{skin.name}</strong><span>{skin.variant === "slim" ? "Slim" : "Classic"}</span>
  </button>;

  return <>
    <div className="page-heading"><div><h1>Skins</h1><p>Preview and change your Minecraft Java Edition skin.</p></div></div>
    {error && <p className="inline-error" role="alert">{error}</p>}
    <div className="skin-layout">
      <section className="skin-stage">
        <div className="skin-stage-heading"><span>{user?.name ?? "Skin preview"}</span>
          <Button size="icon" variant="ghost" title="Reset rotation" aria-label="Reset rotation" onClick={() => setAngle(-23)}><RotateCcw size={16} /></Button>
        </div>
        <div className="skin-turntable" tabIndex={0} aria-label="Skin preview. Use left and right arrow keys to rotate."
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
              event.preventDefault(); setAngle((old) => old + (event.key === "ArrowLeft" ? -15 : 15));
            }
          }}
          onPointerDown={(event) => { drag.current = { x: event.clientX, angle }; event.currentTarget.setPointerCapture(event.pointerId); }}
          onPointerMove={(event) => { if (drag.current) setAngle(drag.current.angle + (event.clientX - drag.current.x) * 1.2); }}
          onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }}>
          {active && <SkinImage key={active.id} skin={active} large angle={angle}
            capeUrl={capePreview?.accountId === user?.id ? capePreview?.url : undefined} />}
        </div>
        <div className="skin-stage-footer"><strong>{active?.name ?? "Loading skins…"}</strong><p className="muted">Drag to rotate</p>
          <Button variant="primary" onClick={apply} disabled={!active || busy || user?.skinChoice === active?.id}>
            {user?.skinChoice === active?.id && active ? "Equipped" : user ? "Apply to Minecraft" : "Sign in to apply"}
          </Button>
        </div>
      </section>
      <div className="skin-collections">
        {user ? <CapePicker key={user.id} accountId={user.id} onPreview={showCape} />
          : <section className="cape-panel"><header className="section-heading"><h2>Capes</h2></header>
            <div className="cape-signin"><span className="muted">Connect a Minecraft account to choose one of your capes.</span>
              <Button size="small" onClick={onSignIn}>Connect account</Button></div></section>}
        <section><header className="section-heading"><h2>Saved skins</h2>
          <div className="actions-wrap"><select aria-label="Imported skin arm style" value={variant} onChange={(event) => setVariant(event.target.value as SkinVariant)}>
            <option value="classic">Classic arms</option><option value="slim">Slim arms</option>
          </select><Button disabled={busy} onClick={add}><Plus /> Import PNG</Button></div>
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
