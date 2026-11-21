import { create } from "zustand";
import { call, errorText, isActiveJob, type Accounts, type Instance, type Job, type Overview, type Page, type Running, type Settings } from "./api";

interface Notice { message: string; tone: "neutral" | "error" | "success"; time: number }
type Destination = { page: Page; detailId: string | null };
export type LibraryFolder = { id: string; name: string };
type LibraryLayout = { folders: LibraryFolder[]; membership: Record<string, string> };

interface Launcher {
  page: Page;
  detailId: string | null;
  previous: Destination[];
  next: Destination[];
  libraryLayout: LibraryLayout;
  libraryFolder: string;
  selectedId: string | null;
  settings: Settings;
  accounts: Accounts;
  instances: Instance[];
  jobs: Job[];
  running: Running[];
  dataDir: string;
  totalMemoryMib: number;
  microsoftConfigured: boolean;
  curseForgeConfigured: boolean | null;
  testLaunchAvailable: boolean;
  recentIds: string[];
  loading: boolean;
  error: string | null;
  notice: Notice | null;
  go(page: Page): void;
  view(id: string): void;
  back(): void;
  forward(): void;
  setLibraryFolder(id: string): void;
  saveLibraryLayout(layout: LibraryLayout): void;
  moveInstance(id: string, folder: string): void;
  pick(id: string | null): void;
  load(): Promise<void>;
  refreshInstances(): Promise<void>;
  poll(): Promise<void>;
  notify(message: string, tone?: Notice["tone"]): void;
}

let nextPoll = 0;
let appliedPoll = 0;
let nextInstanceRefresh = 0;
let appliedInstanceRefresh = 0;

function navigate(state: Launcher, destination: Destination) {
  if (state.page === destination.page && state.detailId === destination.detailId) return {};
  return { ...destination, previous: [...state.previous, { page: state.page, detailId: state.detailId }].slice(-60), next: [] };
}

export const useLauncher = create<Launcher>((set, get) => ({
  page: "Home",
  detailId: null,
  previous: [],
  next: [],
  libraryLayout: readLayout(),
  libraryFolder: "",
  selectedId: null,
  settings: { ramMib: 4096, javaPath: "", downloadLimit: 8, theme: "dark", animations: true, uiScale: 115, snapshotKeep: 12, snapshotLimitMib: 8192, discordPresence: false, discordShowPack: false },
  accounts: { accounts: [], activeId: null },
  instances: [],
  jobs: [],
  running: [],
  dataDir: "",
  totalMemoryMib: 0,
  microsoftConfigured: false,
  curseForgeConfigured: null,
  testLaunchAvailable: false,
  recentIds: readRecent(),
  loading: true,
  error: null,
  notice: null,
  go: (page) => set((state) => navigate(state, { page, detailId: null })),
  view: (id) => set((state) => {
    const recentIds = [id, ...state.recentIds.filter((previous) => previous !== id)].slice(0, 12);
    try { localStorage.setItem("voltra.recent-instances", JSON.stringify(recentIds)); } catch { /* Launcher state is still usable. */ }
    return { ...navigate(state, { page: "Library", detailId: id }), selectedId: id, recentIds };
  }),
  back: () => set((state) => {
    const target = state.previous.at(-1);
    if (!target) return state;
    return { ...target, selectedId: target.detailId ?? state.selectedId, previous: state.previous.slice(0, -1), next: [{ page: state.page, detailId: state.detailId }, ...state.next] };
  }),
  forward: () => set((state) => {
    const target = state.next[0];
    if (!target) return state;
    return { ...target, selectedId: target.detailId ?? state.selectedId, previous: [...state.previous, { page: state.page, detailId: state.detailId }], next: state.next.slice(1) };
  }),
  setLibraryFolder: (libraryFolder) => set({ libraryFolder }),
  saveLibraryLayout: (libraryLayout) => {
    try { localStorage.setItem("voltra.library-layout", JSON.stringify(libraryLayout)); } catch { get().notify("Folder changes could not be saved to this device.", "error"); }
    set({ libraryLayout });
  },
  moveInstance: (id, folder) => {
    const { libraryLayout, saveLibraryLayout } = get();
    const membership = { ...libraryLayout.membership };
    if (libraryLayout.folders.some((item) => item.id === folder)) membership[id] = folder;
    else delete membership[id];
    saveLibraryLayout({ ...libraryLayout, membership });
  },
  pick: (id) => set({ selectedId: id }),
  load: async () => {
    set({ loading: true, error: null });
    try {
      const state = await call<Overview>("overview");
      set({ ...state, loading: false });
    } catch (error) {
      set({ loading: false, error: errorText(error) });
    }
  },
  refreshInstances: async () => {
    const request = ++nextInstanceRefresh;
    const instances = await call<Instance[]>("list_instances");
    if (request < appliedInstanceRefresh) return;
    appliedInstanceRefresh = request;
    set({ instances });
  },
  poll: async () => {
    const request = ++nextPoll;
    const [jobs, running] = await Promise.all([call<Job[]>("jobs"), call<Running[]>("running_games")]);
    if (request < appliedPoll) return;
    appliedPoll = request;
    const state = get();
    const previous = new Map(state.jobs.map((job) => [job.id, job.status]));
    const settled = jobs.filter((job) => !isActiveJob(job) && previous.get(job.id) !== job.status);
    const sessionsChanged = state.running.length !== running.length || running.some((game) =>
      !state.running.some((before) => before.instanceId === game.instanceId && before.pid === game.pid));
    if (sessionsChanged) {
      await state.refreshInstances();
      if (request < appliedPoll) return;
    }
    set({ jobs, running });
    if (settled.length) {
      if (!sessionsChanged) await state.refreshInstances();
      for (const job of settled) {
        if (job.status === "failed") state.notify(job.error ?? "Open Downloads for details.", "error");
        if (job.status === "done") state.notify(job.stage, "success");
      }
    }
  },
  notify: (message, tone = "neutral") => set({ notice: { message, tone, time: Date.now() } }),
}));

function readLayout(): LibraryLayout {
  try {
    const data = JSON.parse(localStorage.getItem("voltra.library-layout") ?? "null") as Partial<LibraryLayout> | null;
    const folders = Array.isArray(data?.folders) ? data.folders.filter((folder) => typeof folder?.id === "string" && typeof folder?.name === "string") : [];
    const membership = Object.fromEntries(Object.entries(data?.membership ?? {}).filter(([, id]) => folders.some((folder) => folder.id === id)));
    return { folders, membership };
  } catch {
    return { folders: [], membership: {} };
  }
}

function readRecent(): string[] {
  try {
    const data: unknown = JSON.parse(localStorage.getItem("voltra.recent-instances") ?? "[]");
    return Array.isArray(data) ? data.filter((id): id is string => typeof id === "string").slice(0, 12) : [];
  } catch {
    return [];
  }
}

export async function runAction<T>(action: () => Promise<T>, success?: string): Promise<T | undefined> {
  try {
    const value = await action();
    if (success) useLauncher.getState().notify(success, "success");
    return value;
  } catch (error) {
    useLauncher.getState().notify(errorText(error), "error");
    return undefined;
  }
}
