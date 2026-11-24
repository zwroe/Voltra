# Contributing to Voltra

Voltra is an open-source launcher for Minecraft Java Edition. Bug reports, platform testing, documentation fixes, and pull requests are welcome. Check the [open issues](https://github.com/zwroe/Voltra/issues) for related work. If you have a larger feature in mind, open an issue to discuss how it would work before building it.

## Run Voltra locally

Install Node.js 22 or newer and stable Rust, then follow the [Tauri prerequisites for your operating system](https://v2.tauri.app/start/prerequisites/).

Run these commands from the repository root.

```sh
npm ci
npm run tauri dev
```

The React and TypeScript code lives in `src/`. Rust code for Minecraft launches, Java management, downloads, and local storage lives in `src-tauri/src/`. See the [developer guide](docs/guide.md) for sign-in and provider configuration.

## Report a bug

Include the following details in your [GitHub issue](https://github.com/zwroe/Voltra/issues/new).

- Your Voltra version, operating system, and CPU architecture.
- The Minecraft version, loader and loader version, and Java version if the issue involves an instance.
- The steps to reproduce it, what you expected, and what happened.
- A screenshot for a visual issue or the relevant lines from **Diagnostics** for a launch or download issue. Review logs for account details, tokens, and API keys before sharing.

If the issue occurs with a specific mod or modpack, include its name and version so someone else can try the same setup.

## Submit a pull request

Keep the change focused and describe how you tested it. For UI changes, include a screenshot or short recording. For changes to installs, instance files, or saved settings, mention which operating systems you tested and whether you tried an existing instance.

```sh
npm run build
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

## Help test and fix

- **Installer and first launch.** Try a clean install on Windows, macOS on Intel or Apple silicon, or Linux. Check that Voltra opens, detects or installs Java, and launches a fresh instance. Include the installer format in your report.
- **Legacy loader compatibility.** Verify older Forge installer and client-patch paths, and NeoForge on Minecraft 1.20.1. Record the exact Minecraft, loader, and Java versions that worked or failed.
- **Pack sharing.** Try sending a pack between two devices on the same network. Report connection failures, interrupted transfers, and error messages that could be clearer.
- **Documentation.** Check the setup and recovery instructions against the current release. Screenshots and platform-specific corrections are welcome.

## Feature ideas

- **First-run setup inside Voltra.** Guide players through Microsoft sign-in, memory settings, and creating or importing their first instance.
- **Launcher update notifications.** Let users know when a new Voltra release is available and direct them to the installer for their platform.
- **CurseForge ZIP import.** Import exported CurseForge modpacks and clearly identify any mods that need to be downloaded from their project pages.
