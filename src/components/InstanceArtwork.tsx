import { useRef, type ChangeEvent } from "react";
import { Box, ImagePlus, RotateCcw } from "lucide-react";
import { errorText, type Instance } from "../api";
import { Button } from "./ui/button";

export function InstanceIcon({ loader, icon, size = 22 }: Pick<Instance, "loader" | "icon"> & { size?: number }) {
  return <span className={"instance-icon " + loader} aria-hidden="true">
    {icon ? <img src={"data:image/png;base64," + icon} alt="" /> : <Box size={size} strokeWidth={1.65} />}
  </span>;
}

export function InstanceArtwork({ loader, icon, change, busy, setBusy, onError }: Pick<Instance, "loader" | "icon"> & {
  change: (icon: string | null) => void;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  onError: (message: string) => void;
}) {
  const picker = useRef<HTMLInputElement>(null);
  async function choose(event: ChangeEvent<HTMLInputElement>) {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (!file) return;
    onError("");
    setBusy(true);
    try {
      if (!/^image\/(png|jpeg|webp)$/.test(file.type) || file.size > 12 * 1024 * 1024)
        throw new Error("Choose a PNG, JPEG, or WebP image up to 12 MB.");
      const source = await createImageBitmap(file);
      try {
        const canvas = document.createElement("canvas");
        canvas.width = canvas.height = 192;
        const context = canvas.getContext("2d");
        if (!context) throw new Error("The icon preview could not be created.");
        const side = Math.min(source.width, source.height);
        context.drawImage(source, (source.width - side) / 2, (source.height - side) / 2, side, side, 0, 0, 192, 192);
        change(canvas.toDataURL("image/png").split(",")[1]);
      } finally {
        source.close();
      }
    } catch (error) {
      onError(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  return <div className="instance-artwork">
    <input ref={picker} type="file" accept="image/png,image/jpeg,image/webp" aria-label="Instance image file" hidden onChange={(event) => void choose(event)} />
    <button type="button" className="instance-artwork-preview" disabled={busy} aria-label="Choose instance icon" onClick={() => picker.current?.click()}>
      <InstanceIcon loader={loader} icon={icon} size={32} />
    </button>
    <div className="instance-artwork-controls">
      <Button size="small" disabled={busy} onClick={() => picker.current?.click()}><ImagePlus /> {icon ? "Change icon" : "Choose icon"}</Button>
      {icon && <Button size="small" variant="ghost" disabled={busy} onClick={() => change(null)}><RotateCcw /> Reset</Button>}
      <p className="field-hint">PNG, JPEG, or WebP</p>
    </div>
  </div>;
}
