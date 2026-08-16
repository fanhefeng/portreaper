# 维护笔记（Raycast 扩展）

`integrations/raycast/README.md` 是**面向用户**的英文文档 —— Raycast Store 会原样
展示它，且只支持美式英语。本文件放维护者视角的决策记录。

**刻意放在 `docs/` 而不是扩展目录里**：`ray publish` 会把 `integrations/raycast/`
整个目录提交进 raycast/extensions 的 PR，一份中文维护文档混进公开 PR 既不合规
（Store 只收美式英语）也是噪音。扩展目录只留提交所需的文件。

## 开发

```bash
npm install            # 本目录独立于主仓库的 pnpm workspace
npm run typecheck      # tsc --noEmit（CI 的 macOS 腿会跑）
npm run lint           # ray lint —— 提交 Store 前跑一次
npm run dev            # ray develop（需要本机装有 Raycast）
npm run build          # ray build —— 提交 Store 前必须通过
npm run publish        # 自动 fork raycast/extensions 并开 PR
```

**为什么这里用 npm 而主仓库用 pnpm**：Raycast Store 要求扩展提交 `package-lock.json`
（官方 CI 用 npm 构建）。这是唯一的例外，不影响仓库其余部分。

**CI 覆盖**：`.github/workflows/ci.yml` 的 macOS 腿跑 `npm ci` + `npm run typecheck`。
不跑 `ray build`（runner 上没有 Raycast 运行时）。

## 依赖升级（没有 Dependabot，靠手跑）

```bash
npm outdated --prefix integrations/raycast    # 提交 Store 前必跑
```

`@raycast/api` 是这里唯一会实质漂移的依赖，而 Store 审核偏好较新的 API 版本。

**`npm audit` 的两条 low 一律不修**（2026-08-08 复核，结论按当前形态成立）：告警指向
`esbuild` 的「Windows 上跑开发服务器时可任意读文件」，它是 `@raycast/api` 的**传递
构建期**依赖。`npm audit fix --force` 给出的所谓修复是把 `@raycast/api` 从 1.104.24
**降级**到 1.104.9 —— 为一个构建工具的 Windows 开发服务器问题，降级官方 SDK，而本扩展
`platforms` 只有 macOS、分发形态里根本不跑那个 dev server。方向完全不成比例。上游修
要等 Raycast 自己抬 esbuild。**若哪天该告警升到 high/critical，或 esbuild 进入运行时
依赖，再重新评估** —— 别因为「audit 是红的」就顺手降级 SDK。

**为什么不交给 Dependabot**：给本目录配 npm 生态试过一次，每月必失败。Dependabot 的
pnpm 探测只看父目录有没有 `pnpm-lock.yaml` + `pnpm-workspace.yaml`，命中就判定
「workspace 子目录」并拒绝更新 —— 与本目录自身是不是规范的 npm 布局无关
（它甚至会拿仓库根的 vite-plus 来 `npm install` 这个目录）。完整错误与结论记在
`.github/dependabot.yml` 的头部注释里，别再加回来。

## portreaper-cli 的查找顺序

`src/cli.ts` 的 `resolveCliPath`，逐个 `existsSync` 探测；**不查 `PATH`** —— 只认下列固定路径：

1. 扩展偏好里的 `Portreaper CLI Path`（显式指定，优先级最高）
2. 扩展支持目录下自动下载并校验过的副本（首次使用时自动获取）
3. `/Applications/Portreaper.app/Contents/MacOS/portreaper-cli`（打包尚未落地，见下）
4. `~/.cargo/bin/portreaper-cli`

**没有 `<repo>/target/...` 这一档**，尽管开发时很想有：扩展跑在 Raycast 里，
`process.cwd()` 是 Raycast 的目录，仓库在哪儿无从得知；写死绝对路径又只对本机有效。
本机调试请走第 1 项（偏好里填 `target/release/portreaper-cli` 的绝对路径）。

全部落空时走 `src/install.ts` 自动下载 + SHA-256 校验（Store 政策明确要求
"Avoid asking users to perform additional downloads"，且下载必须带哈希校验）。
下载失败同样落到引导页（`DownloadFailedError`，见下），列出找过的位置。

> **随 `.app` 分发尚未实现。** Tauri 的 `externalBin` / `bundle.resources` 都要求文件在
> dev 时也存在，会给日常 `pnpm tauri dev` 加一道脆弱前置；且完整 release 流程无法本地
> 彩排。详见 `docs/ARCHITECTURE-CORE-SPLIT.md` 步骤 4。查找阶梯第 3 项已预留位置，
> 打包落地后无需改扩展。

## 为什么不装 ESLint / Prettier

本仓库的格式与 lint 统一由 Vite+ 工具链（`vp check` = oxfmt + oxlint）负责，扩展代码
也在其覆盖范围内。Raycast 官方模板默认带 ESLint + Prettier，但两者都**不是** Store 的
硬性要求 —— 未安装时 `ray lint` 只会 warn 一句并跳过格式检查，真正的硬指标
（`package.json` 字段、图标规格）照常校验，`ray build` 也照常通过。

装上它们的代价是实打实的：Prettier 与 oxfmt 功能重叠、换行风格不同，同管一批文件
会在 `vp check --fix` 和 `ray lint --fix` 之间来回改写，逼得整个目录必须从主仓库的
格式门禁里排除 —— 在一个把「格式门禁统一」当教训写进 CLAUDE.md 的仓库里凿一个飞地，
不划算。（另注：Prettier 是被 `@raycast/eslint-config` 当传递依赖拖进来的，
只卸 prettier 没用，得连 eslint 一起去掉才会真正消失。）

`lint` / `fix-lint` 脚本仍然保留：`ray lint` 不依赖 eslint 也能跑那些硬指标，
提交前应当过一遍。

**残余风险**：官方文档提到 lint 检查「之后也会通过 GitHub 自动检查跑一遍」。若
Store 的 CI 用它自带的 Prettier 检查代码风格，PR 可能被标记格式问题。届时的处置是
提交前单跑一次 `ray lint --fix`，而不是把这套工具链常驻进仓库。

## 类型配置的两处 TS7 陷阱

`tsconfig.json` 里有两条与主仓库同源的注记，改动前先读：

- `module` / `moduleResolution` 必须**成对**设为 `node16`。TS7 移除了 node10
  （旧写法 `module: commonjs` + `moduleResolution: node`），单改一个会报 TS5110。
  本包无 `"type": "module"`，故 node16 仍按 CommonJS 解析，与 `ray build` 的产物一致。
- `types: ["node"]` 必须显式声明。TS7 起 @types 不再自动包含，缺了它
  `child_process` / `fs` / `Buffer` / `fetch` 全部报 TS2591。

## 与桌面版共享状态

星标（白名单）写的是**同一个文件**（下方是 release 构建的路径，debug 另有一份，见后）：

```
~/Library/Application Support/com.fhf.portreaper/whitelist.json
```

由 `portreaper_core::paths` 保证 —— 两边调用的是同一个函数，桌面版启动时还会逐一比对
自解析结果与 Tauri 的解析，不一致就报警（`src-tauri/src/paths.rs assert_matches_tauri`）。

> debug 构建的 CLI 指向 `.../com.fhf.portreaper/dev/whitelist.json`，与
> `pnpm tauri dev` 配对；release 构建指向正式目录，与安装版配对。这是刻意设计，
> 不是 bug。

## 为什么理由显示成机器码

翻译属于「表达」，是前端的事，而桌面版的双语文案住在 `src/i18n.ts` —— 那个模块在顶层
访问 `localStorage` / `navigator`，Node 环境 import 不进来。与其为 Raycast 复制第二份
文案（第二份真相源 + 第二条漂移路径），不如诚实地显示引擎的原始判定码：本扩展的用户是
开发者，`ppid1_orphan` 比一句含糊的翻译更有信息量。

Store 只支持美式英语，故扩展本身是英文单语 —— 这一条与「不复制第二份 i18n」同向。

## 契约同步

`src/cli.ts` 里的 `ProcessEntry` / `ParentRef` 是引擎 serde 契约的**第三份镜像**
（另两份：`crates/portreaper-core/src/scanner/model.rs`、`src/model.ts`）。
三者的字段集由 `scripts/check-model-parity.mjs` 在 CI + pre-push 校验，漏改必翻红。

判定、`whitelist_key`、路径解析一律消费引擎输出，**绝不在 TS 侧重推** ——
这正是 core 拆分要消灭的失败模式（扩展曾自带一份 whitelist-key 规则）。

## 平台语义

Windows 上桌面版只给单个 Terminate 按钮（detached 控制台进程没有可靠的温和终止方式，
CLAUDE.md 钉死的产品决定）。扩展按 `ScanReport.platform` 对齐了这一点。

不过 `package.json` 的 `platforms` 目前只声明 `["macOS"]`：Windows 侧的 CLI 没有
人工 QA，发现阶梯里也含 `/Applications` 这类 macOS 路径。等 Windows 真机验收完成
（`docs/TESTING-WINDOWS.md`）再考虑放开。

## 真机验证状态（2026-08 更新）

`ray lint` 三项硬指标全绿（package.json / 图标 / metadata 截图）、`ray build`
distribution build 通过、`tsc --noEmit` 零错误。

**Raycast 内的 UI 已首次真机跑通**（`ray develop` 载入 Raycast Beta，无需 `ray login`），
实测覆盖：

| 场景 | 结果 |
|---|---|
| 扩展载入与命令启动 | ✅ Development 分组下可见并可启动 |
| 扫描与分组 | ✅ Suspects / Healthy 正确分区，嫌疑排在前 |
| 置信度分档 | ✅ 两个人造孤儿 dev server 均判为 `confirmed` |
| 详情面板 | ✅ 判定理由（`ppid1_orphan` / `nonstandard_path` / `dev_server_keyword`）、PID、类别、端口、运行时长、CPU 自身/子树、内存均有值 |
| 动作面板（⌘K） | ✅ Terminate / Force Kill 为红色破坏性样式；Star / Refresh / Toggle Details / Copy PID 就位 |
| 终止确认 | ✅ `confirmAlert` 弹出并显示「进程名 + PID + 端口」，Cancel / Terminate 双按钮 |
| 详情折叠（⌘D） | ✅ 列表转满宽，徽标完整显示 |
| 搜索过滤 | ✅ 按进程名/端口/PID 过滤生效 |
| 无端口孤儿 | ✅ 带「no port」徽标正常列出（实测样本：被孤儿化的 `ray develop`） |

**真机 QA 发现并已修复的 UI 瑕疵**：搜索过滤时分区标题的计数曾是**未过滤**的总数
（筛出 2 行却写着 "Suspects 4 / Healthy 12"）。成因是 Raycast 的 `List` 内建过滤
只隐藏 item，而分区标题的 `subtitle` 由扩展按全量 `entries` 算。

修法：`filtering={false}` + `onSearchTextChange`，过滤前移到分桶之前
（`matchesQuery`），计数取自各桶长度，与渲染行数恒等一致。
顺带把 Raycast 的模糊匹配换成了子串匹配 —— 本命令的搜索对象是端口号与 PID，
模糊匹配会让 `517` 命中 pid 5170321 之类的无关行，对数字场景是负收益。
`matchesQuery` 的语义（含 `:5173` 这种可复制粘贴的端口写法）与桌面版
`src/App.tsx` 的过滤对齐，两处都是展示层、不涉及判定，改一边时请顺手看另一边。

接管过滤后必须自带「无结果」空状态：内建过滤会渲染 Raycast 自己的 No Results 页，
`filtering={false}` 之后那页不再出现，缺了就是一片空白（已补 `List.EmptyView`）。

**首次运行的下载链路已端到端验证**（2026-08）：把查找阶梯上的所有副本移开
（扩展支持目录，以及偏好里若填过路径则一并清掉）制造「全新用户」环境后重开命令，
扩展自动从 GitHub Release 取回 `portreaper-cli-macos-arm64` 并落盘，
其 SHA-256 与发布的 `portreaper-cli-SHA256SUMS` **逐字节一致**，随后正常渲染列表。

**星标与桌面版的双向同步已验证**（2026-08）：拿扩展**实际使用的那个副本**
（`~/Library/Application Support/com.raycast-x.macos/extensions/portreaper/bin/portreaper-cli`，
Raycast Beta 的支持目录）对一个人造孤儿 dev server 加星，另一个 release 二进制
（`target/release`）立刻在 `whitelist list` 里读到，重新 `scan` 那一行
`is_whitelisted=true` / `is_zombie_suspect=false` 且 `zombie_reasons` 仍完整列出；
反向从 `target/release` 侧移除，扩展侧随即读到空表。两者写的是同一个
`~/Library/Application Support/com.fhf.portreaper/whitelist.json`。
桌面版 GUI 启动时的 `assert_matches_tauri` 未报错，且 v0.7.1 与 v0.8.1 的
`whitelist_key` 推导逐字节相同 —— 装着旧版桌面版也不影响互认。
（纯视觉那一跳 ★ 仍建议上架前扫一眼，但数据链路已闭合。）

**断网时的引导页已修复并验证**（2026-08）：此前 `fetch` 的失败原样冒泡，用户看到的
是一句 `fetch failed`（超时则是 `The operation was aborted due to timeout`），既非引导页
也无从下手 —— 与本文档上面写的「下载失败落到引导页」不符。现由 `DownloadFailedError`
归一（断网/DNS/超时/代理 4xx/资产 404 全部命中，逐项实测），UI 落到引导页并显示
`getaddrinfo ENOTFOUND github.com` 这类有指向性的原因。同时确认安全边界未被吞掉：
校验和不匹配仍抛 `ChecksumMismatchError`、走专属错误页、不留残留文件。

> 造孤儿进程的办法（复现用）：`cd /tmp/demo-app && nohup node dev-server.js &` ——
> 启动它的 shell 一退出，node 即被 launchd 收养成 ppid==1 的孤儿，正是引擎要抓的形态。

## 「终止了没反应」的根因与这一轮 UI 改动（2026-08-10）

用户报告：**从 Terminal 里启动的进程，在扩展里点终止没有任何反应**。排查结论如下，
按解释力排序 —— 前两条是真因，后面几条是把症状放大的观感问题。

1. **根因：进程处于 stopped（`ps state` 含 `T`）态。** 被 Ctrl-Z 挂起、或后台作业
   读终端（SIGTTIN）/ `stty tostop` 下写终端（SIGTTOU）的进程，**收不到已被它
   捕获的 SIGTERM** —— 信号一直挂在 pending 集里。`kill(2)` 照样返回 0，于是
   CLI 退出码 0、扩展弹绿色 "Terminated"，而进程纹丝不动、端口也不释放。
   node / vite / next 全都注册 SIGTERM handler，正好命中这个形态；而终端一旦
   关掉，就再没有人给它 SIGCONT 了 —— 这恰恰是本产品最想抓的那类残留。

   修在引擎（`crates/portreaper-core/src/platform.rs`）：身份探针改成一次
   `ps -o etime=,state=`（不多起进程），温和终止后若目标是 `T` 态就补一发
   SIGCONT。顺序、条件、返回值处理三条约束写在 CLAUDE.md 的 Kill path 一节。

   端到端复现（可原样重跑）：起一个绑 :8799 且 `$SIG{TERM}` 捕获信号的 perl
   监听者 → `kill -STOP` → `portreaper-cli scan --json` 取 `start_unix` →
   `portreaper-cli kill <pid> --start-unix <n>`。修复前进程 state 一直是 `TN`、
   端口不释放；修复后进程终止、端口释放。

2. **「成功」说的是信号已投递，不是进程已死。** 扩展此前在 `kill()` 返回后立刻
   报成功且**永不纠正**。现在改为送出信号后短时轮询确认（`confirmTermination`，
   `--cpu=skip` 探测，2.5s 上限），仍在则如实报 "Still running" 并在 toast 上挂
   Force Kill —— 且 `process_gone` / `pid_reused` 不挂（对已消失或已被复用的 PID
   劝人再用力杀是错误引导）。

3. `killErrorMessage` 缺 `case "os"`，default 还会把 CLI 的两行 stderr（第二行
   是中文）整段甩进 toast。改为按 `code` 分派、原文只进 `console.error`，并新增
   `KillFailedError` 让 UI 能按语义码分叉而不碰文案。`whitelist()` 同样兜了一层
   （此前会泄露完整安装路径 + 中文）。

4. 刷新期 `isLoading` 恒 false，「刷过了但那行还在」与「压根没刷」在屏幕上无法
   区分；`load()` 还可重入，先发后返会覆盖新结果。现在有 `busy` 与单调 `reqId`。

同一轮按 Raycast 官方规范做的 UI 调整（评审依据见各处代码注释）：

- 安装进度页从 `List.EmptyView` 改成 `List.Item`。官方原文：`isLoading` 为真且
  搜索框为空时 EmptyView **永远不显示** —— 而首次使用恰好正是这两个条件同时
  成立的时刻，那段「正在下载并校验引擎」的解释一个字都没渲染出来过。
- 行图标改为「形状 = 残留种类、tintColor = 置信度」；疑似按 Confirmed / Likely /
  Possible 拆三段；搜索栏右侧加判定维 `List.Dropdown`（默认 All —— 默认只看疑似
  会让干净机器打开就是空列表，第一印象像扫描失败）。
- 详情打开时只留一个置信度徽标（官方建议：显示 detail 时不要再挂 accessory）；
  收起时才给 stopped / no port / dup of N / CPU / PID。
- 端口统一 `:5173` 展示（与桌面版和搜索提示对齐，此前注释写着这样、代码不是）；
  判定理由改 `Metadata.TagList` 渲染，**文字仍是引擎原始码，只着色**。
- `List.Item` 补 `id`：不给的话高亮按**位置**记忆，刷新后同一个 Enter 面对的可能
  已是另一个进程 —— 而下一个动作是破坏性的。
- Enter（首个 action）按行分叉：疑似行仍是 Terminate（那是本命令存在的理由），
  Healthy / Starred / 无身份令牌的行落在无害动作上。
- ⌘D 换成 ⌘⇧D（⌘D 是 `Common.Duplicate`，Store 的自动检查会建议改成语义完全
  不对的 "Duplicate"）；Star 用 `Common.Pin`；复制类动作补 `Common.Copy` /
  `CopyName` / `CopyPath`。
- 新增 Open localhost / Show in Finder（仅当 `exe_path` 真的是路径）/ Copy as JSON。
- Markdown 里的命令行与启动链改围栏代码块（外部文本含反引号 / `[]()` 会破坏整段
  渲染甚至画出可点链接）；错误页按处境分四个标题（校验和失败是**安全事件**，
  不能和一次普通扫描故障共用一句 "Scan failed"）。
- 删掉 `commands[0].subtitle`（单命令扩展不该用 subtitle 复述扩展名）。

**行的外观只能由结构化字段驱动**（`confidence` / `is_zombie_suspect` / `ports` /
`duplicate_of` / `state`），**不得**按 `zombie_reasons` 里的具体码名分叉 ——
`scripts/check-reason-parity.mjs` 不覆盖本目录，在这里手抄码名就是一条无守卫的
漂移路径。同理没有引入任何理由词表：理由继续显示引擎原始码。

> **未做、且刻意不做的两项**（评估结论记在这里，免得反复重开）：
> ① `@raycast/utils` 的 `useCachedPromise`（首屏缓存 + abortable）收益确实最高，
> 但它是一次数据层重写、且要往**正在人工评审中的**提交里加一个依赖 —— 本轮先用
> `reqId` + `busy` 拿掉重入与无反馈这两个实际症状，缓存留到 PR 落地之后再做。
> ② menu-bar 第二命令：与桌面版托盘图标抢同一块地方，且 `interval` 背景刷新会
> 定期 spawn 本项目最贵的调用（一次 scan = lsof + 两次 `ps -A` + `launchctl list`）。
> 廉价替代是 `updateCommandMetadata` 把 subtitle 写成「上次扫描时 N 个疑似」。

**截图已于 2026-08-10 全部重出**（4 张，规格与内容见下节 checklist 的表）。
`npx ray lint` 三项硬指标 ready、`npx ray build` distribution 构建通过。

### 真机跑一遍才发现的两个 UI 问题（2026-08-10）

两条都是**只有把扩展装进 Raycast 才看得见**的，tsc / ray build 一个都拦不住：

1. **分区副标题过长会把分区标题挤没。** 详情面板默认打开时列表列只有约 40% 宽，
   `6 · orphaned, nothing is using them` 折成两行、标题被截成 `Confirm...`。
   全部收短到一两个词（`orphaned` / `likely orphaned` / `weak signal` / `exempt` /
   `a live launcher owns these`）。
2. **详情打开时哪怕只留一个 accessory，行标题也会被截断**（实测截成
   `api-gate...`）。官方原文就是「When shown, it is recommended not to show any
   accessories on the `List.Item`」—— 现在照做：详情态零 accessory，置信度由行
   图标的 tintColor 承载，完整判定在右侧 Metadata 的 Verdict 一行。

顺带在真机上把用户报的那个 bug 端到端验完了：造一个绑 :4321、捕获 SIGTERM 的
dev server → `kill -STOP`（等价于在 Terminal 里按 Ctrl-Z）→ 扩展里对它按
Terminate（**普通终止，不是强杀**）→ 进程真的消失、端口释放、列表从 6 行变 5 行。
验证时把扩展支持目录里的那份 CLI 临时换成本地 `target/release` 的新构建
（**验完已还原**）—— 注意扩展下载的那份当时还是 `0.8.1`：它是按
`releases/latest` 取的，此后再没刷新过，schema 没变所以也没有任何机制提醒它过期。

### 重出截图的操作顺序

`npm run typecheck` → 造演示进程（见下）→ `npm run dev` 载入新构建后
`pkill -f "ray develop"`（否则 watcher 自己会以孤儿身份出现在嫌疑列表里）→
`bash scripts/capture-raycast-metadata.sh <输出路径>` → `npx ray lint` →
`npx ray build` → `npm outdated --prefix integrations/raycast` → `npm run publish`
（会**更新同一个 PR**，不会重开）。

**演示进程怎么造**（决定了截图里能出现哪些徽标）：

```bash
# 脚本本身要捕获 SIGTERM（像 vite/node 那样），否则做不出 stopped 那一档
R=~/pr-demo-screens
for p in web-app api-gateway docs-site e2e-suite storefront; do mkdir -p "$R/$p/src"; done
# …把 dev-server.js 放进每个 src/，内容见本节末
(cd "$R/web-app/src"     && nohup node "$R/web-app/src/dev-server.js" 5173 &)
(cd "$R/api-gateway/src" && nohup node "$R/api-gateway/src/dev-server.js" 3000 &)
(cd "$R/docs-site/src"   && nohup node "$R/docs-site/src/dev-server.js" 4321 &)
(cd "$R/e2e-suite/src"   && nohup node "$R/e2e-suite/src/dev-server.js" --no-listen &)
(cd "$R/storefront/src"  && nohup node "$R/storefront/src/dev-server.js" 5180 &)
(cd "$R/storefront/src"  && nohup node "$R/storefront/src/dev-server.js" 5181 &)
sleep 12                                  # 跨过 10s 宽限期，否则全是 possible
kill -STOP $(pgrep -f "dev-server.js 4321")   # 造 stopped 那一档
```

四个不显然、但缺一不可的点（每个都踩过）：

- **必须用绝对脚本路径启动**。`extract_project_name` 要求路径形如
  `/Users/<user>/…/<项目>/src/…`（认 `src`/`dist`/`node_modules` 这类停用词，
  取它前一段）。用 `cd` 进去再跑 `node dev-server.js`，argv 里是相对路径，
  标签就退化成 `dev-server.js · node`，项目名整个消失。
- **每个进程从自己的项目目录启动**。全从同一个 cwd 启动的话，引擎按
  「同 cwd + 同脚本身份」把六行**互相**判成重复实例，画面上一片 `dup of`。
- **放在 `/tmp` 不行**：`extract_project_name` 只认 `/Users/` 下的路径。
- 脚本名 `dev-server.js` 同时充当截图里的搜索过滤词 —— 它出现在 `app_label` 与
  `full_command` 里，一个词就把开发机上的真实进程全挡在画面外（务必先确认它
  在你机器上零命中：`portreaper-cli scan --json | grep -c dev-server`）。

```js
// dev-server.js
const port = process.argv[2];
process.on("SIGTERM", () => process.exit(0));   // 捕获信号，才做得出 stopped 那一档
if (port && port !== "--no-listen") {
  require("node:net").createServer(() => {}).listen(Number(port), "127.0.0.1");
}
setInterval(() => {}, 1 << 30);
```

用完清干净：`pkill -CONT -f dev-server.js; pkill -f dev-server.js; rm -rf ~/pr-demo-screens`。

**deeplink 打开命令时有个会误杀进程的坑**：
`open "raycast://extensions/fhf1121/portreaper/search-ports"` 会先弹一次
「Request to run」确认框（因为是从 Raycast 外部触发的）。用 Return 去确认它，
按键会在确认框已消失的那一帧**穿透到列表**，而列表首项的主动作是 **Terminate**
—— 实测就这么弹出了一个终止确认框。改成点按钮：
`osascript -e 'tell application "System Events" to click at {x, y}'`，
选「Always Run Command」，之后就再不弹了。

**`Likely` / `Possible` 两档在这批截图里没有出现**，是刻意的：造它们需要
「孤儿但无 dev 证据」或「仅会话信号」这类很难稳定复现的形态，而 `duplicate`
信号只会把**非嫌疑**行提升到 Possible（`postprocess.rs:178`），本身已是
Confirmed 的孤儿不会被降档。判定分层由分区标题与 Dropdown 呈现，不靠凑样本。

## Store 提交 checklist

已就位（可复核）：

- [x] `license: MIT`、npm + `package-lock.json`、`platforms: ["macOS"]`、`keywords`
- [x] 图标 512×512 PNG，自带深色圆角底板 ⇒ 明暗主题观感一致，无需 `@dark` 变体
- [x] `assets/` 无未使用文件（Store 会查）
- [x] `CHANGELOG.md` 用 `## [Initial Version] - {PR_MERGE_DATE}` 占位符（merge 时自动替换）
- [x] `README.md` 为英文用户向（Store 展示的就是它；维护者内容在本文件）
- [x] 破坏性操作走 `confirmAlert` + `Alert.ActionStyle.Destructive` + `Action.Style.Destructive`
- [x] `@raycast/api` 为当前最新（1.104.x）；`ray lint` / `ray build` 均通过
- [x] 无 analytics、无 Keychain 访问、UI 全英文
- [x] 二进制走「可信源下载 + SHA-256 校验 + 失败即删 + UI 明示」——
      官方政策点名允许的形态（先例：glean-search #28995、lumen #28909、speedtest）
- [x] **`author` 字段** = Raycast 账号 handle `fhf1121`（不是 GitHub 用户名）——
      2026-08 已核对确认。

- [x] **真机 QA** —— 载入、扫描分组、置信度、详情面板、动作面板、终止确认、搜索过滤、
      无端口孤儿、首次下载 + SHA-256 校验已在 Raycast Beta 实测；星标双向同步与断网
      引导页于 2026-08 补验（断网那条当时**没通过**，改掉后才过 —— 见上节）。
      逐项结果见《真机验证状态》。

待人工完成：

- [x] **上架前扫一眼 ★ 的那一跳** —— 2026-08-08 实测，**没通过，是真 bug，已修**。

      当时的判断是「数据链路已实测闭合，剩下纯粹是前端渲染那一层，风险很低」。
      这个判断错了：链路闭合的只有**写**方向。桌面版的 `whitelist::get_all()` 返回的
      是进程启动时的内存快照，CLI / Raycast 加的星它**永远看不到** —— 那一行仍标红、
      仍计入托盘、**仍留在一键清扫的目标集里**。用户刚在 Raycast 收藏的进程，会被
      桌面版一键清扫杀掉。README 承诺的「Shared with the desktop app」在读方向是假的。

      复现（就是本条 checklist 的动作）：起一个 ppid=1 的孤儿 dev 监听者 → 托盘
      `27` 变 `28 ⚠` → CLI `whitelist add` → 磁盘与 CLI 侧都已生效，**托盘纹丝不动，
      仍是 `28 ⚠`**，等多久都不变。

      修复：`Whitelist::refresh()`（替换语义，取消星标同样传播）+ `get_all()` 每轮调它。
      两条测试钉住：引擎语义一条、「桌面侧到底有没有调」一条。

      **修复后复验（同一台机器，v0.9.0）**：桌面版在自身零操作的前提下，把另一个
      前端进程写入的星标吃了进去 —— 托盘从 `30 ⚠` 变成 `30`（⚠ 是 `suspect_count > 0`
      的布尔量，故须把当时全部 suspect 都加星才看得出跨越，事后已全部撤回、白名单
      恢复为空）。

      > 复验时踩到的坑，值得记下来：**别拿 release 版 CLI 去验 `pnpm tauri dev`**。
      > 按 `paths.rs` 的分环境隔离，release CLI 写 prod 目录、debug 构建写 `dev/`，
      > 两边根本不是同一个 whitelist.json —— 星标「没生效」会看起来像 bug 复发。
      > 要么用 `cargo run -p portreaper-cli`，要么拿装好的 `.app` 配 release CLI。

      **教训记在这里**：「风险很低、不必亲眼看」正是这条 checklist 存在的理由。
      写方向有测试、读方向没有，而两边共用「共享状态」这一个说法，就没人再去分开验。
- [x] **提交前跑一遍** `npm run lint` + `npm run build` + `npm outdated --prefix integrations/raycast`
      —— 2026-08-08 全过：`ray lint` 三项 ready（ESLint/Prettier 未装是预期的，
      见《为什么不装 ESLint / Prettier》）、`ray build` 成功、`tsc --noEmit` 干净。
      `npm outdated` 报的 `@types/node` 26.1.2 → 26.2.0 已升，现已无过期项。
      `npm audit` 剩两条 low，**有意不修**，理由见《依赖升级》一节。

      真机那类事**不是 tsc / ray build 能替你验的** —— 它们只保证代码能编译、清单合规，
      保证不了「点下去真的有反应」。Store 提交 checklist 明确要求实测 distribution build。
- [x] **`metadata/` 截图** —— 4 张，2000×1250 sRGB PNG，浅色主题，`ray lint` 的
      "validate extension metadata" 已通过：

      | 文件 | 内容（2026-08-10 重出） |
      |---|---|
      | `portreaper-1.png` | 满宽列表（⌘⇧D 收起详情）：五种徽标同框 —— `no port` / `stopped` / `dup of <pid>` / `confirmed` / `pid` |
      | `portreaper-2.png` | 列表 + 详情面板，选中的是**被挂起**那行：命令、stopped 的完整说明、Verdict、判定理由 TagList（原始码）、`State: TN · stopped` |
      | `portreaper-3.png` | ⌘K 动作面板：Danger Zone（Terminate ↵ / Force Kill ⇧⌘⌫，红色破坏性样式）+ Inspect（Toggle Details / Open localhost:4321） |
      | `portreaper-4.png` | 终止确认弹窗：`PID 76738 · port :4321 · It is suspended; terminating resumes it so it can shut down.` |

      **画面里只出现临时造的 demo 进程**（`web-app` / `api-gateway` / `docs-site` /
      `e2e-suite` / `storefront` ×2，造法见上一节）—— 搜索框预置 `dev-server`
      过滤，把开发机上的真实应用列表（含可识别身份的企业软件）全部挡在外面。
      重截时务必保持这一点：端口/进程类工具的截图天然会暴露「这台机器装了什么」。
      顺带注意：deeplink 的「Request to run」确认框背后就是 Raycast 的应用列表，
      别在它还在的时候截图。

      生成用 **`scripts/capture-raycast-metadata.sh`**（维护者工具，在主仓库
      `scripts/` 下，不随扩展提交）。三个会静默毁掉成果的坑已固化进脚本，
      不必再靠记忆规避：只截窗口矩形而非全屏、取面积最大的窗口（⌘K 动作面板与
      输入法候选条都是独立窗口）、圆角遮罩剔除窗口外像素。

      脚本管不到、调用方要自己注意的第四个坑：`ray develop` 的 watcher 自己
      就是个无端口孤儿，会以 `raycast · node` 出现在嫌疑列表里。扩展一旦载入
      Raycast，watcher 就可以停掉、命令仍可用 —— 截图前先 `pkill -f "ray develop"`。

  > README 里引用的图片放**顶层 `media/`**，不能混进 `metadata/` 或 `assets/`。
  > 目前 README 未引用任何图片，故无需建 `media/`。

- [x] **提交**：`npm run publish`（自动 fork raycast/extensions 并开 PR）。
      PR 描述里主动交代二进制来源（本项目自己的 GitHub Releases、构建流水线公开可溯源）、
      SHA-256 校验、校验失败即删、UI 明示，并引用上述先例 —— 这是审核最关注的一点，
      主动说明比等着被问效率高。

      **当前状态：raycast/extensions#30075，OPEN，等待人工评审**
      （2026-08-08 提交；raycastbot 提示初审最长 15 个工作日）。

      机器人评审（greptile）提了三条，均成立、均已修并推到同一个 PR，
      见 commit `Update portreaper extension`：schema 不兼容的托管副本换不掉、
      无身份令牌的行仍摆着终止入口、偏好标题不合 title case 约定。

      两条**再提交时的操作要点**：

      - `ray publish` 要求工作区**干净**，否则直接报错退出（不是警告）。
      - 修完再跑一次 `npm run publish` 会**更新同一个 PR**（提示语是
        "Your submission has been updated"），不会开出重复 PR ——
        它靠本地 tag `__raycast_latest_publish_ext/portreaper__` 记状态。
        注意那个 tag 是本地工具产物：`git push --tags` 会把它推上公开仓库，
        污染 tag 列表（本次已误推并删除；发版请用 `git push origin vX.Y.Z`
        而不是 `--tags`）。

## 人工评审前的自动查重（2026-08-14）

Raycast 团队成员在 PR 上贴了一条带 `<!-- store-duplicate-check -->` 标记的机器初筛：
Store 已有 **Port Manager**（`ports`，1.3k 安装）与本扩展相似度 **0.56**。

**这不是驳回。** 原文写明 *"Overlap is not a blocker on its own, but the README
should make the difference clear"* —— 是合并前的**必办项**，PR 全程保持 OPEN。
别把它读成拒绝，更别因此去改产品定位。

应对（2026-08-16，已推同一个 PR 并回评论）：README 加 `How this differs from a
port viewer` 一节。写法是**先承认对方再划边界** —— 硬碰硬宣称「我更好」反而坐实重复。

**该节 2026-08-17 重写过一次，两处教训都值得留着。**

其一，初版把两者放在**同一个问题的两端**：开头「只想放掉 3000 端口的话，普通端口
查看器就是更合适的工具」，结尾「只想要端口列表的话，这是个很重的办法」。礼让是对的，
但一头一尾各让一次，中间夹五条功能对比，读下来像「我承认重复，但我还多带了这些
功能」—— 那正是查重评论想问的问题，等于没答。

重写后改为**两个不同的问题**：端口查看器答「3000 端口上是什么」（你已经知道有东西
挡路，只需定位它）；本扩展答「这台机器上还有什么没人认领」（你在**发现**残留，而不是
定位一个已知端口）。这个框架下，「列出不占端口的孤儿」从一条附加功能变成必然推论 ——
在「放掉这个端口」的问题里它按定义就不该存在，在「我留下了什么」的问题里它是核心。
礼让压缩成第一段末尾半句，结尾不再自贬。四条 bullet 全部从定位差异推出，不罗列功能；
终止安全性那条删掉 —— 下一节 `Terminating is safe by construction` 已完整讲过。

其二，**链接曾经是 404**：初版写 `raycast.com/diegoleteliers10/ports`（GitHub 用户名），
而正确的是 `raycast.com/dleteliers_/ports`（`author` 字段）。这个坑本文件下一条就
写着，README 里却照样踩了 —— 而它偏偏出现在回应「说清差异」这条必办项的那一节里，
评审员点进去就是 404，比不放链接更糟。**改完务必 curl 一遍该节的每个链接。**

三条经手才知道的事实：

- **Store 链接用 `package.json` 的 `author` 字段，不是 GitHub 用户名。**
  Port Manager 的页面是 `raycast.com/dleteliers_/ports`；查重评论里署的
  `@diegoleteliers10` 是 GitHub 账号，拿它拼 URL 是 404。
- **README 里刻意不写平台差异。** 事实是对方声明 `"platforms": ["Windows"]`
  （四个源文件里 `darwin` / `lsof` / `macOS` 零命中，它 README 那句 "Full support
  for both Windows and macOS" 与代码不符），与本扩展的 `["macOS"]` 在 Store 里
  根本不碰面。但对方哪天补上 macOS，那句话就成了过时的错误陈述 —— 而 README 是
  长期文档。平台不重叠是**一次性事实**，写进 PR 评论给人工评审员看即可。
- 评论里同时点明「这不是接受本扩展的理由，只是界定重叠范围」—— 拿平台当挡箭牌
  会显得在回避产品重复这个真问题。

### 读完对方源码后确认的可借鉴点

省得以后再读一遍（`extensions/ports`，696 行）。

- **最值得补的是 `mode: "no-view"` + `arguments` 的第二命令。** 它的
  `kill port 3000` ↵ 一步到位，本扩展目前最短路径是六步（开命令 → 等扫描 →
  搜 `:3000` → ↵ → 确认 → ↵）。它的护栏是硬编码系统端口黑名单（端口号 ≠ 进程身份，
  判据本身是错的），而本项目有真判据：那个端口上的进程是不是 suspect。设计应为
  suspect 直接终止 + 走完整复扫确认 + `showHUD`，healthy / starred 则拒绝静默终止、
  引导回列表。**但要等 PR 合并之后再做** —— 往人工评审中的提交里加命令会让评审重排队。
- README 顶部内嵌截图（本扩展的卖点是视觉性的）。注意引用的图必须放顶层 `media/`，
  见上文。
- 首屏速度：它用两层 TTL 缓存兜「打开那一瞬屏幕上有没有东西」。本扩展冷启动是一段
  纯 loading，官方答案是 `@raycast/utils` 的 `useCachedPromise`（已记在上面的
  「未做、且刻意不做的两项」里，合并后可升级为待办）。

**明确不学的**：靠 `error.message.includes(...)` 分派错误（v0.9.0 刚把这套删干净）；
`taskkill` 返回 0 就报「已终止」而从不复核；乐观更新后立刻全量重扫导致列表闪两次；
温和失败自动升级强杀（本项目的产品决定是让用户显式选）；30s 自动刷新（Raycast 命令
停留时间就几秒，而本项目一次 scan 是最贵的调用）。
