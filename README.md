[English](#english) | [中文](#中文)

---

## English

# Portreaper

**A macOS/Windows menubar app that hunts down orphaned dev-server "zombies" holding your ports hostage.**

[![CI](https://github.com/fanhefeng/portreaper/actions/workflows/ci.yml/badge.svg)](https://github.com/fanhefeng/portreaper/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/fanhefeng/portreaper)](https://github.com/fanhefeng/portreaper/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)

![Portreaper](website/assets/screenshot-main.png)

### What it does

You kill a terminal, but the `vite` / `node` / `cargo run` it launched keeps running — reparented to the OS, still squatting on port 3000. Next time you `npm run dev` the port is "already in use" and you have no idea which ghost to `kill`.

Portreaper is **not** a generic port viewer. It lives in your tray, scans every couple of seconds, and its core job is to **classify which listeners are orphaned dev-server zombies** so you can reap them with one click.

For every TCP-listening process it shows the ports, a human-readable app label, the executable path, the **launcher chain** (e.g. "this Node process was started by iTerm, whose parent is gone"), PID/PPID, uptime, CPU and memory. Suspected zombies are flagged with the exact signals that triggered the suspicion, sorted to the top, and counted in the tray. A **batch sweep** terminates the high-confidence ones in one action, and a **whitelist (收藏)** permanently exempts anything you trust.

### Download

Get it from the **[website](https://fanhefeng.github.io/portreaper/)** or **[GitHub Releases](https://github.com/fanhefeng/portreaper/releases/latest)**.

Stable direct links (always point at the latest release):

| Platform | Download |
| --- | --- |
| macOS (Apple Silicon) | [Portreaper-macos-arm64.dmg](https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-macos-arm64.dmg) |
| macOS (Intel) | [Portreaper-macos-x64.dmg](https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-macos-x64.dmg) |
| Windows (x64) | [Portreaper-windows-x64-setup.exe](https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-windows-x64-setup.exe) |

> **Windows is experimental.** The Windows build is compiled and unit-tested in CI but has not yet had manual QA on a real machine. Expect rough edges. See [docs/TESTING-WINDOWS.md](docs/TESTING-WINDOWS.md).

#### Opening an unsigned build

Releases are **not code-signed yet**, so the OS will warn you. This is expected.

**macOS (Gatekeeper).** After moving Portreaper to `/Applications`, the recommended path:

1. Try to open it once (it gets blocked).
2. Open **System Settings → Privacy & Security**, scroll down, and click **"Open Anyway"** next to the Portreaper notice.

Alternatives if that does not appear:

- Right-click the app → **Open** → confirm in the dialog, or
- Strip the quarantine flag from a terminal:
  ```bash
  xattr -dr com.apple.quarantine /Applications/Portreaper.app
  ```

**Windows (SmartScreen).** Run the installer; on the blue "Windows protected your PC" screen click **More info → Run anyway**. Because Portreaper terminates other processes, some antivirus / EDR products may flag it — allow it if you trust the source.

### How detection works

A process is judged by **signals** (reasons it looks orphaned), **exemptions** (reasons it's legitimately long-lived), and a resulting **confidence tier**.

| Signals (suspect) | Exemptions (never suspect) |
| --- | --- |
| Direct orphan — macOS `PPID=1`; Windows parent exited / PID slot reused | Managed by `launchd` / `launchctl` |
| Orphaned launcher chain — dead shell → live dev server | Homebrew service paths |
| Orphaned TTY session | Standard install paths (`/Applications`, `Program Files`, ...) |
| Defunct (`Z` / zombie state) | Managed by `pm2` |
| Dev-server keyword (`vite`, `node`, `cargo run`, ...) — extra reason, not required | |
| **Port-less dev orphan** — a dev process holding *no* port (e.g. an orphaned `electron-vite` Electron main) is still surfaced, with a "no port" badge | |
| **Duplicate dev server** — two instances of the same project (same command / same cwd + script identity); shown as `Possible`, never swept | |

Processes started (or reparented) less than 10 seconds ago are **downgraded to Possible** — still flagged with a reason chip, but never swept.

| Confidence | Meaning | In batch sweep? |
| --- | --- | --- |
| **Confirmed** | Defunct, or unambiguous orphan in a non-standard path | ✅ |
| **Likely** | Strong orphan signal + dev-server traits | ✅ |
| **Possible** | One weaker signal; shown but treated cautiously | ❌ |

The **one-click sweep** only kills `confirmed` + `likely`. Anything in the whitelist is exempt from suspicion, the tray count, and the sweep — it still appears in the list with a ★ chip.

> **Known limitation:** a dev server you *intentionally* detached (`nohup ... &` then closing the shell) is behaviorally identical to an accidental zombie and will be flagged. If you run such daemons on purpose, ★ star them once — the whitelist permanently exempts them. Daemons managed by `launchd`, `brew services` or `pm2` are already exempted automatically.

The app is **bilingual** (中文 / English) and follows your system language by default, with a manual toggle.

### Development

Prerequisites: **Rust** (stable), **Node.js**, and **pnpm**.

```bash
pnpm install        # first-time setup
pnpm tauri dev      # run the desktop app (vite on :1420 + tauri webview)
pnpm tauri build    # produce the bundle (.app / .dmg / .exe)
```

Rust-only iteration and tests:

```bash
cd src-tauri
cargo check
cargo test          # detection / classification unit tests
```

Releases are cut by pushing a version tag — see [docs/RELEASING.md](docs/RELEASING.md).

### Project structure

```
src/                  React 19 + TS single-file UI (App.tsx) — polls scan, renders table, kill/whitelist UI
src-tauri/src/
  lib.rs              tray, window lifecycle, invoke handlers
  commands.rs         Tauri command surface
  scanner/            process scan + v2 zombie classification (the core logic)
                      mod / model / classify / identify / macos / windows
  platform.rs         cross-platform kill with PID-reuse identity check
  whitelist.rs        JSON-persisted whitelist (收藏)
scripts/              release tooling (bump-version) + CI guards (reason parity, asset-name parity)
.github/workflows/    CI + release pipelines
website/              GitHub Pages download site
docs/                 maintainer & QA docs
```

### License

MIT © fhf. See [LICENSE](LICENSE).

---

## 中文

# Portreaper

**一个 macOS / Windows 菜单栏小工具，专门揪出占着端口不放的「僵尸」开发服务器进程。**

[![CI](https://github.com/fanhefeng/portreaper/actions/workflows/ci.yml/badge.svg)](https://github.com/fanhefeng/portreaper/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/fanhefeng/portreaper)](https://github.com/fanhefeng/portreaper/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)

![Portreaper](website/assets/screenshot-main.png)

### 它能做什么

你关掉了终端，但它启动的 `vite` / `node` / `cargo run` 还在跑 —— 被系统「收养」成了孤儿进程，继续占着 3000 端口。下次 `npm run dev` 提示「端口已被占用」，你却根本不知道该 `kill` 哪个鬼。

Portreaper **不是**通用的端口查看器。它常驻托盘，每隔两秒扫描一次，核心能力是**判断哪些监听进程是孤儿化的僵尸开发服务器**，让你一键收割。

对每个正在 TCP LISTEN 的进程，它会展示端口、人类可读的应用名、可执行文件路径、**启动链**（例如「这个 Node 是 iTerm 启动的，但它的父进程已经没了」）、PID/PPID、运行时长、CPU 和内存。疑似僵尸会标注出触发判断的具体信号、排到列表顶部、并计入托盘数字。**一键清扫**可一次性终止高置信度的那些，**收藏（白名单）**则可永久豁免你信任的进程。

### 下载

从**[官网](https://fanhefeng.github.io/portreaper/)**或 **[GitHub Releases](https://github.com/fanhefeng/portreaper/releases/latest)** 获取。

稳定直链（始终指向最新版本）：

| 平台 | 下载 |
| --- | --- |
| macOS（Apple 芯片） | [Portreaper-macos-arm64.dmg](https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-macos-arm64.dmg) |
| macOS（Intel） | [Portreaper-macos-x64.dmg](https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-macos-x64.dmg) |
| Windows（x64） | [Portreaper-windows-x64-setup.exe](https://github.com/fanhefeng/portreaper/releases/latest/download/Portreaper-windows-x64-setup.exe) |

> **Windows 为实验性版本。** Windows 构建仅在 CI 中编译并跑过单元测试，尚未在真机上做过人工验收，可能有不少粗糙的地方。详见 [docs/TESTING-WINDOWS.md](docs/TESTING-WINDOWS.md)。

#### 打开未签名的版本

发布包**目前尚未做代码签名**，系统会弹出警告，这是正常现象。

**macOS（Gatekeeper）。** 把 Portreaper 拖到「应用程序」后，推荐做法：

1. 先双击打开一次（会被拦截）。
2. 打开「**系统设置 → 隐私与安全性**」，向下滚动，在 Portreaper 的提示旁点击「**仍要打开**」。

如果上面那个按钮没出现，可改用：

- 右键点击 App →「**打开**」→ 在弹窗里确认；或
- 在终端里去掉隔离属性：
  ```bash
  xattr -dr com.apple.quarantine /Applications/Portreaper.app
  ```

**Windows（SmartScreen）。** 运行安装程序，在蓝色的「Windows 已保护你的电脑」界面点击「**更多信息 → 仍要运行**」。由于 Portreaper 会终止其他进程，部分杀毒 / EDR 软件可能将其标记为可疑 —— 若你信任来源，请放行。

### 检测原理

判定一个进程时，会综合**信号**（看起来像孤儿的理由）、**豁免**（合理地长期存活的理由），最终得出一个**置信度等级**。

| 信号（疑似） | 豁免（永不判为僵尸） |
| --- | --- |
| 直接孤儿 —— macOS `PPID=1`；Windows 父进程已退出 / PID 槽位被复用 | 由 `launchd` / `launchctl` 托管 |
| 启动链断裂 —— 已死的 shell → 仍存活的开发服务器 | Homebrew 服务路径 |
| 孤儿 TTY 会话 | 标准安装路径（`/Applications`、`Program Files` 等） |
| 已死（`Z` / defunct 状态） | 由 `pm2` 托管 |
| 命中 dev-server 关键字（`vite`、`node`、`cargo run` 等）—— 额外理由，非必需 | |
| **无端口的 dev 孤儿** —— 不占任何端口的孤儿 dev 进程（如 electron-vite 残留的 Electron 主进程）也会被检出，并带「无端口」徽标 | |
| **同项目重复 dev server** —— 同一项目的两个实例（命令相同 / cwd + 脚本身份相同）；只标为「可能」，永不清扫 | |

启动（或被收养）不足 10 秒的进程会被**降级为「可能」**——仍会标注原因，但永远不会进入清扫。

| 置信度 | 含义 | 计入一键清扫？ |
| --- | --- | --- |
| **确认（confirmed）** | 已 defunct，或在非标准路径下的明确孤儿 | ✅ |
| **疑似（likely）** | 强孤儿信号 + 开发服务器特征 | ✅ |
| **可能（possible）** | 仅一个较弱的信号；会显示但谨慎对待 | ❌ |

**一键清扫**只会终止 `confirmed` + `likely`。收藏（白名单）中的进程会被豁免于疑似判断、托盘计数和清扫 —— 它仍会出现在列表中，并带一个 ★ 标记。

> **已知限制**：你*有意*脱离终端的 dev server（`nohup ... &` 后关掉 shell）与意外产生的僵尸在行为上无法区分，会被标记。如果你确实需要这样跑守护进程，给它点一次 ★ 收藏即可永久豁免；由 `launchd`、`brew services` 或 `pm2` 托管的守护会被自动豁免。

应用为**中英双语**，默认跟随系统语言，也可手动切换。

### 开发

前置依赖：**Rust**（stable）、**Node.js**、**pnpm**。

```bash
pnpm install        # 首次安装
pnpm tauri dev      # 运行桌面应用（vite 跑在 :1420 + tauri webview）
pnpm tauri build    # 产出安装包（.app / .dmg / .exe）
```

仅 Rust 的迭代与测试：

```bash
cd src-tauri
cargo check
cargo test          # 检测 / 分类逻辑的单元测试
```

发布通过推送版本 tag 触发 —— 详见 [docs/RELEASING.md](docs/RELEASING.md)。

### 项目结构

```
src/                  React 19 + TS 单文件 UI（App.tsx）—— 轮询扫描、渲染表格、kill / 收藏交互
src-tauri/src/
  lib.rs              托盘、窗口生命周期、invoke 处理器
  commands.rs         Tauri 命令入口
  scanner/            进程扫描 + v2 僵尸分类（核心逻辑）
                      mod / model / classify / identify / macos / windows
  platform.rs         跨平台 kill，带 PID 复用身份校验
  whitelist.rs        JSON 持久化的收藏（白名单）
scripts/              发布工具（版本号 bump）+ CI 守卫（reason parity / 资产名一致性）
.github/workflows/    CI 与发布流水线
website/              GitHub Pages 下载站
docs/                 维护者与 QA 文档
```

### 许可证

MIT © fhf。详见 [LICENSE](LICENSE)。
