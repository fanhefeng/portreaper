# 架构规划：引擎下沉为 core，适配多前端（Raycast 等）

> 状态：**步骤 1 已落地**（见 §8 的进度标记），步骤 2–5 待做。
> 目标读者：本仓库维护者。阅读前建议先看 `CLAUDE.md` 的「Architecture」一节。
>
> §4 的目录结构是**目标态**。当前 `crates/portreaper-core/src/` 下仍是搬迁来的
> 原始布局（`scanner/` 六个文件 + `platform.rs`），子模块细分属于步骤 2。

## 1. 目标与非目标

**目标**

- 把「孤儿 / 僵尸进程的判定与清理」逻辑抽成独立、零 GUI 依赖的 Rust crate，成为唯一真相源。
- 提供一个稳定的**进程级契约**（CLI + JSON），让 Raycast 扩展、脚本、CI 检查、未来的 Alfred / 编辑器插件都能复用同一套判定，不重写、不漂移。
- 桌面版 GUI 行为**零变更**：同样的判定结果、同样的白名单文件、同样的托盘语义。

**非目标**

- 不做常驻 daemon、不开监听端口。一个用来杀占端口进程的工具自己占端口，产品上说不通；同时 daemon 会引入生命周期、权限、升级三类新问题。
- 不做 napi-rs / node 原生模块。要为每个 Node ABI × 平台出预编译包，维护成本远超收益。
- 不引入 async 运行时。引擎是「一次采集 + 纯计算」，同步阻塞模型最简单；并发由调用方决定（GUI 现在用 `spawn_blocking`，保持不变）。
- 不做移动端。`crate-type = ["rlib"]` 的既有决策延续。

## 2. 现状盘点

好消息：**引擎其实已经解耦了，只是没被暴露出去**。

`src-tauri/src/scanner/`（`mod.rs` / `classify.rs` / `identify.rs` / `model.rs` / `macos.rs` / `windows.rs`，约 5200 行）与 `platform.rs` 对 `tauri` 的引用数为 **0** —— 唯一的字面命中是 `classify.rs` 里的 dev 关键字字符串 `"tauri"` 和 `windows.rs` 的一句注释。`classify()` 已经是纯函数，`ProcessSnapshot` 已经把所有 OS 探测预计算掉了，表驱动单测覆盖两个平台。

真正的耦合只有四处，且都不在判定逻辑里：

| 耦合点 | 现状 | 影响 |
|---|---|---|
| `scanner` 是 `mod` 不是 crate | 编译进 `portreaper_lib` | 外部消费者无法引用 |
| `whitelist.rs` | 进程级 `static` + `init(path)` 注入 | 常驻 GUI 模型；短命 CLI 进程别扭，路径来源在外部 |
| `paths.rs` | 依赖 `tauri::AppHandle` | CLI 找不到同一份 `whitelist.json` |
| ReasonCode 文案 | 只在 `src/i18n.ts` | 第二个前端必须重写翻译，必然漂移 |

另有两处「不是耦合、但会挡住第二个前端」的设计债，在拆分时一并处理（见 §5）：错误用 `String` 前缀传递、Windows CPU 依赖全局 `System` 的轮询采样区间。

## 3. 分层模型

```
┌─ L2 前端 ────────────────────────────────────────────────┐
│  桌面 GUI (Tauri+React)   Raycast 扩展   裸 CLI / 脚本 / CI │
└──────┬──────────────────────┬─────────────────┬──────────┘
       │ invoke()             │ spawn + JSON    │ spawn + JSON
┌──────┴──────────┐   ┌───────┴─────────────────┴──────────┐
│ L1 src-tauri    │   │ L1 portreaper-cli                   │
│ GUI 壳：托盘 /  │   │ 进程边界：子命令 + stdout JSON      │
│ 窗口 / 命令注册 │   │ + 稳定 exit code                    │
└──────┬──────────┘   └───────┬─────────────────────────────┘
       │                      │
       └──────────┬───────────┘
       ┌──────────┴──────────────────────────────────────────┐
       │ L0 portreaper-core                                   │
       │ 采集 → 判定 → 排序 → 终止；白名单；路径；理由文案     │
       │ 零 GUI 依赖，零 IPC 依赖，同步阻塞                    │
       └──────────────────────────────────────────────────────┘
```

判定逻辑只存在于 L0。L1 只做「翻译」：把 L0 的结构体翻译成 IPC 消息或 JSON。L2 只做展示与交互。

## 4. 目录结构

```
portreaper/
├── Cargo.toml                      # ← 新增：[workspace] members = ["crates/*", "src-tauri"]
├── crates/
│   ├── portreaper-core/
│   │   ├── Cargo.toml              # serde / log / once_cell；(win) sysinfo + windows
│   │   └── src/
│   │       ├── lib.rs              # 门面：Scanner、scan_once、kill、公开类型 re-export
│   │       ├── scan/
│   │       │   ├── mod.rs          # 编排：collect → build_entry → 后处理 → 排序
│   │       │   ├── entry.rs        # build_entry（两条扫描路径的共用体）
│   │       │   ├── chain.rs        # build_parent_chain + ChainFlags
│   │       │   ├── duplicates.rs   # mark_duplicates（跨条目后处理）
│   │       │   ├── subtree.rs      # fill_subtree_cpu（展示用后处理）
│   │       │   └── model.rs        # ProcessEntry / ProcMeta / Collected / ProcessSnapshot
│   │       ├── classify.rs         # 纯判定 + ReasonCode / Confidence（原样搬）
│   │       ├── identify.rs         # 路径/项目/脚本识别（原样搬）
│   │       ├── platform/
│   │       │   ├── mod.rs          # cfg 分派 + 平台无关的 system_bin 等
│   │       │   ├── macos.rs
│   │       │   ├── windows.rs
│   │       │   └── kill.rs         # 原 src-tauri/src/platform.rs
│   │       ├── whitelist.rs        # 值类型 Whitelist（不再是全局 static）
│   │       ├── paths.rs            # 自解析目录，不依赖 tauri
│   │       └── reasons.rs          # ← 新增：ReasonCode → zh/en 文案，单一真相源
│   └── portreaper-cli/
│       ├── Cargo.toml              # 依赖 portreaper-core + 一个极小的参数解析
│       └── src/main.rs             # scan / kill / whitelist / reasons 子命令
├── src-tauri/                      # 瘦身后：只剩 GUI 壳（~700 行）
│   ├── Cargo.toml                  # 依赖 portreaper-core + tauri
│   ├── tauri.conf.json             # bundle 里附带 CLI 二进制（见 §7）
│   └── src/
│       ├── main.rs
│       ├── lib.rs                  # builder / 托盘 / 菜单 / 窗口生命周期
│       ├── commands.rs             # #[tauri::command] 薄封装
│       ├── tray.rs                 # ← 从 lib.rs 摘出（托盘与菜单构建）
│       └── app_paths.rs            # AppHandle → core::paths 的桥接 + 一致性断言
├── src/                            # 桌面 GUI 前端（结构不变）
│   ├── model.ts                    # 只留纯逻辑，类型改为从 contracts/ 导入
│   └── ...
├── contracts/
│   ├── process-entry.d.ts          # ← ts-rs 从 Rust 生成，GUI 与 Raycast 共用
│   └── SCHEMA.md                   # 契约版本与兼容策略
├── integrations/
│   └── raycast/                    # Raycast 扩展（独立 TS 包，不进 pnpm workspace 主构建）
│       ├── package.json
│       └── src/
│           ├── find-cli.ts         # 二进制发现阶梯
│           ├── scan.ts             # spawn + 解析 + schemaVersion 校验
│           └── search-ports.tsx    # List UI + kill / 星标 Action
├── scripts/                        # 守卫脚本（新增两个，见 §6）
└── docs/
```

**为什么 workspace 根放仓库根**：`crates/` 与 `src-tauri/` 是平级的兄弟 crate，只有根 workspace 能自然表达。代价是 `target/` **和 `Cargo.lock`** 都从 `src-tauri/` 上移到仓库根，牵动五处（步骤 1 已全部处理）：

- `.gitignore`（根新增 `/target/`；`src-tauri/.gitignore` 的那条**要留着**，见下）；
- 两个 workflow 的 `Swatinem/rust-cache` 键 `". -> target"`；
- `release.yml` 的 dmg 后处理产物路径；
- `scripts/bump-version.mjs` 的 `CARGO_LOCK`（`Cargo.toml` 仍在 `src-tauri/` —— 这个不对称是刻意的：lockfile 属于整个 workspace，版本号属于应用 crate）；
- 所有 `cargo` 调用从 `--manifest-path src-tauri/Cargo.toml` 改成 `--all` / `--workspace`。**这条最危险**：旧写法在 workspace 下依然「成功」，只是把引擎整个跳过 —— 门禁绿着，而判定逻辑一行没查。

**踩到的坑：`src-tauri/.gitignore` 的 `/target/` 不能删。** oxfmt 按 gitignore 决定扫描范围；删掉那行后，拆分前遗留的 `src-tauri/target/`（本机 5.2 GB）立刻涌进 `vp check`，它开始格式化 cargo 的 fingerprint JSON。规则保留 + 清掉遗留目录，两件都要做。

## 5. core 的公开 API 设计

拆分不是「原样搬文件」。有五处契约要在这次一并改对，否则第二个前端会把现有的隐式约定复制一遍。

### 5.1 `Scanner` 持有平台状态，取代全局 static

Windows 的 CPU 百分比依赖 `sysinfo::System` 两次刷新之间的采样区间，现在靠全局 `Lazy<Mutex<System>>` + GUI 每 2 秒轮询天然提供。**CLI 冷启动只刷新一次，CPU 会恒为 0%** —— 不处理的话 Raycast 上 Windows 用户看到的全是 0。

```rust
pub struct Scanner { /* macOS: 空；Windows: sysinfo::System */ }

impl Scanner {
    pub fn new() -> Self;
    /// 一次扫描。连续调用之间的间隔即 Windows 的 CPU 采样区间。
    pub fn scan(&mut self, whitelist: &[String]) -> Vec<ProcessEntry>;
}

/// 一次性调用者（CLI / 脚本）的便捷入口：内部按需做双采样。
pub fn scan_once(whitelist: &[String], cpu: CpuSampling) -> Vec<ProcessEntry>;

pub enum CpuSampling {
    /// 跳过采样（最快，Windows 的 cpu_percent 为 0）
    Skip,
    /// 采样一次，间隔 d（推荐 200ms）；macOS 走 ps pcpu，不受影响
    Interval(Duration),
}
```

GUI 持有一个长驻 `Scanner`，语义与今天完全一致。全局 static 消失，测试也不再互相踩状态。

### 5.2 错误从字符串前缀改为枚举

现在 kill 失败靠 `"ERR_PID_REUSED: ..."` 这种前缀字符串，前端 `startsWith` 解析。加一个消费者就要再写一份解析，且没有任何编译期保护。

```rust
#[derive(Debug, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum KillError {
    IdentityUnknown,
    ProcessGone,
    PidReused,
    AccessDenied,
    /// 操作系统原文，无语义
    Os { message: String },
}
```

L1 负责翻译：`src-tauri` 序列化给前端（前端按 `code` 分支，不再 `startsWith`）；CLI 映射成稳定 exit code + stderr JSON。**`ERR_*` 前缀的字符串形态在 GUI IPC 边界上保留一个版本**作为过渡，避免前端与后端必须同一次发版。

### 5.3 白名单：值类型 + 显式路径

```rust
pub struct Whitelist { entries: Vec<String>, path: PathBuf }

impl Whitelist {
    /// 损坏文件备份为 .corrupt 的既有行为保留
    pub fn load(path: PathBuf) -> Self;
    pub fn entries(&self) -> &[String];
    /// 原子写（同目录 .tmp + rename）+ 失败回滚，行为原样保留
    pub fn add(&mut self, key: String) -> Result<(), WhitelistError>;
    pub fn remove(&mut self, key: &str) -> Result<(), WhitelistError>;
}
```

GUI 在 `commands.rs` 里包一层 `Lazy<Mutex<Whitelist>>`（含毒化恢复），语义不变；CLI 每次进程内 load / mutate / save。`whitelist_key` 的推导规则（`exe_path` 仅在含路径分隔符时使用，否则回退全命令行）随 core 走，`src/model.ts` 与 Raycast 都不再各自实现 —— 星标去重需要它时，从 CLI 输出里读现成的 `whitelistKey` 字段，**由 core 直接产出**。

### 5.4 路径自解析 —— 这是整件事的成败点

用户在 Raycast 里加的星标，GUI 下一轮扫描必须立刻看见。这要求 core 算出的目录与 Tauri 算的**逐字节相同**。

注意不能直接用 `directories::ProjectDirs`：Tauri 的 `app_config_dir()` 在 macOS 是 `~/Library/Application Support/{identifier}`、Windows 是 `%APPDATA%\{identifier}`，而 `ProjectDirs` 会插入 `{org}/{app}/config` 之类的额外层级。**必须自己按 identifier 拼**：

```rust
pub const APP_IDENTIFIER: &str = "com.fhf.portreaper";
pub fn config_dir() -> Option<PathBuf>;   // 含 cfg(debug_assertions) 的 dev/ 隔离
pub fn data_dir() -> Option<PathBuf>;
pub fn log_dir() -> Option<PathBuf>;
pub fn cache_dir() -> Option<PathBuf>;
pub fn temp_dir() -> Option<PathBuf>;
pub fn env_label() -> &'static str;
```

`dev/` 子目录的编译期隔离语义原样保留：CLI 是 release 编译 → 指向 prod 目录 → 与安装版 GUI 对齐；`cargo run` 出来的 debug CLI 指向 `dev/`，与 `pnpm tauri dev` 对齐。这个巧合是好事，写进注释别让人后来"修"掉。

防漂移靠 `src-tauri` 侧的一致性测试钉死（见 §6）。

### 5.5 理由文案 —— ~~下沉到 core~~ **方案已推翻，留在 i18n.ts**

> **实施时推翻了原设计。** 原方案基于「Raycast 要重写一遍翻译」这个前提，而这个
> 前提是错的：Raycast 扩展住在**同一个仓库**（`integrations/raycast/`），可以直接
> `import` `src/i18n.ts`。把文案再复制一份进 Rust，等于凭空造出第二份真相源和
> 第二条漂移路径，还要新写一套守卫去看住它 —— 净负收益。
>
> 现行分工：**引擎只输出机器码**（`ReasonCode` / `Confidence` 的 snake_case），
> 文案是「表达」，属于前端。`check-reason-parity.mjs` 已经保证 Rust 枚举 ↔
> `i18n.ts` ↔ `model.ts` 三方齐全，新增消费者不改变这个闭环。
>
> 只有当出现**非 TypeScript 的前端**（shell 脚本、Alfred workflow）时才需要重开此题，
> 届时正确的做法多半也是让 CLI 出一个 `reasons --json`，而不是在 Rust 里写死双语文案。
>
> 以下为已作废的原设计，保留以便追溯决策过程。

~~`reasons.rs` 成为 ReasonCode 文案的唯一真相源：~~

```rust
pub enum Lang { Zh, En }
pub struct ReasonText { pub label: &'static str, pub tip: &'static str, pub story: Option<&'static str> }
pub fn reason_text(code: ReasonCode, lang: Lang) -> ReasonText;
pub fn verdict_text(c: Confidence, lang: Lang) -> Option<&'static str>;
/// 全量导出，供 CLI `reasons --json` 喂给任意前端
pub fn all_reason_texts(lang: Lang) -> BTreeMap<String, ReasonText>;
```

桌面前端可以继续用 `src/i18n.ts`（它还有大量 UI 文案），但 reason 系列的键改由生成器从 core 导出，parity 脚本升级为三方校验。Raycast 直接吃 `portreaper-cli reasons --json`，零翻译维护。

## 6. 契约与防漂移守卫

本项目的既有风格是「每个隐式约定都配一个会响的守卫」。多一个消费者，守卫必须跟着扩。

| 守卫 | 状态 | 作用 |
|---|---|---|
| `check-reason-parity.mjs` | **升级** | 从「Rust enum ↔ i18n.ts ↔ model.ts」扩成四方：再加 `core/reasons.rs` 与 Raycast 的消费点 |
| `check-paths-parity`（新） | **新增** | `src-tauri` 的单测：断言 `core::paths::config_dir()` == `app.path().app_config_dir()` 加 `dev/` 后的结果，五个目录逐一比。路径分家 = 白名单分家，必须编译期/测试期就炸 |
| `contracts/process-entry.d.ts` | **新增** | ts-rs 从 `ProcessEntry` 派生生成（dev-dependency，`cargo test` 时导出）；CI 校验「生成结果与提交版本一致」。取代 `src/model.ts` 的手工镜像 |
| `schemaVersion` | **新增** | CLI JSON 输出的顶层字段。Raycast 读到不认识的大版本 → 提示升级，而不是渲染出错乱的行 |
| `check-release-assets.mjs` | **扩展** | 资产清单加上随包分发的 CLI 二进制 |

`schemaVersion` 的兼容策略写进 `contracts/SCHEMA.md`：主版本号只在字段删除 / 语义变更时递增；新增可选字段不递增。

## 7. Raycast 适配层

### 契约形态

```bash
portreaper-cli scan --json [--no-orphans] [--cpu-sampling=skip|200ms]
portreaper-cli kill <pid> --start-unix <token> [--force]
portreaper-cli whitelist add|remove|list <key>
portreaper-cli reasons --json --lang=zh|en
```

`scan --json` 输出：

```json
{
  "schemaVersion": 1,
  "scannedAt": 1754380800,
  "platform": "macos",
  "entries": [ /* ProcessEntry[]，字段与 GUI 前端完全一致 */ ]
}
```

**kill 的安全性天然成立**：core 的身份令牌是 fail-closed 的 —— 没有 `--start-unix` 直接拒绝。这意味着 CLI 无法「盲杀」，任何调用方都必须先 scan 拿到令牌，PID 复用防护自动覆盖到所有前端，不需要在 CLI 层再加确认逻辑。这是既有设计白送的红利，别在 CLI 上加 `--yes` 之类的旁路。

### 二进制发现阶梯

Raycast 扩展按顺序找：

1. `$PORTREAPER_CLI`（用户显式覆盖 / 开发调试）
2. `/Applications/Portreaper.app/Contents/MacOS/portreaper-cli`（随 dmg 分发，主路径）
3. `PATH` 中的 `portreaper-cli`（brew tap / 手工安装）
4. 都没有 → 渲染一个引导页，给下载链接，不报错崩溃

主路径选「随 `.app` 附带」：用户装了桌面版就自动具备 Raycast 能力，无需第二次安装；`tauri.conf.json` 的 `bundle.externalBin`（或 resources）负责把二进制塞进 `.app`，release 工作流的资产校验跟着扩。

### 扩展形态

一个 `search-ports` 命令：`<List>` 渲染分组（疑似 / 健康 / 星标），行内展示端口、置信度、主判定故事；`ActionPanel` 提供 Terminate（`Action.Style.Destructive` + `confirmAlert`）、Force kill、加星/取消星、复制 PID、Reveal in Finder。语言跟随扩展 preference，文案全部来自 `reasons --json`。

**冷启动性能**：`scan --json` 约 100ms（macOS 主要花在 lsof）。Raycast 交互可接受，无需守护进程。

## 8. 迁移路线

分五步，每步都能独立跑通全套门禁并单独发版。**不要合并成一个大 PR** —— 这次动的是全项目最核心的 5000 行判定逻辑，回归窗口必须小。

**步骤 0 — 前置**：当前工作区有未提交改动（`scanner/` 四个文件、`src/components/` 未跟踪、四份文档），先落定提交，让拆分 diff 纯净可读。

**步骤 1 — 机械搬迁（零逻辑变更）✅ 已完成**
建根 `Cargo.toml` workspace；`scanner/` + `platform.rs` 原样搬进 `crates/portreaper-core`；`src-tauri` 改为依赖它。判定代码一行不改。

实测记录：`cargo test --workspace` 72 passed（core 71 + shell 1，与拆分前逐个对齐，2 个 live smoke 仍 ignored）；`cargo fmt --all --check` / `clippy --workspace -D warnings` / 前端 28 passed / 四个守卫脚本全绿；`pnpm tauri build` 实跑通过，产物落在 `target/release/bundle/`，证实 tauri-action 的路径假设成立。`serde_json` 从主依赖降为 core 的 dev-dependency（只有 classify 的 serde 键名断言用它 —— 引擎自身不产出 JSON）。

`src-tauri` 侧 `sysinfo` / `windows` 依赖一并移走：GUI 壳不该直接碰平台 API。

**步骤 2 — 解开三处 GUI 耦合 ✅ 已完成**
`paths.rs` 去 Tauri 化；`whitelist.rs` 改值类型，GUI 侧包 static；`KillError` 枚举化，GUI IPC 保留 `ERR_*` 字符串兼容层。

一致性保障做成了**两层**（原计划只有单测一层，实施时发现单测覆盖不到真正危险的那一半）：

- 静态层 `scripts/check-paths-parity.mjs` —— 只查 `APP_IDENTIFIER` 常量与 `tauri.conf.json` 是否一致，不需要启动应用，进 CI 与 pre-push；
- 运行时层 `src-tauri/src/paths.rs::assert_matches_tauri` —— 逐一比对四个目录与 `app.path().app_*_dir()`，debug 下 panic、release 下记 `log::error!`。**放弃了原计划的 mock-app 单测**：Tauri 的路径解析需要活的 AppHandle，而 mock app 会拉起窗口，在 headless CI 上并不可靠；真实启动是唯一能同时拿到两侧答案的地方。

实测：`cargo run -p portreaper`（debug）启动无 panic，目录落在 `~/Library/Application Support/com.fhf.portreaper/dev` 与 `~/Library/Logs/com.fhf.portreaper/dev`，与拆分前一致。`cargo test --workspace` 82 passed（core 71 → 82：paths 4 + whitelist 4 + KillError 契约 3）。

> **计划调整**：原定在本步顺带做的 `scanner/mod.rs`（1681 行）细分**推迟到步骤 3 之后**。步骤 3 的 `Scanner` 结构体会重写 `scan()` 的入口与 Windows 侧的 `System` 持有方式，先拆再改等于同一块代码动两遍、review 两遍。拆分本身是纯代码组织，不阻塞任何前端。

**步骤 3 — 契约化**
`Scanner` + `CpuSampling`；ts-rs 生成 `contracts/process-entry.d.ts`，`src/model.ts` 改为导入类型、只留纯函数；`reasons.rs` + parity 脚本升级。

**步骤 4 — CLI ✅ 已完成（分发方式另议，见下）**
`portreaper-cli` 四个子命令，手写参数解析（不引入 clap —— 这个二进制要随 `.app` 分发，每个依赖都进用户的下载包；四个子命令手写约 100 行，还能给出贴合语义的错误信息）。

实测（本机真实进程）：
- `scan` 列出 13 行，揪出 2 个真实的 `http.server · Python` 残留（confirmed，reasons = `ppid1_orphan` + `dev_server_keyword` + `duplicate_dev_server`）；
- `kill` 三条路径逐一验证：缺 `--start-unix` → exit 2 + 解释为什么它是强制的；错误令牌 → exit 1 + stderr `{"code":"pid_reused"}`；正确令牌 → 进程真正终止；
- `whitelist add` 落盘到 `~/Library/Application Support/com.fhf.portreaper/dev/whitelist.json` —— **与桌面版同一个文件**，这是整次拆分最关键的一条证明。

实测抓到一个契约 bug：外层 `ScanReport` 起初用了 `rename_all = "camelCase"`，而 `entries` 里的 `ProcessEntry` 是引擎的 serde 输出（snake_case，`src/model.ts` 镜像的正是它）—— 同一份 JSON 两种命名风格。已统一为 snake_case，附带好处是 Raycast 可以直接复用 `src/model.ts` 的类型。

> **未做：随 `.app` 分发。** Tauri 的 `externalBin` / `bundle.resources` 都要求文件在 **dev 时也存在**，会给日常 `pnpm tauri dev` 加一道「必须先构建 CLI」的脆弱前置；而完整 release 流程本地无法彩排，改坏了只有发版当天才知道。故本次不动打包，改由 Raycast 扩展实现完整的二进制发现阶梯（含引导页）。待某次真机验证 release 流程时再补。

**步骤 5 — Raycast 扩展**
`integrations/raycast/`。此时 core 与契约已稳定，扩展是纯 TS 工作，不再触碰 Rust。

## 9. 风险清单

| 风险 | 缓解 |
|---|---|
| `target/` 与 `Cargo.lock` 迁移打断 CI 缓存、release 产物路径与版本脚本 | 步骤 1 已改全五处并本地实跑 `tauri build`；**CI/release 只能在真机验证** —— 合并后单独发一次 tag 走完整 release 流程再继续步骤 2 |
| 旧写法 `--manifest-path src-tauri/Cargo.toml` 在 workspace 下静默跳过引擎 | 所有门禁（CI、pre-push、文档命令清单）已改 `--all`/`--workspace`；新增 cargo 步骤时必须同样处理 |
| core 与 Tauri 的目录算法漂移 → 白名单分家 | `check-paths-parity` 单测，五个目录逐一断言 |
| Windows 无手工 QA，拆分放大回归面 | 步骤 1 严格零逻辑变更；`sysinfo::System` 从 static 改 `Scanner` 字段是 Windows 侧唯一实质改动，单独一个 commit 便于回滚 |
| CLI 让 kill 能力脚本化 | 身份令牌 fail-closed 已强制「先 scan 后 kill」，不加旁路即可 |
| 契约新增消费者后 i18n 漂移 | reason 文案单一真相源 + 四方 parity 守卫 |
| 拆分期间 `docs/KNOWN-GAPS.md` 的 Gap 修复与重构冲突 | 拆分期间冻结 `classify.rs` / `identify.rs` 的功能改动，只做搬迁 |
