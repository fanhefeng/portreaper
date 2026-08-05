# Portreaper for Raycast

在 Raycast 里列出监听端口的进程与**不占端口的孤儿 dev 进程**，一键终止无人认领的那些。

判定逻辑与桌面版**完全相同** —— 都是 `portreaper-core` 这一个引擎。本扩展不做任何
判定，只负责调用与展示。

## 前置：portreaper-cli

扩展通过 `portreaper-cli` 与引擎通信，按以下顺序寻找它：

1. 扩展偏好里的 `portreaper-cli path`（显式指定，优先级最高）
2. `/Applications/Portreaper.app/Contents/MacOS/portreaper-cli`（打包尚未落地，见下）
3. `~/.cargo/bin/portreaper-cli`
4. `PATH`

都找不到时会渲染一个引导页，列出找过的位置。

目前请从源码安装：

```bash
# 方式一：装到 PATH
cargo install --path crates/portreaper-cli

# 方式二：本地构建后在扩展偏好里指向它
cargo build --release -p portreaper-cli
# → <repo>/target/release/portreaper-cli
```

> **随 `.app` 分发尚未实现。** Tauri 的 `externalBin` / `bundle.resources` 都要求文件在
> dev 时也存在，会给日常 `pnpm tauri dev` 加一道脆弱前置；且完整 release 流程无法本地
> 彩排。详见 `docs/ARCHITECTURE-CORE-SPLIT.md` 步骤 4。

## 开发

```bash
npm install            # 本目录独立于主仓库的 pnpm workspace
npm run typecheck      # tsc --noEmit
npm run dev            # ray develop（需要本机装有 Raycast）
npm run build          # ray build —— 提交 Store 前必须通过
```

**为什么这里用 npm 而主仓库用 pnpm**：Raycast Store 要求扩展提交 `package-lock.json`
（官方 CI 用 npm 构建）。这是唯一的例外，不影响仓库其余部分。

**为什么不装 ESLint / Prettier**：本仓库的格式与 lint 统一由 Vite+ 工具链
（`vp check` = oxfmt + oxlint）负责，扩展代码也在其覆盖范围内。Raycast 官方模板
默认带 ESLint + Prettier，但两者都**不是** Store 的硬性要求 —— 未安装时
`ray lint` 只会 warn 一句并跳过格式检查，真正的硬指标（`package.json` 字段、
图标规格）照常校验，`ray build` 也照常通过。

装上它们的代价是实打实的：Prettier 与 oxfmt 功能重叠、换行风格不同，同管一批文件
会在 `vp check --fix` 和 `ray lint --fix` 之间来回改写，逼得整个目录必须从主仓库的
格式门禁里排除 —— 在一个把「格式门禁统一」当教训写进 CLAUDE.md 的仓库里凿一个飞地，
不划算。（另注：Prettier 是被 `@raycast/eslint-config` 当传递依赖拖进来的，
只卸 prettier 没用，得连 eslint 一起去掉才会真正消失。）

**残余风险**：官方文档提到 lint 检查「之后也会通过 GitHub 自动检查跑一遍」。若
Store 的 CI 用它自带的 Prettier 检查代码风格，PR 可能被标记格式问题。届时的处置是
提交前单跑一次 `ray lint --fix`，而不是把这套工具链常驻进仓库。

## 与桌面版共享状态

星标（白名单）写的是**同一个文件**：

```
~/Library/Application Support/com.fhf.portreaper/whitelist.json
```

在 Raycast 里加的星，桌面版下一轮扫描（2 秒内）就能看到，反之亦然。这由
`portreaper_core::paths` 保证 —— 两边调用的是同一个函数，桌面版启动时还会逐一比对
自解析结果与 Tauri 的解析，不一致就报警。

> debug 构建的 CLI 指向 `.../com.fhf.portreaper/dev/whitelist.json`，与
> `pnpm tauri dev` 配对；release 构建指向正式目录，与安装版配对。这是刻意设计，
> 不是 bug。

## 为什么理由显示成 `ppid1_orphan` 这样的机器码

翻译属于「表达」，是前端的事，而桌面版的双语文案住在 `src/i18n.ts` —— 那个模块在顶层
访问 `localStorage` / `navigator`，Node 环境 import 不进来。与其为 Raycast 复制第二份
文案（第二份真相源 + 第二条漂移路径），不如诚实地显示引擎的原始判定码：本扩展的用户是
开发者，`ppid1_orphan` 比一句含糊的翻译更有信息量。完整解释在桌面版的详情面板。

## 安全性

`kill` 强制携带扫描时捕获的 `start_unix`（进程创建时间）。引擎在终止前重新核对它，
对不上就拒绝 —— 这防的是「scan 与点击之间 PID 被回收、误杀另一个进程」。这条防护
在引擎侧是 fail-closed 的，所有前端自动继承，扩展这边没有也不该有绕过它的开关。

## 未验证的部分

扩展代码通过了 `tsc --noEmit`（`@raycast/api` 类型全部对上），CLI 侧的 scan / kill /
whitelist 三条链路都在本机真机验证过。但**Raycast 内的 UI 交互尚未真机验证** ——
需要在装有 Raycast 的机器上 `pnpm dev` 走一遍。
