# Voltra guide

Voltra is a local-first Minecraft Java Edition desktop launcher for Windows. Tauri hosts the React interface, and Rust manages game files, credentials, downloads, and Java processes. Source is licensed under MIT.

## Build on Windows

Install [Node.js 22 LTS](https://nodejs.org/) or later, [Rust](https://rustup.rs/) with the default MSVC toolchain, and the Windows dependencies in [Tauri's prerequisites](https://v2.tauri.app/start/prerequisites/#windows). Microsoft C++ Build Tools needs the **Desktop development with C++** workload. Tauri uses the Edge WebView2 runtime.

Open a new terminal after installing the prerequisites. From the project directory, confirm the terminal can find Node and Cargo:

```powershell
node --version
cargo --version
npm ci
npm run tauri dev
```

If Tauri reports that `cargo metadata` could not run because the program was not found, install Rust with `winget install --id Rustlang.Rustup -e` and reopen your terminal. If Rust is already installed, confirm `where.exe cargo` resolves to its executable and `rustup show active-toolchain` reports an MSVC toolchain.

Vite is pinned to 6.4.6. The development dependency tree currently includes `picomatch` 4.0.7, which has an [upstream advisory](https://github.com/advisories/GHSA-fc5q-39xc-vjpf) with no published fix. The Vite development server binds to `127.0.0.1`. Keep the lockfile intact if `npm audit fix --force` proposes changing the Vite version.

Run the production bundle and checks with:

```powershell
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

The Windows installer is produced in `src-tauri/target/release/bundle/nsis/` after a successful bundle. Use `npm run tauri -- build --no-bundle` for `src-tauri/target/release/voltra.exe`. CI checks the React build, Rust unit tests, and native Tauri build on Windows.

Open `releases/voltra.exe` to run the current local release. Its published source ZIP is next to it, and older builds and archives are in `releases/archive/`. Cargo keeps build output in `src-tauri/target/`.

## Prepare and play

1. Select **Library → New instance**. Enter a name, choose a Minecraft release and mod loader, and set the RAM limit.
2. Use **Prepare files** in the instance overview to fetch Minecraft and any required runtime without signing in. Voltra detects the exact required Java major version and downloads a SHA-256-verified Windows Temurin distribution if needed.
3. Select **Connect Microsoft account** and authorize Voltra in your browser. A build with an approved Voltra client ID configured opens the authorization page directly.
4. Select **Play Minecraft**. The Downloads page shows preparation progress. Check **Diagnostics** for console output, crash reports, and live Java CPU and resident memory readings.

To check whether a modpack starts before connecting an account, open the instance and choose **Test Play** beside **Play**. Minecraft opens in single-player demo mode in a temporary copy of the pack. Voltra leaves existing worlds in the original instance and discards test worlds when the game closes.

Turn on **Discord Rich Presence** under **Settings → Integrations** and save preferences to show Voltra while the launcher is open. Launching Minecraft updates the activity with your version and elapsed time. Keep the Discord desktop app open with activity sharing enabled. The modpack name is optional. The settings panel shows whether Voltra has sent an activity update to Discord.

Use the title-bar back and forward arrows to revisit pages. The sidebar plus opens the instance creator from any page. In **Library**, create folders from the folder list. Drag a tile onto a folder, the Library root, or the Archive target. Click the tile’s grip to choose a folder with the keyboard. Archive asks for confirmation before moving an instance to **Library → Backups**. Removing a folder returns its instances to the Library.

An instance’s **Tools** menu opens Health, Diagnostics, Performance, History, and Screenshots. **Library → Add existing** imports modpacks and accepts share codes. Use **Settings → Appearance** to turn animations off.

Register Voltra as a public desktop client that supports personal Microsoft accounts. Add `http://localhost/oauth/callback` as a Mobile and desktop applications redirect URI. Minecraft Services must approve the registration. Set `VOLTRA_MICROSOFT_CLIENT_ID` when building a release. Developers can also set it at runtime. Voltra uses the authorization code flow with PKCE. Refresh tokens stay in the OS credential store. Minecraft access tokens live in launcher memory and are passed to Java when the game starts. See the [Microsoft desktop registration documentation](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration) and the [Minecraft application review form](https://aka.ms/mce-reviewappid).

Vanilla uses Mojang’s version manifest. Fabric and Quilt use their official launcher profiles. Forge (Minecraft 1.13 and newer) and NeoForge use their official client installers in isolated launcher cache directories. Exact usable versions depend on the loader’s upstream release catalog. Microsoft session renewal, new game and Java downloads, and provider requests require internet access.

## Mods and packs

- **Discover** searches mods, modpacks, resource packs, shaders, and data packs. Results can be filtered by source, game version, loader, and category. The current results stay available when you switch pages. Use **Refresh results** to fetch the same search again. Managed mod changes resolve required dependencies and use published checksums. Use an instance's **Content** tab to toggle, update, or remove installed mods.
- CurseForge requires your authorized key from the [CurseForge developer console](https://console.curseforge.com/). Enter it in Settings or supply `VOLTRA_CURSEFORGE_API_KEY` at runtime. Voltra checks distribution permissions and uses API-provided URLs. For a project that restricts distribution, download from the project page and place the JAR in the instance’s mods folder.
- **Import modpack** accepts Modrinth `.mrpack` files and Voltra ZIP exports. Modrinth imports support the Modrinth CDN and hash-verified required client files. Voltra ZIP exports contain configuration and pack files plus download references for managed Modrinth mods. Add CurseForge or unmanaged JARs separately on the destination. Copies of local worlds are stored in backups and excluded from pack exports.
- **Content** looks up Modrinth names, versions, and icons for local mod files, including files from already imported packs. Voltra sends file checksums to Modrinth and keeps matches in a local cache. Files without a match keep their filename and generic icon.
- The instance's **Files** tab includes local ZIP resource and shader pack management. Select installed packs in Minecraft or the relevant shader mod.
- **Skins** previews and applies built-in and imported Minecraft PNG skins. The cape picker lists capes from the active Minecraft account and can equip or hide a cape. **Screenshots** browses images from local instances. **Settings** lists installable Temurin Java major releases.
- **Home → Jump back in** lists local worlds and servers joined in Voltra. Each entry uses its original instance. Joining destinations directly and recording server history use Minecraft Java 1.20 or newer Quick Play.

## Local files and recovery

Voltra uses the OS local application data directory. Find its exact path in **Settings → Downloads and storage**. Instance settings, account metadata, and mod records use atomic JSON replacement. Each game uses its own `.minecraft` directory. Downloads go to a shared cache or the target instance. App and game console logs go in `logs/`.

**History → Pack snapshots** shows snapshots created before mod changes. Snapshots include mod metadata, JARs, config, resource and shader packs, data packs, known script directories, and game options. Compare any two snapshots or compare with current files by SHA-256. Restoring a pack creates a safety snapshot and leaves worlds in place. **Library → Backups** holds full instance backups and archives. An explicit full backup restore can replace worlds, so read the confirmation before proceeding.

Set the per-pack snapshot count and size budget in **Settings**. **History → Review retention** previews snapshots over budget and asks for confirmation before removing them. The latest snapshot is always kept. Managed downloads share a checksum-keyed cache. Installed JARs remain independent writable copies, so editing a file outside Voltra cannot alter another instance or a cached download.

## Host and share an instance

- **Instance → Server** starts a Vanilla or Fabric dedicated server on this PC. On first start, Voltra copies server-compatible mods and configs to a separate server folder, skips client-only and unclassified mods, and enables the Minecraft allowlist. **Console** shows live output and Java metrics. **Addons** manages server JARs, **Properties** edits server options, **Players** shows online players and manages the allowlist, and **Settings** sets server memory. Use **Stop server** to save and shut down the world. Friends need a route to your Minecraft TCP port.
- **Instance → Share → Create share code** prepares an encrypted direct transfer of installed mods, resource packs, shaders, scripts, and configs. Review configs for private data and share the code with someone you trust. In their Library, they choose **Use share code** to create a new instance. The host needs to stay online during the transfer. A LAN, a VPN, or port forwarding can provide the connection. There is no relay or cloud storage. Codes are single-use and expire if unused for 15 minutes after the pack is ready. Saves, account files, and launcher commands are excluded.

## Pack maintenance

- **Diagnostics** analyzes launcher, game, crash, and trial logs. High-confidence findings reflect what the log reported. Suspects and repeating errors are evidence for investigation. Unexpected exits are recorded and the session keeps matched diagnostic titles. **Share diagnostic summary** previews rule titles and fixes. **Share on mclo.gs** in Diagnostics and the share actions in the server console open an editable preview that masks common secrets. The log is uploaded only when you choose **Upload to mclo.gs**. Anyone with the resulting link can read it.
- **Guided mod isolation** tests a group in a temporary game folder, retains known dependencies, and tests the complementary group if the first trial is clean. Reproduce the same problem and record each result. Interactions, unavailable dependency metadata, and failures that need an existing world can be inconclusive. Trials use your connected Minecraft account when available, and single-player demo mode when signed out. Trial worlds are temporary. Custom launch hooks, environment variables, and JVM arguments are skipped for trials.
- **Health** reads managed catalog records and local Fabric manifests. Its dependency explorer shows relationships in both directions. Catalog mod versions, reported conflicts, and direct duplicate Fabric IDs are checked. Nested or opaque metadata, semantic version ranges in local manifests, and overlapping features are not fully inferred. Batch fixes can enable verified installed dependencies after a second check and snapshot. Missing downloads and conflicts without a safe resolution remain manual decisions.
- **Compare packs** previews checksums and selectively copies mods, configs, and other tracked files after confirmation. Mod copies require matching game and loader settings, and managed dependencies are rechecked. **Migration** checks existing projects for builds on a target Minecraft version and verifies downloads for related catalog search results. Related projects still need feature and world compatibility review. **Content → Export server folders** uses the existing metadata and manual side overrides to assemble server and client output.
- **World compatibility** compares active mod JARs and game and loader versions with the last successful session that Quick Play observed for each world. Unknown baselines and removed or changed files prompt a review before Play or Quick Play opens a world. The check is conservative and cannot establish what content a mod added, or account for every world opened from inside Minecraft. World backups are separate from pack restore.
- **Performance** records launch preparation time, process duration, exit outcome, periodic CPU and resident memory, and a local activity heatmap. Compare equivalent workloads before drawing conclusions. Process-level measurements cannot attribute CPU, heap, or FPS to an individual mod. Existing total playtime is preserved; older sessions have no graphs or individual records.

### Optional in-game telemetry

A compatible companion mod can read `VOLTRA_TELEMETRY_SESSION` and atomically write `voltra-telemetry.json` in the instance’s game directory. Send an updated record about every six seconds. The launcher accepts an increasing `sequence` for the current session and ignores stale, oversized, or invalid records. The optional `stutters` field is cumulative for that session. Send `world` only while the player is in an identified world. World time is estimated from consecutive samples. Each field can be omitted if unavailable.

```json
{
  "session": "00000000-0000-0000-0000-000000000000",
  "sequence": 17,
  "fps": 120,
  "frameMs": 8.3,
  "stutters": 2,
  "world": "Survival"
}
```

The launcher includes the local telemetry reader and graphs. A Minecraft-side companion is needed to produce FPS, frame time, stutter, and world data. Voltra does not guess these values from CPU or memory.

CPU is measured for the Java process and can exceed 100% across multiple cores. Memory is resident memory. Repeated-error findings apply to the visible log tail.

## Source layout

- `src/` contains the desktop interface and Tauri command client.
- `src-tauri/src/minecraft/` contains version preparation, Java detection, loader installation, and process startup.
- `src-tauri/src/` contains local storage, downloads, credential handling, provider implementations, modpacks, and diagnostics.
- Rust unit tests sit next to the behavior they cover.

Voltra is an independent launcher for Minecraft Java Edition.
