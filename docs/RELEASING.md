# Releasing Portreaper

Maintainer runbook for cutting a release. Releases are driven by **pushing a version tag**; CI builds the platform bundles and publishes the GitHub Release. There is no manual artifact upload.

> 简要说明（zh）：发布完全由 push 一个 `vX.Y.Z` tag 触发，CI 自动构建并发布。下面是给维护者的英文操作手册。

## Prerequisites

- Push access to `github.com/fanhefeng/portreaper`.
- Local toolchain: Rust (stable) + Node.js + pnpm, so the bump/build steps work.
- A clean `main` working tree (commit or stash unrelated changes first).

## Steps

### 1. Bump the version

```bash
node scripts/bump-version.mjs X.Y.Z
```

This rewrites the version in all the places that must stay in sync:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`
- `crates/portreaper-cli/Cargo.toml`
- `Cargo.lock` — 仓库根，**不是** `src-tauri/Cargo.lock`（workspace 根在仓库根，
  lockfile 属于整个 workspace）。其中 `portreaper` 与 `portreaper-cli` 两个包块都会被同步。

哪些 crate 纳入同步，判据是「**用户能不能看见这个版本号**」：

- `portreaper`（安装包）与 `portreaper-cli`（release 资产，用户会下载、会在
  `--version` 里读到、会拿它报 issue）都看得见 → 必须同步，否则用户报的版本
  对应不到任何一个 release（v0.8.0 就踩过：CLI 自报 0.1.0）；
- `crates/portreaper-core` 是不发布的内部库，用户永远看不到 → 刻意不同步，
  免得每次发版都产生无意义的 diff。

Use a plain semver string (`1.2.0`), no leading `v`.

### 2. Refresh / sanity-check the lockfile + formatting

```bash
cargo build --workspace
pnpm exec vp check
```

This confirms `Cargo.lock` is consistent after the bump and that the crate still compiles. Commit any lockfile changes it produces. `vp check` guards the bumped manifests' formatting — the v0.7.0 release turned main's CI red because the (since-fixed) bump script re-serialized `tauri.conf.json` in a non-oxfmt style; if it ever flags something, run `pnpm exec vp check --fix` and include it in the release commit.

### 3. Commit, tag, push

```bash
git add -A
git commit -m "chore(release): vX.Y.Z"
git tag vX.Y.Z
git push origin main --tags
```

The tag **must** match the version from step 1 (`v` prefix on the tag, none in the files). CI gates on this — see `verify-version` below.

### 4. Watch the release workflow

Pushing the tag triggers `release.yml`, which runs four stages:

1. **`verify-version`** — fails the run if the pushed tag does not match the version baked into `package.json` / `tauri.conf.json` / `Cargo.toml`. This is the guard against a forgotten `bump-version` step.
2. **`create-release`** — creates the **draft** GitHub Release exactly once, before any build leg. (The build legs upload by release id; letting three parallel legs get-or-create the same release is a documented tauri-action race that can leave duplicate/stuck drafts.)
3. **Build matrix** — builds in parallel. Each leg produces its installer bundle **plus the
   `portreaper-cli` binary** (uploaded directly under its stable name — the Raycast extension
   downloads it by that exact name):
   - macOS **arm64** `.dmg` + `portreaper-cli-macos-arm64`
   - macOS **x64** `.dmg` + `portreaper-cli-macos-x64`
   - Windows **x64** NSIS installer `.exe` + `portreaper-cli-windows-x64.exe`
4. **`publish`** — verifies all **six** assets exist on the draft, re-uploads the installers
   under their **stable asset names**, generates `portreaper-cli-SHA256SUMS` (aggregated from
   the three CLI binaries — the Raycast extension verifies its download against it), and only
   then flips the GitHub Release from **draft → published**:
   - `Portreaper-macos-arm64.dmg`
   - `Portreaper-macos-x64.dmg`
   - `Portreaper-windows-x64-setup.exe`
   - `portreaper-cli-macos-arm64`
   - `portreaper-cli-macos-x64`
   - `portreaper-cli-windows-x64.exe`
   - `portreaper-cli-SHA256SUMS`

   These names back the "latest" direct-download links in the README and on the website, e.g.
   `https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-macos-arm64.dmg`,
   and the Raycast extension's engine download (`integrations/raycast/src/install.ts`).
   `scripts/check-release-assets.mjs` guards both sides against drift.

### 5. If the publish job fails

The release is created as a **draft** and only published in the final `publish` job. **If `publish` fails, the release stays a draft** and the stable links 404. Fix the cause, then **re-run only the failed job** from the Actions UI (no new tag needed). Do not push a second tag for the same version — re-running the job is the supported path.

## Future: code signing

Builds are currently **unsigned** (hence the Gatekeeper / SmartScreen steps in the README).

To enable signing: (1) add the secrets below in **repo Settings → Secrets and variables → Actions**, then (2) **uncomment the signing `env:` block** in `.github/workflows/release.yml`.

> ⚠️ Why the block is commented out rather than always-on: an unset GitHub
> secret renders as an *empty string*, and tauri-bundler treats a
> present-but-empty `APPLE_CERTIFICATE` as a signing request — `security
> import` then fails the entire macOS build leg (verified on the first
> v0.2.0 run). Only uncomment after the secrets actually exist.

| Secret | Purpose |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: NAME (TEAMID)` |
| `APPLE_ID` | Apple ID used for notarization |
| `APPLE_PASSWORD` | app-specific password for notarization |
| `APPLE_TEAM_ID` | Apple Developer Team ID |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater key (for the future auto-updater) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | password for the updater key |
| `WINDOWS_CERTIFICATE` *(placeholder)* | Windows Authenticode cert — slot reserved; not yet configured |

The macOS group (`APPLE_*`) enables Developer ID signing + notarization for the `.dmg`s. The `TAURI_SIGNING_*` pair is reserved for the planned updater, not current release builds. The Windows Authenticode entry is a placeholder until a code-signing certificate is obtained.

## Checklist

- [ ] `bump-version.mjs X.Y.Z` run; all five files updated — `package.json`,
      `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`,
      `crates/portreaper-cli/Cargo.toml`, and the workspace `Cargo.lock`.
      (The five writes are not atomic — if the script dies midway, run
      `--check X.Y.Z` to see which files were left behind, then re-run the bump.)
- [ ] `cargo build --workspace` clean; `Cargo.lock` committed. (Bare `cargo build`
      from the repo root builds the default members only — it must cover
      `portreaper-core`, `portreaper-cli`, and the desktop shell alike.)
- [ ] Commit `chore(release): vX.Y.Z` + tag `vX.Y.Z` pushed.
- [ ] `verify-version` passed.
- [ ] All **six** artifacts built and uploaded with stable names (3 installers +
      3 `portreaper-cli-*` binaries), plus `portreaper-cli-SHA256SUMS` generated by
      the publish job — a missing CLI asset means the Raycast extension installs
      but can never fetch its engine.
- [ ] Both dmgs contain the quarantine helper (`解除隔离 Remove Quarantine.command`,
      injected by the "Inject quarantine helper into dmg" step from
      `src-tauri/dmg-extras/`) — mount one and check before announcing.
- [ ] First release shipping the helper: update the website/README install
      steps to mention "right-click the .command in the dmg" as the easy path.
- [ ] Release flipped from draft → published; `latest/download/...` links resolve.
- [ ] (When Windows leaves experimental) experimental section removed from the `body.md` heredoc in `release.yml`'s `create-release` job (grep for the `WINDOWS-EXPERIMENTAL` anchor comment) and the website (`dl-btn--exp` / `dl.experimental` in `website/index.html`).
