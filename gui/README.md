# mmcp-gui

Desktop client for mmcp memories. Tauri 2 shell with a SvelteKit + Tailwind v4
frontend. Talks to `mmcp-store` directly over Tauri IPC — no HTTP, no MCP
protocol round-trip.

This is a peer of `webui/` (the Leptos server-side web frontend). Both
ultimately target the same `$lib/components/` shape so a future pass can
extract shared UI into `packages/ui/` and consume it from both apps. Today,
only this desktop app exists in the new stack.

## Prerequisites

- **Rust** (stable toolchain — matches the workspace `rust-toolchain.toml`).
- **[bun](https://bun.sh/)** ≥ 1.1 for the frontend (`bun install`, `bun run
  tauri dev`).
- **[Tauri 2 prerequisites](https://tauri.app/start/prerequisites/)** for
  your OS:
  - **Windows**: WebView2 (bundled with Windows 11; the installer pulls it
    on first `bun run tauri dev` if missing), Microsoft C++ Build Tools.
  - **macOS**: Xcode command-line tools.
  - **Linux**: `libwebkit2gtk-4.1-dev`, `build-essential`, `curl`, `wget`,
    `libxdo-dev`, `libssl-dev`, `libayatana-appindicator3-dev`,
    `librsvg2-dev` (names vary by distro — see Tauri's docs).

## Layout

```
gui/
├── src-tauri/          Rust backend (Tauri commands wrapping mmcp-store)
│   ├── Cargo.toml      Path deps to ../../crates/mmcp-*; OUT of the Rust
│   │                   workspace (see root Cargo.toml `[workspace.exclude]`)
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs      Builder + setup + reachability probe
│       ├── state.rs    AppState (backend, group index, sync bundle, author)
│       ├── error.rs    GuiError (serde-tagged)
│       └── commands/   groups / memory / sync / diagnose / settings
├── src/
│   ├── app.html        SvelteKit HTML shell
│   ├── app.css         Tailwind entry + @plugin typography
│   ├── routes/
│   │   ├── +layout.svelte
│   │   ├── +layout.ts  ssr = false, prerender = true (pure SPA)
│   │   └── +page.svelte  Three-pane layout + modals
│   └── lib/
│       ├── types.ts     TS mirrors of Rust DTOs
│       ├── api/         Tauri command wrappers (swappable for fetch in
│       │                the future webui pass)
│       ├── stores/      Svelte 5 rune state modules
│       └── components/  Pure presentational (future shared with webui)
├── static/
├── package.json        bun scripts (dev / build / check / tauri)
├── svelte.config.js    @sveltejs/adapter-static
├── vite.config.ts      sveltekit + @tailwindcss/vite
├── tailwind.config.ts  Preset with per-kind colour tokens (exportable)
├── tsconfig.json       extends .svelte-kit/tsconfig.json
└── .gitignore
```

## Development

```bash
cd gui
bun install
bun run tauri dev
```

The first run will pull the tauri-cli, webview runtime components (on
Windows), compile the Rust backend, and open a native window. Vite's HMR
picks up every `.svelte`, `.ts`, and `.css` change instantly; Cargo
rebuilds the backend when anything under `src-tauri/src/` changes.

### Devtools

`F12`, `Ctrl+Shift+I`, or right-click → Inspect opens WebView2 devtools.
The `devtools` feature is on by default on the `tauri` dep so this works
in dev and release.

### Exercising the backend

```js
// in devtools console
await window.__TAURI__.core.invoke('list_groups')
await window.__TAURI__.core.invoke('sync_status')
window.__TAURI__.event.listen('reachability:changed', e => console.log(e.payload))
await window.__TAURI__.core.invoke('load_memory', { groupId: '<uuid>', slug: '<slug>' })
```

Tauri 2's global is `window.__TAURI__.core.invoke` (note: `.core.`, not
bare `.invoke`) and `window.__TAURI__.event.listen`.

## Release build

```bash
cd gui
bun install
bun run tauri build
```

Produces a bundle under `src-tauri/target/release/bundle/` appropriate for
the host OS (MSI/EXE on Windows, DMG on macOS, AppImage/DEB on Linux).

## Type check

```bash
cd gui
bun install
bun run check
```

Runs `svelte-kit sync` (generates `.svelte-kit/tsconfig.json` with the
`$lib/*` path alias) followed by `svelte-check` across the whole
`src/` tree. IDE errors about `Cannot find module '$lib/...'` disappear
as soon as this has run once.

## Platform notes

- The bundled placeholder icons (`src-tauri/icons/`) are minimal
  stand-ins so `tauri-build`'s `windres` step passes on Windows. A
  future commit ships real artwork — use `bunx @tauri-apps/cli icon
  <source.png>` to regenerate every size from a source PNG if you want
  to swap them yourself.
- `bun` has occasional rough edges with Tauri's native deps. If
  `bun install` trips on `@tauri-apps/cli` or a transitive native
  module, `npm install` is a drop-in replacement (the `package.json`
  scripts work with either).
