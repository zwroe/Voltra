<p align="center">
  <img src="src-tauri/icons/icon.png" width="56" height="56" alt="">
</p>

<h1 align="center">Voltra</h1>

<p align="center">Open-source launcher for modded Minecraft Java Edition.</p>

<p align="center">
  <a href="https://github.com/zwroe/Voltra/releases"><strong>Releases</strong></a>
  &nbsp;·&nbsp;
  <a href="docs/guide.md">User and developer guide</a>
</p>

![Voltra Library showing Minecraft instances and folders](docs/images/voltra-library.webp)

## Features

- Browse mods, modpacks, shaders, and resource packs from Modrinth and CurseForge. Filter by Minecraft version and loader.
- Keep each instance’s mods, settings, and saves separate. Run Vanilla, Fabric, Quilt, Forge, and NeoForge with managed Java.
- Take pack snapshots, test changes in a temporary instance, and inspect launch logs.
- Host a Vanilla or Fabric server from an instance and manage players, add-ons, and the live console.

| Discover | Server manager |
| --- | --- |
| [![Discover tab showing mod results and compatibility filters](docs/images/voltra-discover.webp)](docs/images/voltra-discover.webp) | [![Voltra showing a running Fabric server and its live console](docs/images/voltra-server.webp)](docs/images/voltra-server.webp) |

## Build from source

On Windows, use Node.js 22 or newer, Rust with the MSVC toolchain, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#windows).

```powershell
npm ci
npm run tauri dev
```

Run `npm run tauri build` to create the Windows installer.

[MIT License](LICENSE) · Independent launcher for Minecraft Java Edition.
