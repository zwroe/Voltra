import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ChevronLeft, ChevronRight, Download, Minus, Square, X } from "lucide-react";
import logo from "../../src-tauri/icons/icon.png";
import { isActiveJob } from "../api";
import { runAction, useLauncher } from "../store";

export function WindowBar() {
  const page = useLauncher((state) => state.page);
  const go = useLauncher((state) => state.go);
  const count = useLauncher((state) => state.jobs.filter(isActiveJob).length);
  const instance = useLauncher((state) => state.instances.find((item) => item.id === state.detailId));
  const previous = useLauncher((state) => state.previous.length > 0);
  const next = useLauncher((state) => state.next.length > 0);
  const back = useLauncher((state) => state.back);
  const forward = useLauncher((state) => state.forward);
  function windowAction(action: "minimize" | "toggleMaximize" | "close") {
    if (isTauri()) void runAction(() => getCurrentWindow()[action]());
  }

  return <header className="window-bar">
    <nav className="window-history" aria-label="Page history">
      <button disabled={!previous} onClick={back} title="Back" aria-label="Back"><ChevronLeft size={18} /></button>
      <button disabled={!next} onClick={forward} title="Forward" aria-label="Forward"><ChevronRight size={18} /></button>
    </nav>
    <div className="window-drag" data-tauri-drag-region>
      <img src={logo} alt="" width={19} height={19} data-tauri-drag-region />
      <strong data-tauri-drag-region>Voltra</strong>
      <span className="window-divider" data-tauri-drag-region />
      <span className="muted" data-tauri-drag-region>{instance?.name ?? page}</span>
    </div>
    <button className="window-job" onClick={() => go("Downloads")} aria-label="Downloads" title="Downloads">
      <Download size={15} />{count > 0 && <span>{count}</span>}
    </button>
    <div className="window-controls">
      <button type="button" onClick={() => windowAction("minimize")} title="Minimize" aria-label="Minimize"><Minus size={16} /></button>
      <button type="button" onClick={() => windowAction("toggleMaximize")} title="Maximize or restore" aria-label="Maximize or restore"><Square size={12} /></button>
      <button type="button" className="window-close" onClick={() => windowAction("close")} title="Close" aria-label="Close"><X size={17} /></button>
    </div>
  </header>;
}
