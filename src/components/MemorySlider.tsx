import { useId, type CSSProperties } from "react";
import { useLauncher } from "../store";

const step = 256;
const floor = 512;
const gib = (mib: number) => (mib / 1024).toLocaleString(undefined, { maximumFractionDigits: 2 }) + " GB";

export function MemorySlider({ value, onChange, label = "Memory allocation" }: {
  value: number; onChange: (value: number) => void; label?: string;
}) {
  const id = useId();
  const total = useLauncher((state) => state.totalMemoryMib);
  const max = Math.min(65536, Math.max(value, Math.floor((total || 16384) / step) * step));
  const marks = [0.25, 0.5, 0.75].map((part) => Math.max(floor, Math.round(max * part / step) * step));
  const snap = (next: number) => {
    const mark = marks.find((candidate) => Math.abs(candidate - next) <= Math.min(512, max * 0.025));
    return mark ?? next;
  };
  return <div className="memory-field">
    <div className="memory-caption"><label htmlFor={id}>{label}</label><output htmlFor={id}>{gib(value)}</output></div>
    <input id={id} type="range" min={floor} max={max} step={step} value={value}
      aria-valuetext={gib(value)} onChange={(event) => onChange(event.target.valueAsNumber)}
      onPointerUp={(event) => onChange(snap(event.currentTarget.valueAsNumber))}
      style={{ "--range-progress": ((value - floor) / Math.max(1, max - floor) * 100) + "%" } as CSSProperties} />
    <div className="memory-marks"><span>{gib(floor)}</span>{marks.map((mark, index) => (
      <button type="button" key={index} onClick={() => onChange(mark)} title={gib(mark)}>{(index + 1) * 25}%</button>
    ))}<span>{gib(max)}</span></div>
  </div>;
}
