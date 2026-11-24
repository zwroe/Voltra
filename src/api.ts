import { invoke, isTauri } from "@tauri-apps/api/core";

export type Loader = "vanilla" | "fabric" | "quilt" | "forge" | "neoforge";
export type Platform = "modrinth" | "curseforge";
export type Theme = "dark" | "light" | "system";
export type Page = "Home" | "Library" | "Discover" | "Skins" | "Screenshots" | "Downloads" | "Diagnostics" | "Settings";
export type ProjectKind = "mod" | "modpack" | "resourcepack" | "shader" | "datapack";
export type ProjectSort = "relevance" | "downloads" | "updated" | "newest";
export type UpdateChannel = "release" | "beta" | "alpha";
export type SyncItem = "options" | "servers" | "resourcePacks" | "commandHistory" | "hotbars";
export type EnvironmentVariable = { name: string; value: string };
export interface GameWindow {
  fullscreen?: boolean | null;
  width?: number | null;
  height?: number | null;
}
export interface LaunchHooks {
  preLaunch?: string | null;
  wrapper?: string | null;
  wrapperArgs?: string[] | null;
  postExit?: string | null;
}
export interface LaunchOverrides {
  inheritMemory?: boolean;
  javaPath?: string | null;
  environment?: EnvironmentVariable[] | null;
  window?: GameWindow;
  hooks?: LaunchHooks;
}

export const loaderNames: Record<Loader, string> = { vanilla: "Vanilla", fabric: "Fabric", quilt: "Quilt", forge: "Forge", neoforge: "NeoForge" };
export const platformNames: Record<Platform, string> = { modrinth: "Modrinth", curseforge: "CurseForge" };

export interface Settings {
  ramMib: number;
  javaPath: string;
  downloadLimit: number;
  theme: Theme;
  animations: boolean;
  uiScale: number;
  snapshotKeep: number;
  snapshotLimitMib: number;
  discordPresence: boolean;
  discordShowPack: boolean;
  window?: GameWindow;
  hooks?: LaunchHooks;
  jvmArgs?: string[];
  environment?: EnvironmentVariable[];
}
export interface Account { id: string; name: string; skinUrl?: string; skinChoice?: string; skinVariant?: SkinVariant }
export interface Accounts { accounts: Account[]; activeId: string | null }
export interface Cape { id: string; name: string; url: string; active: boolean }
export interface QuickTarget { kind: "world" | "server"; id: string }
export interface QuickDestination {
  instanceId: string;
  target: QuickTarget;
  name: string;
  playedAt: number;
  mode: "survival" | "creative" | "adventure" | "spectator" | null;
}
export interface InstanceOptions {
  name: string;
  minecraftVersion: string;
  loader: Loader;
  loaderVersion: string | null;
  ramMib: number;
  jvmArgs: string[];
  icon?: string | null;
  updateChannel?: UpdateChannel;
  launch?: LaunchOverrides;
}
export interface Instance extends InstanceOptions {
  id: string;
  createdAt: number;
  lastPlayedAt: number | null;
  playtimeSeconds: number;
  modCount: number;
}
export interface Overview {
  settings: Settings;
  instances: Instance[];
  accounts: Accounts;
  dataDir: string;
  totalMemoryMib: number;
  microsoftConfigured: boolean;
  testLaunchAvailable: boolean;
}
export interface Version {
  id: string;
  type: string;
}
export interface Catalog {
  latest: { release: string; snapshot: string };
  versions: Version[];
}
export interface Backup {
  id: string;
  instanceId: string;
  name: string;
  minecraftVersion: string;
  createdAt: number;
  archived: boolean;
  contentOnly: boolean;
  bytes: number;
}
export interface InstanceFile {
  name: string;
  directory: boolean;
  size: number;
  createdAt: number | null;
  modifiedAt: number | null;
}
export interface Job {
  id: string;
  instanceId: string | null;
  title: string;
  stage: string;
  status: "queued" | "running" | "done" | "cancelled" | "failed";
  completed: number;
  total: number;
  bytes: number;
  startedAt: number;
  error?: string;
  catalog?: {
    kind: ProjectKind;
    platform: Platform;
    project: string;
    world: string | null;
  };
}
export function isActiveJob(job: Job) {
  return job.status === "queued" || job.status === "running";
}

export interface Running {
  instanceId: string;
  pid: number;
  startedAt: number;
  playtimeSeconds: number;
  test: boolean;
}
export interface Performance { cpuPercent: number; memoryBytes: number }
export interface Project {
  id: string;
  title: string;
  description: string;
  author: string;
  downloads: number;
  platform: Platform;
  iconUrl: string | null;
  categories: string[];
  updatedAt: string | null;
  url: string | null;
  canDownload: boolean;
}
export interface ProjectDetails {
  summary: string;
  body: string | null;
  descriptionUnavailable: boolean;
  gallery: GalleryImage[];
  url: string | null;
}
export interface GalleryImage {
  url: string;
  thumbnailUrl: string;
  title: string | null;
  description: string | null;
}
export type ModSide = "server" | "client" | "both" | "singleplayer" | "unknown";
export const modSides: Record<ModSide, string> = {
  client: "Client-side only", server: "Server-side only", both: "Client and server", singleplayer: "Singleplayer only", unknown: "Unknown",
};
export interface InstalledMod {
  file: string;
  enabled: boolean;
  side: ModSide;
  sideOverride: ModSide | null;
  addedAt: number | null;
  managed: { title: string; platform: Platform; project: string; version: string; displayVersion?: string; iconUrl?: string; minecraftVersion: string; loader: Loader } | null;
}
export interface ModrinthArtwork {
  project: string;
  version: string;
  title: string;
  iconUrl: string | null;
  displayVersion: string | null;
}
export interface ModHistoryEntry {
  id: string;
  at: number;
  action: "install" | "update" | "remove";
  compatible: boolean;
  changes: Array<{
    title: string;
    before: ModRevision | null;
    after: ModRevision | null;
    undone: boolean;
  }>;
}
interface ModRevision {
  file: string;
  version: string | null;
}
export interface ModUpdatePreview {
  token: string;
  changes: Array<{ title: string; previous: string | null; next: string; file: string }>;
}
export interface JavaVersion { path: string; major: number }
export interface BrowserSignIn { id: string }
export interface Screenshot { instanceId: string; instance: string; name: string; takenAt: number }
export type SkinVariant = "classic" | "slim";
export interface Skin { id: string; name: string; variant: SkinVariant; height: number; builtin: boolean }
export type LogKind = "console" | "game" | "crash" | "test";
export interface Log {
  name: string;
  text: string;
  truncated: boolean;
  findings: { certainty: "confirmed" | "suspected"; title: string; evidence: string; confidence: string; severity: string; recommendation: string; suspectedMod: string | null }[];
}
export type Folder = "data" | "game" | "mods" | "resourcePacks" | "shaderPacks" | "screenshots" | "worlds";

export function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) return Promise.reject(new Error("Open Voltra through the desktop application to access local files and launch Minecraft."));
  return invoke<T>(command, args);
}

export function errorText(error: unknown) {
  return error instanceof Error ? error.message : typeof error === "string" ? error : "The operation could not be completed.";
}

export function displayDate(seconds: number) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(seconds * 1000);
}
