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

**为什么不交给 Dependabot**：给本目录配 npm 生态试过一次，每月必失败。Dependabot 的
pnpm 探测只看父目录有没有 `pnpm-lock.yaml` + `pnpm-workspace.yaml`，命中就判定
「workspace 子目录」并拒绝更新 —— 与本目录自身是不是规范的 npm 布局无关
（它甚至会拿仓库根的 vite-plus 来 `npm install` 这个目录）。完整错误与结论记在
`.github/dependabot.yml` 的头部注释里，别再加回来。

## portreaper-cli 的查找顺序

`src/cli.ts` 的 `resolveCliPath`，逐个 `existsSync` 探测；**不查 `PATH`** —— 只认下列固定路径：

1. 扩展偏好里的 `portreaper-cli path`（显式指定，优先级最高）
2. 扩展支持目录下自动下载并校验过的副本（首次使用时自动获取）
3. `/Applications/Portreaper.app/Contents/MacOS/portreaper-cli`（打包尚未落地，见下）
4. `<repo>/target/release/portreaper-cli`、`<repo>/target/debug/portreaper-cli`（开发构建）
5. `~/.cargo/bin/portreaper-cli`

全部落空时走 `src/install.ts` 自动下载 + SHA-256 校验（Store 政策明确要求
"Avoid asking users to perform additional downloads"，且下载必须带哈希校验）。
下载失败才渲染引导页，列出找过的位置。

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
（扩展支持目录 + `target/release` + `target/debug`）制造「全新用户」环境后重开命令，
扩展自动从 GitHub Release 取回 `portreaper-cli-macos-arm64` 并落盘，
其 SHA-256 与发布的 `portreaper-cli-SHA256SUMS` **逐字节一致**，随后正常渲染列表。

**仍未验证**（需要人工补跑）：断网时的引导页、星标与桌面版的双向同步。

> 造孤儿进程的办法（复现用）：`cd /tmp/demo-app && nohup node dev-server.js &` ——
> 启动它的 shell 一退出，node 即被 launchd 收养成 ppid==1 的孤儿，正是引擎要抓的形态。

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

待人工完成：

- [ ] **真机 QA 的剩余两项** —— 其余场景（载入、扫描分组、置信度、详情面板、动作面板、
      终止确认、搜索过滤、无端口孤儿、首次下载 + SHA-256 校验）已在 Raycast Beta 实测通过，
      逐项结果见上节《真机验证状态》。`npm run dev` 后补跑这两条：

      | 场景 | 看什么 |
      |---|---|
      | 星标双向同步 | 在 Raycast 加星 → 桌面版 2 秒内是否出现 ★（反向再试一次）。README 的「Shared with the desktop app」按这个时延写死了承诺，验不过就得回去改措辞 |
      | 断网 | 断网后以「全新用户」状态（挪走查找阶梯上的所有 CLI 副本）首次运行，是否渲染引导页而不是白屏或崩溃 |

      这两条**都不是 tsc / ray build 能替你验的** —— 它们只保证代码能编译、清单合规，
      保证不了「点下去真的有反应」。Store 提交 checklist 明确要求实测 distribution build。
- [x] **`metadata/` 截图** —— 4 张，2000×1250 sRGB PNG，浅色主题，`ray lint` 的
      "validate extension metadata" 已通过：

      | 文件 | 内容 |
      |---|---|
      | `portreaper-1.png` | 满宽列表：Healthy / Suspects 分区，置信度徽标完整 |
      | `portreaper-2.png` | 列表 + 详情面板：判定理由、PID、类别、端口、运行时长 |
      | `portreaper-3.png` | ⌘K 动作面板：Terminate / Force Kill 红色破坏性样式 |
      | `portreaper-4.png` | 终止确认弹窗：进程名 + PID + 端口 + Cancel/Terminate |

      **画面里只出现临时造的 demo 进程**（`web-app` / `api-gateway` / `docs-site`
      三个孤儿 node server）与一个通用的 `node` 行 —— 搜索框预置 `node` 过滤，
      把开发机上的真实应用列表（含可识别身份的企业软件）全部挡在外面。
      重截时务必保持这一点：端口/进程类工具的截图天然会暴露「这台机器装了什么」。

      生成用 **`scripts/capture-raycast-metadata.sh`**（维护者工具，在主仓库
      `scripts/` 下，不随扩展提交）。三个会静默毁掉成果的坑已固化进脚本，
      不必再靠记忆规避：只截窗口矩形而非全屏、取面积最大的窗口（⌘K 动作面板与
      输入法候选条都是独立窗口）、圆角遮罩剔除窗口外像素。

      脚本管不到、调用方要自己注意的第四个坑：`ray develop` 的 watcher 自己
      就是个无端口孤儿，会以 `raycast · node` 出现在嫌疑列表里。扩展一旦载入
      Raycast，watcher 就可以停掉、命令仍可用 —— 截图前先 `pkill -f "ray develop"`。

  > README 里引用的图片放**顶层 `media/`**，不能混进 `metadata/` 或 `assets/`。
  > 目前 README 未引用任何图片，故无需建 `media/`。

- [ ] **提交**：`npm run publish`（自动 fork raycast/extensions 并开 PR）。
      PR 描述里主动交代二进制来源（本项目自己的 GitHub Releases、构建流水线公开可溯源）、
      SHA-256 校验、校验失败即删、UI 明示，并引用上述先例 —— 这是审核最关注的一点，
      主动说明比等着被问效率高。
