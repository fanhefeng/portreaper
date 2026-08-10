# KNOWN-GAPS.md

已知检出盲区的案例存档。每条记录一个**真实发生过**的漏报场景 —— 有的当前仍是
盲区，有的已经修掉（如 Gap 1，v0.7.2 落地）：现场数据、逐层追踪到具体放行的
那一行、为什么它不是一个"顺手修掉"的 bug、以及动手时必须同时满足的约束。
每条都在标题或开头标明**当前状态**，已修的保留下来是因为约束仍然成立 ——
它们解释了现行实现为什么长这样。

写在这里的都不是待办清单 —— 是动手前必须读完的背景。判定逻辑的每一条豁免
都是某次误报事故换来的，删豁免比加豁免容易得多。

---

## Gap 1 — `/Applications` 里的应用跑出来的 headless 自动化实例

**状态**：**已修复**（2026-08-03，按下方方向 A + A2 + B 实施；真机复现验证过）
**发现日期**：2026-08-02
**平台**：macOS（Windows 同理，见文末）
**影响**：漏报。一个空转 7 小时、单核 100% 占用的进程树，扫描器全程判定"清白"。

> **修复后仍要读完本篇**：下面记录的每一条约束都还在生效，是修复的形状本身。
> 尤其是 A2 的实测反例 —— 那是**唯一**能把这次修复变成误杀事故的场景。
> 实现要点见文末「修复实现（2026-08-03）」。

### 现场

用户反馈 MacBook 风扇长时间高转。`ps` 抓到：

```
  PID  %CPU %MEM     ELAPSED COMM
64841  99.2  0.3    07:06:43 Google Chrome Helper     ← 满核 7 小时
  590  49.2  0.5 21-11:30:06 WindowServer
```

顺父进程查上去，是一棵孤儿进程树：

```
PID 64834  /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
           --headless=new --disable-gpu --hide-scrollbars --no-first-run
           --user-data-dir=/private/tmp/claude-501/<session-id>/scratchpad/cprof8/
           --remote-debugging-port=9339 about:blank
           PPID = 1        ← 父进程已死，被 launchd 收养
           CPU  ≈ 0%
           监听 127.0.0.1:9339 (LISTEN)   ← lsof 可见

  └─ PID 64841  .../Google Chrome Helper.app/.../Google Chrome Helper
                --type=gpu-process --headless=new --use-gl=disabled
                --user-data-dir=<同上>
                PPID = 64834   ← 父进程活着
                CPU  = 99.2%   ← 烧 CPU 的是它
                不监听任何端口
```

来源：某次 Claude Code 会话通过 chrome-devtools MCP 启动的 headless Chrome，
会话结束后没被回收。`--user-data-dir` 指向 `/private/tmp` 下的一次性目录，
`--remote-debugging-port` 是自动化调试端口 —— 形态上是彻头彻尾的开发残留，
只是宿主可执行文件恰好住在 `/Applications`。

复现（不依赖 Claude Code，任何自动化框架同理）：

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --user-data-dir=/tmp/gap1-repro \
  --remote-debugging-port=9339 about:blank &
disown            # 制造 ppid=1 孤儿
```

### 为什么两条扫描路径都没接住

`scan()` 有两条路径（`scanner/mod.rs`），这个案例在两条上各自出局，原因不同。

#### 路径一：监听者（`mod.rs:100-126`）— 主进程 64834 进来了，但被硬豁免清白放行

64834 监听 9339，`lsof` 抓得到，正常进入 `build_entry`。孤儿信号也确实成立：
`macos.rs:60` 的 `direct_orphan` 看到 `ppid == 1`，会返回 `Ppid1Orphan`。

但它走不到那一步。`mod.rs:237`：

```rust
let exe_is_standard_install = app_category == "installed-app"
    || app_category == "system"
    || (platform_impl::is_standard_install_path(&exe_path) && app_category != "dev-script");
```

exe 是 `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`
→ `identify_app` 的路径阶梯归为 `installed-app` → 该标志为 `true`。

于是 `classify.rs:186`：

```rust
// ---- 2. 硬豁免（顺序即优先级，先于一切正向信号）----
if s.exe_is_standard_install {
    return Verdict::clear(vec![ReasonCode::InstalledApp]);
}
```

**在第 3 步收集任何正向信号之前就 return 了**。`Ppid1Orphan` 连被算出来的机会
都没有（`direct_orphan` 虽在 `mod.rs:245` 构造 snapshot 时已求值，但 verdict
里被整体丢弃）。

结果：这一行**会出现在列表里**（所有监听者都列出），显示为 9339 端口上的
Google Chrome，`confidence = none`，理由 `InstalledApp`。不标红、不计入托盘
计数、不进一键清扫。用户扫一眼列表，看到的是"Chrome 开着个端口"，
和日常那个 Chrome 无从区分。

#### 路径二：无端口孤儿（`mod.rs:128-170`）— 真凶 64841 在预闸就被跳过

v0.6.0 加的第二条路径本来正是为这类"不占端口的 dev 残留"设计的。但它的纳入
门槛刻意比监听者更严 —— `mod.rs:141-151`：

```rust
// 廉价预闸（不回溯父链）：dev-like 是孤儿纳入的硬门槛，非 dev 进程直接跳过，
// 避免对全进程表（数百个）逐个做 build_parent_chain 的父链回溯。
let identity = platform_impl::identify_app(&meta.full_command, &command, &meta.exe_path);
let dev_like = is_dev_server(&meta.full_command)
    || is_dev_server(&command)
    || identity.1 == "dev-script";
if !dev_like {
    continue;
}
```

64841 的命令行是
`Google Chrome Helper --type=gpu-process --headless=new --use-gl=disabled ...`。

- `DEV_SERVER_SUBSTRINGS`（`classify.rs:77`）收了 `electron`、`tauri`、`vite`……
  **没有 `chrome` / `headless` / `puppeteer` / `playwright`**。
- `DEV_SERVER_TOKENS`（`classify.rs:130`）是 `node`/`python`/`deno`…… 按 token
  基名精确匹配，`Chrome`、`Helper`、`--type=gpu-process` 都不命中。
- `identify_app` 归 `installed-app`，不是 `dev-script`。

`dev_like = false` → `continue`。**在回溯父链之前就出局**，从来没进过 `build_entry`。

即使放宽这道预闸，它后面还有两道会拦住：

1. 它 `ppid = 64834`，父进程活着，`direct_orphan` 不成立；
2. 父链 64841 → 64834 → launchd，而 `macos.rs:90` 的 `is_chain_stopper`
   在任何含 `.app/` 的路径处停下 —— 64834 就是 `.app/`，链在那里停住，
   `chain_terminates_at_init` 为假；
3. 就算前两条都过了，它自己的 exe 也在 `/Applications` 下，一样吃 `InstalledApp` 硬豁免。

#### 第三重：CPU 从来不是判定依据

`ProcessEntry`（`scanner/model.rs:31`）有 `cpu_percent` 字段，但那**纯粹是展示用**。
喂给 `classify` 的 `ProcessSnapshot`（`model.rs:83-107`）里**没有任何 CPU/能耗信号**。

这是有意的设计 —— 判定的语义是"这是不是无人认领的残留"，不是"这是不是在费电"。
一个健康的 `vite build` 也能吃满核。但代价就是本案：一个空转 7 小时的 100% 占用
进程，在判定链路上和一个 0% 的闲置进程完全等价，没有任何一处能拉开它们的差距。

### 为什么不能直接把 `/Applications` 豁免删掉

这条豁免不是偷懒，是产品的防误杀底线。Chrome、Docker Desktop、VS Code、
Slack 都监听端口，很多还常态 `ppid=1`。删掉豁免，日常桌面应用会被整片标红，
一键清扫直接变成事故。

仓库里已经有一个测试**明确锁死了这个行为** —— `mod.rs:1050`：

```rust
/// 对照（防误杀）：/Applications 里的 VS Code（也是 Electron）即便 ppid=1
/// 也必须被 installed-app 豁免 —— node_modules 信号不得波及真安装的应用。
#[test]
fn installed_electron_app_in_applications_is_exempt() {
    let exe = "/Applications/Visual Studio Code.app/Contents/MacOS/Electron";
    // ... ppid=1
    assert!(!raw_suspect, "已安装应用即便 ppid=1 也不是孤儿嫌疑");
}
```

**任何修复都必须让这个测试继续通过。** 它和本 Gap 是同一枚硬币的两面：
VS Code 的 Electron 和 headless Chrome 都是 `/Applications` 下的 `ppid=1` 进程，
区分它们的信息**不在路径里，只在命令行里**。

### 已有的先例：`dev-script` 对路径规则的例外

同样的"混血身份"问题项目里已经解过一次。CLAUDE.md 的不变量清单写着：

> **Exception to the path rule:** `dev-script` category is *not* exempted by the
> interpreter's exe path — a script runtime's identity is its *script*.

`/usr/bin/python3 app.py` 里，`/usr/bin/` 是系统路径，但真正的身份是 `app.py`，
所以 `identify_app` 先判脚本、`mod.rs:239` 用 `&& app_category != "dev-script"`
把它从路径豁免里摘出来。

**本 Gap 是完全对称的一例**：`--headless --remote-debugging-port` 跑起来的
Chrome，浏览器可执行文件在 `/Applications`，但真正的身份是"一次性自动化实例"。
修复的形状应该照抄这个先例 —— 加一个由**命令行特征**决定的新类别，把它从
路径豁免里摘出来，而不是去动路径豁免本身。

### 修复方向

按侵入性从小到大。方向 A 最贴合上面的先例，建议先做。

#### A. 新增 `automation-instance` 类别，作为路径豁免的第二个例外

判据：命令行同时满足
- 含 `--headless`（或 `--headless=new`），**且**
- 含 `--remote-debugging-port=` 或 `--user-data-dir=<临时目录>`
  （`/tmp`、`/private/tmp`、`/private/var/folders/`、Windows `%TEMP%`）

> ⚠️ **命令行特征单独用会误报 —— 必须叠加下面 A2 的存活性否决。**
> 写这份文档 20 分钟后就撞到了反例，见「A2」。**`--headless` 是必要条件，
> 不可省**：省掉它、只靠"调试端口 + 临时 profile"会直接命中所有有头的
> 自动化浏览器实例。

命中后 `identify_app` 归 `automation-instance`，`mod.rs:237`
的条件相应改成排除它：

```rust
&& app_category != "dev-script"
&& app_category != "automation-instance"
```

同时把它并入 `dev_like`（`classify.rs` 的 `dev_category` 或预闸），让第二条
路径也能捞到同族的无端口子进程。

注意 `/private/var/folders/` 目前在 `macos.rs:41` 的 `is_standard_install_path`
里是**豁免项**（为 App Translocation 让路），临时 `--user-data-dir` 判定要按
命令行参数值来看，别和 exe 路径判定混用同一个函数。

**新增 ReasonCode 的连带义务**（CI 会卡）：
- `classify.rs` 加枚举变体（正向信号区）
- `src/model.ts` 归入 `REASON_PRIORITY`（正向）或 `EXEMPT_REASONS`（豁免）
- `src/i18n.ts` 补 `reason.*` / `reasonTip.*` / `story.*` 的 zh+en
- `node scripts/check-reason-parity.mjs` 必须通过

#### A2. 存活性否决：调试端口上有无 ESTABLISHED 连接（**方向 A 的必要组成**）

本文档写完 20 分钟内就撞到了方向 A 的反例，实测记录：

```
PID 397  ppid=1   CPU≈0%（其 gpu-process 子进程 39%）  ELAPSED 10:39
         /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
         --remote-debugging-port=9222
         --user-data-dir=/private/tmp/claude-501/<session>/scratchpad/chrome-profile
         --no-first-run --no-default-browser-check
```

`ppid=1`、临时目录 profile、调试端口 —— **方向 A 的判据全中，但它是活的**：

```
$ lsof -nP -iTCP:9222
Google  397    TCP 127.0.0.1:9222->127.0.0.1:54191 (ESTABLISHED)
node    99141  TCP 127.0.0.1:54191->127.0.0.1:9222 (ESTABLISHED)
              └─ 99141 chrome-devtools-mcp
                 └─ 99094 npm exec chrome-devtools-mcp@latest --autoConnect
                    └─ 99081 claude.exe   ← 活跃会话，正在用这个浏览器
```

它与 Gap 1 主案的差别**不在命令行里**（两者命令行几乎同构），只在两处：

| | 主案 64834（残留） | 反例 397（活跃） |
|---|---|---|
| `--headless` | 有 | **无**（有窗口实例） |
| 调试端口连接 | 仅 LISTEN，**零 ESTABLISHED** | **有 ESTABLISHED**，对端是 MCP 客户端 |
| 子树 CPU | 99% 持续 7 小时 | 39%，10 分钟，随交互波动 |

结论：**"有没有客户端连着"是区分死活最强的证据，强于任何命令行特征。**
一个自动化浏览器实例的存在意义就是被客户端驱动；调试端口只 LISTEN、无人连接，
才是真正的"无人认领"。

实现要点：
- macOS 数据源：**只能是「候选 PID 非空时再取一次按 PID 限定的 ESTABLISHED
  查询」**（`lsof -a -p <pids> -iTCP -sTCP:ESTABLISHED -Fpn`），对**同一 PID 的
  同一端口**统计对端连接数。曾经考虑过的另一条路 —— 放宽主查询的
  `-sTCP:LISTEN` 过滤、把全部状态收下来自行分流 —— **已排除**：lsof 是本项目
  最贵的一次调用，让每台机器的每轮扫描都去拉全量连接表，只为服务一个通常为空
  的候选集，代价完全不成比例。正常机器上候选集为空，这次查询根本不发生。
- Windows 侧 `GetExtendedTcpTable` 本来就返回全部状态的连接表（现在只筛
  `MIB_TCP_STATE_LISTEN`），拿 ESTABLISHED 是纯过滤条件改动，零额外成本。
- 判定用法：作为 `automation-instance` 的**否决项**而非独立信号 ——
  有活跃连接 ⇒ 直接豁免（新增一个豁免类 ReasonCode，如 `DebuggerAttached`）。
  宁可漏报也不能误杀别人正在用的浏览器。
- 注意竞态：客户端可能瞬时断开重连。这条否决应当**只用于豁免、不用于升级置信度**，
  且建议配合 `GRACE_SECS` 式的缓冲，避免连接抖动导致行在"清白/嫌疑"间闪烁。

这条同时解释了为什么方向 C（高 CPU）不能单独用：反例 397 的子树也有 39%。

#### B. 把子树 CPU 汇总到被列出的那一行

本案的荒诞点在于：列表里那行主进程显示 ~0% CPU，而它子树里有个进程在 100%。
即便判定不改，UI 上把"整棵子树 CPU 合计"露出来，用户也能自己发现异常。

`ProcMeta` 里已有 `ppid` 和 `cpu_percent`，全进程表也已采集，聚合是纯内存计算，
无额外系统调用。只加展示字段，不进 `ProcessSnapshot`、不参与判定 —— 风险最低。

#### C. 把持续高 CPU 作为置信度的加权项（不作为独立正向信号）

**不要**让高 CPU 单独触发嫌疑 —— `vite build`、`tsc`、`cargo build` 都会满核，
误报会非常难看。可行的用法是：**已经有孤儿信号时**，用"长时间持续高 CPU"
把 `Likely` 提升到 `Confirmed`。

难点是"持续"需要跨轮次状态，而 `classify` 是纯函数、前端每 2 秒重新扫描。
要做得先在 `scan()` 外层维护一个 PID→采样历史的状态（注意 PID 复用，用
`start_unix` 一起做键），别把状态塞进纯函数。优先级最低。

### Windows 侧的对应情况

同一形态在 Windows 上一样漏，且理由平行：`is_standard_install_path` 的
`Program Files` / `SHGetKnownFolderPath` 阶梯把 `chrome.exe` 归 `installed-app`。
差别在孤儿信号 —— Windows 用 `ParentExited` / `PidSlotReused` 而非 `Ppid1Orphan`，
但同样被硬豁免挡在前面。方向 A 的命令行判据是跨平台的（`--headless` 等参数
两边一致），应当在 `identify.rs` 的共享层实现，而不是各写一份。

Windows 无本地 QA，改完按 `docs/TESTING-WINDOWS.md` 走验收。

### 动手前的检查清单

（全部已完成 —— 保留原文，是回归时要重新走一遍的清单）

- [x] `installed_electron_app_in_applications_is_exempt`（`mod.rs`）仍然通过
- [x] 新增 fixture 测试：`/Applications` 下带 `--headless` + `--remote-debugging-port`
      的 `ppid=1` 进程 → 判为嫌疑
      （`orphan_headless_automation_in_applications_is_detected`）
- [x] 新增对照 fixture：`/Applications` 下**不带**这些参数的 `ppid=1` Chrome → 仍豁免
      （`plain_chrome_in_applications_stays_exempt`）
- [x] 新增对照 fixture（A2 反例，实测案例）：`ppid=1` + 临时 `--user-data-dir`
      + `--remote-debugging-port` 但**无 `--headless`**、且调试端口有 ESTABLISHED
      连接 → **必须豁免**。这是真实活跃实例，误杀会打断用户正在跑的会话
      （`live_driven_browser_instance_is_never_flagged`，两道防线各断言一次）
- [x] `cargo test` + `cargo clippy --all-targets -- -D warnings`（双平台 target）
- [x] `node scripts/check-reason-parity.mjs`（新增了 2 个 ReasonCode）
- [x] `pnpm exec vp check` + `pnpm test` + `pnpm exec tsc --noEmit`
- [x] `cargo test live_scan -- --ignored --nocapture` 在真机上扫一遍，确认日常
      Chrome / VS Code / Docker Desktop 没有被新规则波及

---

## 修复实现（2026-08-03）

### 落地位置

| 部件 | 位置 |
|---|---|
| 命令行判据 | `identify.rs is_automation_instance`（**跨平台共享**，开关两平台逐字相同） |
| 新类别 | `automation-instance`（常量 `scanner::AUTOMATION_CATEGORY`），两侧 `identify_app` 阶梯 0b |
| 路径豁免例外 | `mod.rs build_entry` 的 `identity_beats_path`（与 `dev-script` 并列） |
| 存活性否决 | `classify.rs` 硬豁免第 2 位；证据由 `Collected::established_local_ports` 提供 |
| 新 ReasonCode | `AutomationInstance`（正向）/ `DebuggerAttached`（豁免），前端四键族已补齐 |
| 子树 CPU（方向 B） | `mod.rs fill_subtree_cpu` → `ProcessEntry::cpu_percent_tree`，详情面板 + 行内徽标 |

顺带收口的同族漏报（同一枚硬币的其它面）：

- **工具下载的浏览器 runtime**：`identify.rs is_dev_tool_runtime_path` 把原先只认
  `/node_modules/` 的判定泛化到 `ms-playwright` / `.cache/puppeteer` 等，并从
  macos.rs 提到共享层（Windows 侧此前完全没有这条，属静默漏报）。
- **自动化工具链关键字**：`chromedriver` / `geckodriver` / `playwright` /
  `puppeteer` / `selenium` / `cypress` … 进 `DEV_SERVER_SUBSTRINGS`，让 driver
  自身孤儿化后也能被第二条扫描路径捞到。

### 方向 C（持续高 CPU 加权）仍未做

理由未变：需要跨轮次状态，而 `classify` 是纯函数。方向 B 落地后，用户已能在
UI 上看见"子树在满核"这件事，C 的边际收益进一步降低。

### 修复后仍存在的残留风险（复审记录，2026-08-04）

两条已知的、有意留下的口子。**动这块代码前先读这两条**，它们解释了当前形状的边界。

#### 1. A2 建议的「连接抖动缓冲」没有实现

A2 结尾建议「配合 `GRACE_SECS` 式的缓冲，避免连接抖动导致行在清白/嫌疑间闪烁」——
没做，理由与方向 C 同源：缓冲需要跨轮次状态，而 `classify` 是纯函数。

实际暴露面比字面窄得多，需要**三个条件同时成立**：

- 实例是 `--headless` 的（有头的根本不进 `automation-instance`，A2 那个实测反例
  397 就是有头的）；
- 且 `ppid=1`（驱动进程已经死了或 disown 了 —— 父进程活着时孤儿信号根本不成立，
  连接怎么抖都无害）；
- 且客户端在两次调用之间断开重连（如 Playwright 用例之间）。

三条同时中的那一瞬，行会从「清白」跳成 `Confirmed`；用户恰在此刻点一键清扫才会
误杀。窗口窄、且需要用户主动操作，故接受。**若哪天要修，别把状态塞进 `classify`**
—— 在 `scan()` 外层用 `(pid, start_unix)` 做键维护「上次见到连接的时刻」，判定仍读
纯快照。

#### 2. 有头浏览器的 helper 子进程孤儿化后不会被检出

`--headless` 是 `is_automation_instance` 的必要条件（A2 的全部意义），代价就是：
一个**有头**浏览器的 `--type=gpu-process` 子进程，若主进程死掉而它被收养成
`ppid=1`，第二条扫描路径的 dev-like 预闸不会放它进来。

这是**刻意的**，不是漏洞：放宽 `--headless` 会让判据命中用户此刻正在看的那个浏览器
窗口的全部子进程 —— 用漏报换误杀，方向完全错。且这种残留在实践中很少见：有头
浏览器的主进程死亡时通常会带走自己的 helper 树。

`mod.rs busy_helper_under_live_parent_is_not_listed_separately` 锁住的是相邻的另一条
边界（父进程健在时不单独列行）；这一条目前只有本文记录，没有夹具 —— 因为它断言的是
「不检出」，而能让它退化的改动（放宽必要条件）会先撞碎 A2 的那三个对照夹具。

### 真机验证记录（2026-08-03）

复现 → 检出 → 否决 → 恢复 → 清理，五步全部实跑：

```
基线                          12 个监听者，全部 conf=None（无误报）
起 headless Chrome + disown   ppid=1，:9339 LISTEN，零 ESTABLISHED
  → 扫描                      conf=Confirmed  automation-instance
                              reasons=[Ppid1Orphan, AutomationInstance]
                              label "Google Chrome · headless"
挂一个客户端到 :9339          127.0.0.1:9339->127.0.0.1:58505 (ESTABLISHED)
  → 扫描                      conf=None  reasons=[DebuggerAttached]   ← A2 否决生效
断开客户端
  → 扫描                      conf=Confirmed（恢复）
kill + 清理 profile           → 回到 12 个监听者，0 个 suspect
```

同一轮里 VS Code / Discord / QQ / WeChat / Warp / Zed 等日常应用的判定与基线
逐行一致，未被新规则波及。

> **上表的 reasons 一栏已按 2026-08-04 的修正就地更新**，与当天原始快照
> `[Ppid1Orphan, OrphanedChain, NonstandardPath, AutomationInstance]` 相比少了两条。
> 两条的消失各有原因，都**只影响详情面板的证据列表**，不影响检出与置信度
>（`conf=Confirmed` 与 label 与当天一致，五步流程逐步可复现）：
>
> - `NonstandardPath` —— 当时被无条件推入，于是 exe 明明住在 `/Applications` 的
>   headless Chrome 也被贴上「可执行文件不在标准安装位置」。「摘出路径豁免」被
>   错当成了「路径非标准」的同义词，而这恰是本 Gap 两个身份例外（`dev-script` /
>   `automation-instance`）的共同特征。现按路径事实取证，且事实谓词
>   （`is_conventional_install_path`）与豁免谓词（`is_standard_install_path`）
>   **分开实现** —— 后者刻意向 true 偏（收 `/private/var/folders/` 给 App
>   Translocation 让路、Windows 对读不到的空 exe 放行），拿它陈述事实，它每放宽
>   一次就多撒一次谎。这与 `identify.rs is_temp_dir_path` 的注释是同一条教训。
> - `OrphanedChain` —— 该行 ppid=1，`build_parent_chain` 第一次迭代就终止，一个
>   真实祖先都没走过，所以「链终止于 init」完全是 `Ppid1Orphan` 的同义反复。
>   现按「链有没有真的走过祖先」这个结构事实决定是否列出
>   （`ChainFlags::walked_real_ancestor`）；本体 ppid 正常、链走过 zsh→npm 才撞到
>   launchd 的那类行仍会照常列出，那里它是唯一的孤儿证据。

---

## Gap 2 — 挂起（stopped，`ps state` 含 `T`）的进程，温和终止形同无效

**状态：已修（2026-08-10，`crates/portreaper-core/src/platform.rs`）。** 本节保留
现场与推理过程，因为这类「syscall 返回 0 ≠ 目的达成」的错误极易以别的形态复发。

### 现场

用户报告：**从 Terminal 里启动的进程，在 Raycast 扩展里点终止没有任何反应。**

复现（可原样重跑）：

```bash
# 一个绑 :8799 且像 vite/node 一样捕获 SIGTERM 的监听者
perl -e '$SIG{TERM}=sub{exit 0}; use IO::Socket::INET;
         IO::Socket::INET->new(LocalAddr=>"127.0.0.1",LocalPort=>8799,
                               Proto=>"tcp",Listen=>5,ReuseAddr=>1) or die $!;
         sleep 600' &
kill -STOP $!          # 等价于用户在 Terminal 里按了 Ctrl-Z
portreaper-cli scan --json      # 取该行的 start_unix
portreaper-cli kill <pid> --start-unix <n>
echo $?                # 0 —— 而进程 state 仍是 TN，:8799 也没释放
```

### 为什么

POSIX 语义：**默认动作是终止**的信号（未安装 handler）由内核直接施加，停止态也
照杀不误 —— 所以拿 `sleep 300` 当样本永远测不出这个 bug。而**被捕获**的信号必须
由进程自己执行 handler，停止态的进程根本不运行，信号就一直挂在 pending 集里。
`kill(2)` 只报告「投递成功」，它对此一无所知。

node / vite / next / nodemon 全都注册 SIGTERM handler，正好落在这一格。进入 `T`
态的路径也不止 Ctrl-Z：后台作业读终端（SIGTTIN）、`stty tostop` 下写终端
（SIGTTOU）同样会停住 —— 而终端一旦关掉，就再没有人给它 SIGCONT 了。这恰恰是
本产品最想抓的那类残留：一个永远醒不过来、又占着端口的孤儿。

三层同时被这一个事实骗过：引擎（返回 `Ok(())`）、CLI（退出码 0）、两个前端
（绿色「已终止」）。**没有任何一层去看进程是不是真的没了。**

### 修法

1. **引擎**：身份探针从 `ps -o etime=` 改成 `ps -o etime=,state=`（同一次
   fork/exec，零额外开销），温和终止后若目标是 `T` 态就补一发 SIGCONT。
   顺序 TERM→CONT、只在确认停止时发、返回值一律忽略 —— 三条约束的理由写在
   CLAUDE.md 的 Kill path 一节与代码注释里。`force` 分支不发（SIGKILL 不可捕获）。
2. **两个前端**：终止后短时轮询确认 `(pid, start_unix)` 是否真的消失（~2.5s 上限），
   仍在则如实报告并给出强杀出口。这一层是通用兜底：SIGCONT 治的是停止态，
   而「装了 handler 却迟迟不退出」「被 supervisor 立刻重启」同样会让人看到
   「点了没反应」。
3. **UI**：`T` 态在两端都显式呈现（桌面版详情面板的原码 + 人话，Raycast 的
   `stopped` 徽标与详情说明），并在终止确认里说明「会唤醒它以便它自己收尾」——
   唤醒是一次用户可见的状态改变，不该不打招呼就做。

### 回归守卫

`platform::live_tests::kill_stopped_process_that_catches_sigterm`（`--ignored`，
`cargo test --workspace kill_stopped -- --ignored`）。它必须用 `try_wait` 而不是
阻塞 `wait`：回归时目标根本不会退出，阻塞等待会让整个测试挂死而不是响亮失败
（写这条测试时就先踩了一次，表现为超时而非断言失败）。

### 仍未做

- **被 supervisor 立刻重启**（nodemon / concurrently / turbo / cargo-watch /
  docker）：杀掉后同端口冒出新 PID，用户视角同样是「没反应」。引擎目前只认识
  launchd 与 pm2；「谁在托管它」属于判定形态的知识，要做就得住 core
  （`chain.rs ChainFlags` 旁扩 `supervisor: Option<SupervisorKind>`），并走
  `check-model-parity.mjs` 的三方字段同步。零契约成本的替代：终止后的确认扫描里，
  若同一端口被**另一个 PID** 持有就提示「有东西在重启它」。
- **fork 继承同一监听套接字**：父 bind 后 fork，`lsof` 同时列出两个 PID、同一个
  socket。杀掉被标记那行，端口纹丝不动，列表立刻又出现一个同名 confirmed 行。
  按设计这不是 duplicate（`mark_duplicates` 排除父子且要求端口不相交）。可选的
  低成本改法：`fill_subtree_cpu` 已在做带 `visited` 的子树 DFS，顺手产出
  `descendant_count`，确认框据此提示「还有 N 个子进程会活下来」。
