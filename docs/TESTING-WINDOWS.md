# Windows 手动验收清单 / Windows Manual Acceptance Checklist

> 状态：Windows 构建目前是**实验性**的 —— 只在 CI 里编译并跑过单元测试，**还没有在真机上做过人工验收**（暂时没有 Windows 机器）。等拿到 Windows 机器后，按本清单逐项验证；**全部通过后** → 移除 `release.yml` 的 `releaseBody` 以及官网上的「实验性」标签。
>
> Status: the Windows build is **experimental** — CI-compiled and unit-tested only, **with no manual QA on real hardware yet** (no Windows machine available). When a Windows machine is available, run this checklist top to bottom. **When everything passes** → remove the experimental label from `release.yml` `releaseBody` and from the website.

平台差异要点 / Platform notes to keep in mind:

- 扫描走 `GetExtendedTcpTable` + `sysinfo`，杀进程用 `TerminateProcess`。
  Scanning uses `GetExtendedTcpTable` + `sysinfo`; killing uses `TerminateProcess`.
- Windows **没有 SIGTERM 的对应物**：只有一个「**Terminate**」按钮（相当于强杀），没有「优雅终止 / 强杀」之分。
  Windows has **no SIGTERM equivalent**: a single **Terminate** button (hard kill), no graceful-vs-force split.
- Windows 托盘**没有标题文字**：计数只能显示在 **tooltip（悬停提示）**里，不是 macOS 那种托盘标题。
  The Windows tray has **no title text**: counts live in the **tooltip**, not a tray title like macOS.
- 孤儿判定信号在 Windows 上是「**父进程已退出 / PID 槽位被复用**」，而非 macOS 的 `PPID=1`。
  The orphan signal on Windows is "**parent exited / PID slot reused**", not macOS `PPID=1`.

---

## 清单 / Checklist

### 1. 安装 / Install

- [ ] 用 NSIS 安装包 `Portreaper-windows-x64-setup.exe` 安装。
      Install via the NSIS installer `Portreaper-windows-x64-setup.exe`.
- [ ] SmartScreen 弹出「Windows 已保护你的电脑」时，点击「**更多信息 → 仍要运行**」可继续。
      When SmartScreen shows "Windows protected your PC", **More info → Run anyway** lets it proceed.
- [ ] 安装完成，开始菜单 / 桌面有快捷方式。
      Install completes; Start-menu / desktop shortcut present.

### 2. 首次启动与托盘 / First launch & tray

- [ ] 启动后出现**托盘图标**。
      A **tray icon** appears on launch.
- [ ] 托盘 **tooltip 显示计数**（端口数 / 疑似僵尸数），**而不是**标题文字。
      The tray **tooltip shows counts** (ports / suspects), **not** title text.
- [ ] **左键单击托盘图标**打开主窗口。
      **Left-clicking the tray icon** opens the main window.
- [ ] 点窗口的**关闭按钮 → 隐藏到托盘**（应用仍在跑），不是退出。
      The window **close button hides to tray** (app keeps running), not quit.
- [ ] 通过托盘菜单的「退出 / Quit」才真正退出进程。
      Only the tray menu **Quit** actually exits the process.

### 3. 扫描与展示 / Scan & display

- [ ] 列表列出所有正在 **LISTEN 的进程**，端口 / PID / 可执行文件路径正确。
      The list shows all **listening processes** with correct ports / PID / exe path.
- [ ] **含中日韩（CJK）字符的用户路径**（如 `C:\Users\张三\...`）显示正常，无乱码。
      **CJK user paths** (e.g. `C:\Users\张三\...`) render correctly, no mojibake.
- [ ] 一个进程监听多个端口时，端口被**合并**到同一行。
      A process listening on multiple ports has its ports **merged** into one row.
- [ ] **CPU / 内存列数值合理**（注意：**首次扫描 CPU 可能显示 0%**，因为采样需要两次间隔才能算出占用，这是正常的）。
      **CPU / memory columns are sane** (note: the **first scan may show 0% CPU** because usage needs two samples — this is expected).

### 4. 孤儿检测 / Orphan detection

- [ ] 在一个 **cmd 窗口**里启动一个监听进程：
      From a **cmd window**, start a listener:
      ```
      node -e "require('http').createServer().listen(34567)"
      ```
- [ ] **直接关掉那个 cmd 窗口**（父进程消失）。
      **Close that cmd window** (the parent process disappears).
- [ ] 刷新后，该 node 进程被**标记为疑似僵尸**，理由是「**父进程已退出**」。
      After a refresh, the node process is **flagged as a suspect** with reason "**parent exited**".

### 5. 终止 / Terminate

- [ ] 点该进程的「**Terminate**」按钮 → 进程被杀（`TerminateProcess`），**对应端口被释放**（端口从列表消失）。
      Clicking the row's **Terminate** button kills it (`TerminateProcess`) and the **port is freed** (disappears from the list).
- [ ] 确认对话框文案符合 Windows 语境（**没有** SIGTERM/SIGKILL 字样）。
      The confirm dialog wording fits Windows (**no** SIGTERM/SIGKILL terminology).
- [ ] **一键清扫**只终止 `confirmed` + `likely` 的进程，可正常工作。
      The **batch sweep** terminates only `confirmed` + `likely` entries and works as expected.

### 6. 收藏（白名单）往返 / Whitelist round-trip

- [ ] 对某个进程**加入收藏（★）**→ 它不再被判为疑似僵尸，从托盘计数和一键清扫中排除，但仍出现在列表里并带 ★ 标记。
      **Star (favorite)** a process → it stops being a suspect, drops from the tray count and the sweep, but stays in the list with a ★ chip.
- [ ] **取消收藏**后恢复原判定。
      **Un-starring** restores the original classification.
- [ ] **重启应用后收藏仍在**（持久化到 JSON 生效）。
      Favorites **persist across a restart** (JSON persistence works).

### 7. 中英切换 / zh↔en toggle

- [ ] 默认**跟随系统语言**。
      Defaults to **following the system language**.
- [ ] 手动切换 **中文 ↔ English**，界面文案全部切换。
      Manual **中文 ↔ English** toggle switches all UI strings.
- [ ] **托盘菜单文案**也随之切换。
      The **tray menu** text switches too.

---

## 已知限制 / Known limitations

- Windows **没有优雅终止**：只有强制 `TerminateProcess`，被杀进程不会收到清理信号。
  **No graceful kill on Windows**: only forced `TerminateProcess`; killed processes get no cleanup signal.
- **提权 / MSIX 打包的进程**可能拿不到完整命令行或可执行路径，标签会**降级**（显示不全或更粗略）。
  **Elevated / MSIX-packaged processes** may not expose a full command line or exe path, so labels are **degraded** (partial or coarse).
- **EDR / 杀软可能拦截 kill**：此时终止失败，界面会**显示返回的错误码**。
  **EDR / AV may block kills**: termination then fails and the UI **shows the returned error code**.

---

**全部通过后 / When all pass** → 移除 `release.yml` `releaseBody` 与官网上的「实验性 / experimental」标签。
Remove the experimental label from `release.yml` `releaseBody` and the website.
