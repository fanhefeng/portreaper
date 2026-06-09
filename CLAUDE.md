# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this app is

Portreaper is a macOS + Windows menubar/tray desktop app that finds processes listening on TCP ports and helps the user kill orphaned dev-server "zombies" (e.g. a `vite`/`node`/`cargo run` process whose launching shell died). It also surfaces **orphaned dev processes that hold no port at all** (e.g. an `electron-vite` Electron main process left behind when its parent `node` is killed — adopted by launchd, listening on nothing) — these are invisible to a port scan but are exactly the kind of dev residue users want reaped. It is **not** a generic port viewer — the core value is the zombie-suspect classification (with confidence tiers) and the launcher-chain UI.

Stack: Tauri 2 + React 19 + TypeScript + Vite, package manager is **pnpm**. UI is bilingual (zh/en) via `src/i18n.ts`.

## Commands

```bash
pnpm install           # first-time setup
pnpm tauri dev         # run the desktop app (launches vite on :1420, then tauri webview)
pnpm tauri build       # produce a .app/.dmg (macOS) or NSIS .exe (Windows)
pnpm build             # tsc --noEmit + vite build (used by tauri build)

cd src-tauri && cargo test                 # 35+ unit tests incl. classify fixtures
cd src-tauri && cargo clippy --all-targets -- -D warnings
cargo test live_scan -- --ignored --nocapture   # real-machine smoke: scan this Mac

pnpm test                                  # frontend regression tests (vitest + happy-dom)

# Windows cross-compile check from macOS (needs `brew install llvm` for llvm-rc):
PATH="/opt/homebrew/opt/llvm/bin:$PATH" cargo check --target x86_64-pc-windows-msvc

node scripts/check-reason-parity.mjs       # Rust ReasonCode <-> i18n/render-path guard
node --test scripts/*.test.mjs             # guard-script self-tests
node scripts/bump-version.mjs --check 0.1.0
```

CI (`.github/workflows/ci.yml`) runs the full gate on macOS + Windows. Release is tag-triggered (`v*`, see `docs/RELEASING.md`). The Rust crate is `portreaper_lib` (see `Cargo.toml [lib]`); `main.rs` is a thin entry point calling `portreaper_lib::run()`.

## Architecture

### Two-process structure

- **Frontend** (`src/App.tsx` + `src/i18n.ts`): polls `scan_ports` every 2 seconds, renders the table, owns the kill-confirm modals and search/filter, pushes counts into the tray via `update_tray_title`. All strings go through the typed i18n dict — a missing English key is a tsc error.
- **Rust backend** (`src-tauri/src/`): owns scanning, classification, the whitelist file, the tray, and window lifecycle.

All frontend↔backend communication is Tauri `invoke()` calls registered in `src-tauri/src/lib.rs` (`invoke_handler![...]`). When adding a command, register it there and add any new permissions to `src-tauri/capabilities/default.json` if it touches gated APIs.

### Scanner module layout (`src-tauri/src/scanner/`)

Platform variance is isolated in two leaf files with **identical cfg-gated function signatures** (compile-time polymorphism, no traits):

```
scanner/mod.rs       orchestration: collect → (per-PID) build_entry [snapshot → classify → chain walk] → sort
                     two scan paths share build_entry: (1) listeners from lsof/GetExtendedTcpTable;
                     (2) orphan dev processes — full-process-table rows that hold no port
scanner/model.rs     ProcessEntry (serde contract with App.tsx), ProcMeta, ProcessSnapshot
scanner/classify.rs  PURE classifier + ReasonCode/Confidence enums + fixture tests
scanner/identify.rs  shared, separator-agnostic path/project/script helpers
scanner/macos.rs     lsof + ps + launchctl list; macOS path ladder
scanner/windows.rs   GetExtendedTcpTable + sysinfo; SHGetKnownFolderPath path ladder
platform.rs          kill with start-time identity check (PID-reuse guard)
```

Data sources: macOS = `lsof -iTCP -sTCP:LISTEN -P -n -FpcLn` + `ps -A -o pid=,ppid=,state=,tty=,etime=,pcpu=,rss=,command=` + a second `ps -A -o pid=,comm=` (comm is the authoritative exe path — it survives spaces in paths, unlike splitting the command line) + `launchctl list` (managed-PID set). Session leaders are identified by the `s` flag in the ps *state* column — **never** use `ps -o sess=`: on macOS it prints 0 for every process (review-caught bug that made every terminal process look session-orphaned). Windows = `GetExtendedTcpTable` (IPv4+IPv6 listeners, no subprocess) + a long-lived `sysinfo::System` (the 2s poll provides the CPU sampling interval; first scan shows 0% CPU by design). `LANG=en_US.UTF-8` is forced on all macOS subprocesses.

### Classification v2 (the product's core logic)

`classify(&ProcessSnapshot) -> Verdict` in `classify.rs` is a **pure function** — every OS probe is precomputed into the snapshot by `mod.rs`/the platform collectors, so the decision tree is table-testable on both platforms. Decision order:

1. `state` contains `Z` (defunct) → always suspect, `Confirmed`.
2. **Hard exemptions** (in order): `launchd_managed` (launchctl claims the PID — covers brew services, LaunchAgents) → `exe_is_standard_install` → `brew_service_path` (fallback for system-domain brew services launchctl can't see) → `pm2_managed`.
3. Orphan signals: `direct_orphan` (macOS: PPID=1; Windows: parent missing or parent created *after* child = PID slot reused), `chain_orphan` (parent chain ends at init/dead root without passing a live user-visible app, and the leaf is dev-like or an ancestor shell is itself orphaned), `tty_orphaned` (real ttys with no session leader — dead terminal session).
4. No signal → not a suspect.
5. `elapsed_secs < 10` → `Possible` + `just_reparented` (grace period; never swept).
6. Tiers: orphan×dev or orphan×dead-session → `Confirmed`; bare orphan/chain → `Likely`; session-only → `Possible`.

**Invariants — do not break:**
- Standard install path / launchd-managed ⇒ never auto-flagged.
- **Exception to the path rule:** `dev-script` category is *not* exempted by the interpreter's exe path — a script runtime's identity is its *script*. `/usr/bin/python3 app.py` and `C:\Program Files\nodejs\node.exe vite.js` must remain detectable (`identify_app` classifies by script location first). The same applies to `-m <module>` invocations (`identify.rs extract_module_arg`): an orphaned `python -m http.server` is `dev-script` regardless of where the interpreter lives, and the `brew_service_path` exemption is decided by the *identity path* (`mod.rs brew_service_exemption`) — the script's location when there is a script, never the interpreter's for `-m` calls; the interpreter path is only the conservative fallback for bare/console-script invocations (keeps system-domain brew python services exempt).
- The chain walk stops at any live user-visible app: `installed-app` category, any `.app/` bundle on macOS (stock Terminal.app lives in `/System/Applications/`!), and live session roots on Windows (`explorer.exe`, `services.exe`, ... — without this every cmd-launched dev server would be a false positive, since Windows chains all end at the exited `userinit.exe`).
- The flagged process is always the **port-holding leaf**, never an intermediate shell.
- Sweep ("一键清扫") and the tray count cover `Confirmed`+`Likely` only; `Possible` is never swept.
- **Duplicate dev-server detection** (`mod.rs mark_duplicates`, cross-entry post-pass after the pure per-entry classify): two dev-script listeners are duplicates when ports are disjoint, they are not parent/child, and (a) the full command is identical, (b) the path-derived (project, script/module) identity matches, or (c) cwd is identical + script/module identity matches (catches "Warp started :5173, VS Code started :5174 of the same project"). **cwd is the strongest evidence and a known-different cwd vetoes the pair** — hoisted node_modules collapses path-derived project names to the monorepo root and even makes full commands identical across distinct apps, so monorepo sub-packages / git worktrees are only told apart by cwd (collected for listener PIDs only: macOS `lsof -d cwd`, Windows `sysinfo cwd()`). A live common non-shell parent or grandparent (concurrently/cluster/turbo) marks intentional multi-instance, never duplicates; a shell ancestor or a dead/synthetic parent does **not** exempt (re-running in one terminal, or a co-reparented orphan pair, is exactly the target). Duplicates only ever reach `Possible` (+ `duplicate_of` peer PID) — the machine cannot know which instance the user is using, so they are **never swept**.
- **Orphan dev processes (second scan path in `scan()`):** the full process table (already collected via `ps -A` / `sysinfo` for the chain walk — no extra syscall) is swept for processes holding no port. Inclusion gate is **stricter** than for listeners — a row must be **dev-like** (`dev_keyword || dev_category`) *and* classify as a suspect. Without the dev-like gate the dozens of normal `ppid==1` system daemons would flood in: a port is the listeners' "worth-attention" evidence, dev-likeness is the orphans'. Whitelisted orphans still surface (so the user can un-star); non-suspect dev processes (a healthy `vite` in a live terminal) are not listed. A cheap dev-like pre-gate skips the parent-chain walk for non-dev rows. Orphan entries have empty `ports` (frontend shows a "no port" badge).
- **node_modules `.app` is `dev-script`, not `installed-app`** (`macos.rs identify_app` ladder step 1): electron/electron-vite ship `Electron.app` under `node_modules/electron/dist`, byte-identical in form to a real `/Applications` app. Without this an orphaned dev Electron would be exempted as an installed app and never detected. `/node_modules/` in the path is a zero-false-positive signal — user-installed apps never live there.
- Whitelist (`is_whitelisted`) still lists the row, forces `is_zombie_suspect=false`, excluded from sweep/tray. Key (`scanner::mod::whitelist_key`, mirrored by `App.tsx whitelistKey`): `exe_path` **only when it contains a path separator** — a PATH-resolved bare interpreter name (`ps -o comm=` returns just `node` for `node app.js`/shebang shims) would collapse the key across every same-named listener and whitelist them all, so bare names fall back to the full command line (then lsof `command`). Keep frontend and `scan()` byte-for-byte identical.

`ReasonCode`/`Confidence` serialize as snake_case/lowercase and are rendered in the frontend through four key families: `reason.*` + `reasonTip.*` (detail panel, every code), `story.*` (inline primary-reason story, positive codes only), `verdict.*` (inline confidence prefix). **Adding a ReasonCode variant requires (a) classifying it into `App.tsx` `REASON_PRIORITY` (positive) or `EXEMPT_REASONS` (exemption), and (b) adding the zh+en keys for its families in `src/i18n.ts`** — `scripts/check-reason-parity.mjs` (run in CI, self-tested by `scripts/check-reason-parity.test.mjs`) fails otherwise.

### Kill path (`platform.rs`)

`kill_process(pid, force, start_unix)`: the frontend passes the scan-time `start_unix`; the backend re-reads the process creation time (macOS: `ps -o etime=`; Windows: `GetProcessTimes` on the same handle later used by `TerminateProcess`) and refuses with `ERR_PID_REUSED` if it moved by >5s — killing a recycled PID is a data-loss bug. macOS: SIGTERM/SIGKILL two-button UI. Windows: single "Terminate" button (`TerminateProcess`; there is no reliable graceful kill for detached console processes — deliberate product decision, do not add CTRL_BREAK heuristics). `ERR_*:`-prefixed errors are app-semantic and localized in the frontend; everything else surfaces as OS text.

### Tray / window lifecycle

The window close button is intercepted (`lib.rs` `on_window_event` → hide + `prevent_close()`) — the app lives in the tray; quitting only happens via the tray menu. **macOS ⌘Q** is handled by replacing the default app menu (`Builder::menu` in `lib.rs`): muda's predefined Quit calls `[NSApp terminate:]`, which tao 0.35 does **not** route through `ExitRequested` (empirically verified — the process just dies; do not "fix" ⌘Q by intercepting `ExitRequested{code: None}`, that event only fires when the last window is *destroyed*). The custom `quit-to-tray` item owns the Cmd+Q accelerator and hides the window instead; the Edit/Window submenus must stay (webview ⌘C/⌘V/⌘W depend on them). Dock-quit / logout / shutdown (AppleEvent quit) still exit for real — deliberate: system-initiated quits must be honored. macOS shows counts in the tray *title* (`icon_as_template` gated to macOS); Windows has no tray title — counts go to the *tooltip*. Tray menu labels are bilingual: handles are stored in `TrayMenuItems` state (+ `AppMenuItems` for the macOS ⌘Q item) and re-texted by `set_tray_language` (called by the frontend on toggle; initial language from `sys-locale`).

### Windows caveats (no dev machine — CI is the safety net)

`scanner/windows.rs` and the Windows half of `platform.rs` are compile-checked locally via the cross-target command above and compiled+unit-tested in CI, but have **no manual QA**. The Windows release asset is labeled experimental; the acceptance checklist is `docs/TESTING-WINDOWS.md`. When exe paths are unreadable (MSIX/elevated processes), the scanner errs toward *not* flagging (empty exe ⇒ treated as standard install).

## Release / website

Tag push `v*` → `.github/workflows/release.yml`: version-consistency gate → tauri-action matrix (macOS arm64/x64 dmg, Windows NSIS) → publish job verifies all 3 assets, re-uploads **stable-named** copies (`Portreaper-macos-arm64.dmg`, `Portreaper-macos-x64.dmg`, `Portreaper-windows-x64-setup.exe`) and flips the draft to published. The website (`/website`, GitHub Pages via `pages.yml`) hardcodes `releases/latest/download/<stable-name>` URLs, so downloads auto-update on every release with no site changes. Bump versions only via `node scripts/bump-version.mjs X.Y.Z` (syncs package.json, tauri.conf.json, Cargo.toml, Cargo.lock). Runbook: `docs/RELEASING.md`.
