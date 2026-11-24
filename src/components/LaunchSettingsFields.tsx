import { Select } from "./ui/select";
import { useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";
import type { EnvironmentVariable, GameWindow, LaunchHooks } from "../api";
import { Button } from "./ui/button";

export const argumentsFrom = (text: string) => text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
export const environmentText = (values?: EnvironmentVariable[] | null) => values?.map(({ name, value }) => name + "=" + value).join("\n") ?? "";

export function environmentFrom(text: string) {
  return text.split(/\r?\n/).filter((line) => line.trim()).map((line) => {
    const equals = line.indexOf("=");
    if (equals < 1) throw new Error("Enter each environment variable as NAME=value.");
    return { name: line.slice(0, equals).trim(), value: line.slice(equals + 1) };
  });
}

export function ResetField({ label, disabled, onReset }: { label: string; disabled?: boolean; onReset: () => void }) {
  return <Button size="icon" variant="ghost" title={"Reset " + label} aria-label={"Reset " + label} disabled={disabled} onClick={onReset}>
    <RotateCcw size={15} />
  </Button>;
}

export function WindowFields({ value, defaults, onChange }: { value: GameWindow; defaults?: GameWindow; onChange: (value: GameWindow) => void }) {
  return <div className="launch-field-stack">
    <div className="launch-field">
      <div><label htmlFor="game-fullscreen">Fullscreen at launch</label><p className="field-hint">Controls the fullscreen option when Minecraft starts.</p></div>
      <Select id="game-fullscreen" value={value.fullscreen == null ? "" : String(value.fullscreen)}
        onValueChange={(nextValue) => onChange({ ...value, fullscreen: nextValue === "" ? null : nextValue === "true" })}>
        <option value="">{defaults ? (defaults.fullscreen == null ? "Use launcher default" : "Default (" + (defaults.fullscreen ? "on" : "off") + ")") : "Use Minecraft setting"}</option>
        <option value="true">On</option><option value="false">Off</option>
      </Select>
    </div>
    {(["width", "height"] as const).map((field) => <div className="launch-field" key={field}>
      <div><label htmlFor={"game-" + field}>{field === "width" ? "Window width" : "Window height"}</label><p className="field-hint">Pixels</p></div>
      <div className="launch-control"><input id={"game-" + field} type="number" min={100} max={16384} step={1} value={value[field] ?? ""}
        placeholder={String(defaults?.[field] ?? (field === "width" ? 854 : 480))}
        onChange={(event) => onChange({ ...value, [field]: event.target.value === "" ? null : event.target.valueAsNumber })} />
        <ResetField label={"window " + field} disabled={value[field] == null} onReset={() => onChange({ ...value, [field]: null })} /></div>
    </div>)}
  </div>;
}

export function HookFields({ value, instance, onChange }: { value: LaunchHooks; instance?: boolean; onChange: (value: LaunchHooks) => void }) {
  const [wrapperText, setWrapperText] = useState(() => value.wrapperArgs?.join("\n") ?? "");
  useEffect(() => {
    if (value.wrapperArgs == null) setWrapperText("");
  }, [value.wrapperArgs]);
  return <div className="launch-field-stack">
    {([
      ["preLaunch", "Pre-launch command", "Runs in the instance’s game folder before Minecraft starts."],
      ["wrapper", "Wrapper executable", "Full path to an executable that receives Java and its arguments."],
      ["postExit", "Post-exit command", "Runs in the game folder after Minecraft closes."],
    ] as const).map(([field, title, hint]) => <div className="launch-field-block" key={field}>
      <div className="launch-control"><label htmlFor={"hook-" + field}>{title}</label>
        <ResetField label={title.toLowerCase()} disabled={value[field] == null} onReset={() => onChange({ ...value, [field]: null })} /></div>
      <input id={"hook-" + field} value={value[field] ?? ""} maxLength={4096} spellCheck={false} autoComplete="off"
        placeholder={instance && value[field] == null ? "Use launcher default" : field === "wrapper" ? "Path to executable" : "Enter a shell command"}
        onChange={(event) => onChange({ ...value, [field]: event.target.value })} />
      <p className="field-hint">{hint}</p>
      {field === "wrapper" && <>
        <div className="launch-control"><label htmlFor="wrapper-arguments">Wrapper arguments</label>
          <ResetField label="wrapper arguments" disabled={value.wrapperArgs == null} onReset={() => onChange({ ...value, wrapperArgs: null })} /></div>
        <textarea id="wrapper-arguments" rows={2} spellCheck={false}
          placeholder={instance && value.wrapperArgs == null ? "Use launcher default" : "One argument per line"}
          value={wrapperText} onChange={(event) => {
            setWrapperText(event.target.value);
            onChange({ ...value, wrapperArgs: argumentsFrom(event.target.value) });
          }} />
      </>}
    </div>)}
    <p className="field-hint">Hooks can read INST_NAME, INST_ID, INST_DIR, INST_MC_DIR, INST_JAVA, and INST_JAVA_ARGS from their environment.</p>
  </div>;
}
