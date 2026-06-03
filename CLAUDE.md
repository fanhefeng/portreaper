# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this app is

Portreaper is a macOS-only menubar/tray desktop app that finds processes listening on TCP ports and helps the user kill orphaned dev-server "zombies" (e.g. a `vite`/`node`/`cargo run` process whose parent shell died, leaving `PPID=1`). It is **not** a generic port viewer — the core value is the zombie-suspect classification and the launcher-chain UI.

Stack: Tauri 2 + React 19 + TypeScript + Vite, package manager is **pnpm**. UI strings are in Chinese.

## Commands

```bash
pnpm install           # first-time setup
pnpm tauri dev         # run the desktop app (launches vite on :1420, then tauri webview)
pnpm tauri build       # produce a signed .app / .dmg bundle (targets: all)
pnpm dev               # frontend-only vite (rarely useful — no Rust IPC)
pnpm build             # tsc --noEmit + vite build (used by tauri build)
```

There are no tests, no linter, and no CI. `tsc` runs via `pnpm build`.

For Rust-only iteration: `cd src-tauri && cargo check` (or `cargo build`). Note that the Rust crate is `portreaper_lib` (see `Cargo.toml [lib]`); `main.rs` is just a thin entry point that calls `portreaper_lib::run()`.

## Architecture

### Two-process structure

- **Frontend** (`src/App.tsx`, single-file React app): polls `scan_ports` every 2 seconds, renders the table, owns the kill-confirm modal and the search/filter UI, and pushes a count into the tray title via `update_tray_title`.
- **Rust backend** (`src-tauri/src/`): owns process scanning, the whitelist file, the tray icon, and window lifecycle.

All frontend↔backend communication is Tauri `invoke()` calls listed in `src-tauri/src/lib.rs` (`invoke_handler![...]`). When adding a command, you must register it there **and** add any new permissions to `src-tauri/capabilities/default.json`.

### Tray-first window lifecycle

The window close button is intercepted (`lib.rs` `on_window_event` → `window.hide()` + `prevent_close()`) — the app keeps running in the tray and is re-shown from the tray menu or `show_main_window` command. Quitting only happens via the tray "退出" menu item. Keep this in mind when reasoning about app shutdown or state cleanup.

### Scanner pipeline (`src-tauri/src/scanner.rs`)

`scan()` shells out to two macOS commands and joins them by PID:

1. `lsof -iTCP -sTCP:LISTEN -P -n -FpcLn` — listening sockets, parsed in field-prefix mode (`p`/`c`/`L`/`n` tags). One PID may listen on multiple ports; they are merged into `ports: Vec<u16>`.
2. `ps -A -o pid=,ppid=,state=,tty=,etime=,pcpu=,rss=,command=` — process metadata for **all** PIDs, used both for the lsof rows and for walking parent chains.

`LANG=en_US.UTF-8` is forced on both subprocesses so column parsing isn't broken by localized output. The whole pipeline is macOS-specific (lsof flags, ps `etime`/`rss` columns, `/Applications/` heuristics, `launchd` as PID 1). Porting to Linux/Windows would require a separate scanner module.

### Classification (the product's core logic)

Two intertwined heuristics in `scanner.rs`:

- `identify_app(full_command, short_command) -> (label, category)` — figures out a human-readable name and one of: `installed-app` / `system` / `dev-script` / `user-binary` / `unknown`. Order matters: `.app/` bundle → `/Applications/` bare binary → system path prefixes → script runtime (node/python/...) extracting the script name and project → `/usr/local/`, Homebrew → `/target/{debug,release}/` → `/Users/...` fallback.
- `classify(...) -> (is_suspect, reasons)` — a process is a zombie suspect **only if** `PPID=1` AND the exe path is not in a standard install location (`SYSTEM_PATH_PREFIXES`) AND its category isn't `installed-app`/`system`. Real defunct (`state` contains `Z`) is always a suspect. Dev-server keyword match (`DEV_SERVER_PATTERNS`) is added as an extra reason but is **not** required to trigger suspicion.

`build_parent_chain` walks PPIDs up to 12 levels, stopping at PID 1 (synthesizes a "launchd" node) or at the first `installed-app` parent (so we surface "this Node process was launched by iTerm/Cursor" rather than the whole shell chain).

When adjusting these heuristics: extending `DEV_SERVER_PATTERNS` is cheap and additive, but changing the `classify` predicate or the `identify_app` ordering can reclassify huge swaths of the user's process list — preserve the "standard install path ⇒ never a zombie" invariant.

### Whitelist (`src-tauri/src/whitelist.rs`)

Persisted as JSON at `{app_config_dir}/whitelist.json` (path injected at startup in `lib.rs` via `whitelist::init`). The key is `exe_path` (preferred, stable across restarts) falling back to the lsof `command` string — see `scan()` where `wl_key` is built. Frontend toggles via `add_whitelist` / `remove_whitelist` with whatever key the row exposes; keep both sides using the same precedence.

Whitelisted entries still appear in the list but with `is_zombie_suspect: false` and a star chip — they're filtered out of the tray suspect count and the "一键清扫" batch kill.
