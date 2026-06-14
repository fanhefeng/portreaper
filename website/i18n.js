/* Portreaper site — i18n dictionary. zh default, en toggle. */
window.I18N = {
  zh: {
    "meta.title": "Portreaper · 收割端口上的僵尸进程",
    "meta.description":
      "Portreaper —— 一个 macOS / Windows 桌面托盘工具，找出占用 TCP 端口的进程，精准识别并清扫孤儿 dev-server 僵尸进程。",
    "a11y.skip": "跳到主要内容",

    "nav.features": "功能",
    "nav.install": "安装",
    "nav.github": "GitHub",

    "hero.langBadge": "中 / EN",
    "hero.tagline": "收割端口上的僵尸进程。",
    "hero.sub":
      "找出谁在占用端口，看清是谁拉起的，然后一键终结那些父进程已死、却还赖在端口上的孤儿 dev-server。",
    "hero.allReleases": "查看全部版本 →",

    "dl.macArm": "Apple Silicon · .dmg",
    "dl.macIntel": "Intel · .dmg",
    "dl.experimental": "实验性",
    "dl.winDetail": "安装包 · .exe",

    "showcase.title": "应用界面",
    "mock.proc": "进程",
    "mock.ports": "端口",
    "mock.suspect": "疑似僵尸",
    "mock.fav": "已收藏",
    "mock.col.port": "端口",
    "mock.col.type": "类型",
    "mock.col.app": "App",
    "mock.col.launcher": "启动者",
    "mock.col.pid": "PID",
    "mock.col.action": "操作",
    "mock.cat.script": "脚本",
    "mock.reason.orphan": "孤儿 · PPID=1",
    "mock.kill": "Kill",

    "features.title": "不是又一个端口查看器",
    "features.lead":
      "核心价值是僵尸识别和启动链 —— 帮你分清「正在干活的进程」和「该被收割的孤魂」。",
    "feat.detect.title": "僵尸识别",
    "feat.detect.body":
      "置信度分层（confirmed / likely / possible）综合孤儿、孤儿链、会话失效等信号判定。launchd / Homebrew 后台服务自动豁免 —— 不误报正在服务的进程。",
    "feat.chain.title": "启动链",
    "feat.chain.body":
      "沿父进程链一路上溯，看清这个进程到底是谁拉起的：Terminal → zsh → npm → vite。一眼分辨「我自己开的」还是「不知哪来的孤儿」。",
    "feat.sweep.title": "一键清扫",
    "feat.sweep.body":
      "批量终止所有 Confirmed + Likely 的僵尸进程，一次释放占用的端口。白名单收藏的进程永远豁免，绝不被误杀。",
    "feat.cross.title": "跨平台",
    "feat.cross.body":
      "常驻 macOS 菜单栏与 Windows 系统托盘，关窗即隐藏、不打断工作流。界面中英双语，可疑进程数实时显示在托盘（macOS 菜单栏标题 / Windows 悬停提示）。",

    "install.title": "安装与首次启动",
    "install.lead": "应用未经签名公证。下面是绕过系统拦截的标准步骤。",
    "install.tab.mac": "macOS",
    "install.tab.win": "Windows",
    "install.mac.s1":
      "打开下载的 <code>.dmg</code>，把 Portreaper 拖入「应用程序」后再打开 —— 不要直接在 dmg 里双击运行（会触发 App Translocation，下面的步骤会失效）。",
    "install.mac.s2":
      "首次打开若提示「无法验证开发者」：前往「系统设置 → 隐私与安全性」，点最下方的「仍要打开」。",
    "install.mac.s3":
      "或者在「应用程序」里 <strong>右键 → 打开</strong>，在弹窗里再次确认「打开」。",
    "install.mac.s4":
      "若提示的是「<strong>已损坏，无法打开</strong>」（未签名 App 的常见报错，此时「仍要打开」常常不出现、右键打开也无效），在终端执行下面这行移除隔离属性最可靠：",
    "install.copy": "复制",
    "install.win.s1":
      "运行 <code>.exe</code> 安装包。若出现 SmartScreen 蓝屏，点击「更多信息 → 仍要运行」。",
    "install.win.s2":
      "杀毒软件可能误报 —— 本应用需要枚举并终止进程，这类行为容易被启发式引擎标记。可将其加入信任列表。",
    "install.win.expTitle": "实验性",
    "install.win.expBody":
      "Windows 版本目前仅经过 CI 编译与单元测试，尚未在多机型上充分验证。欢迎在 issue 区反馈问题。",

    "footer.repo": "GitHub 仓库",
    "footer.license": "MIT License © 2026 fhf",
    "footer.built": "Built with Tauri",

    "version.label": "最新版本",
    "copy.done": "已复制",
  },

  en: {
    "meta.title": "Portreaper · Reap zombie processes off your ports",
    "meta.description":
      "Portreaper — a macOS / Windows desktop tray app that finds processes holding TCP ports and precisely identifies and reaps orphaned dev-server zombies.",
    "a11y.skip": "Skip to main content",

    "nav.features": "Features",
    "nav.install": "Install",
    "nav.github": "GitHub",

    "hero.langBadge": "中 / EN",
    "hero.tagline": "Reap the zombies squatting on your ports.",
    "hero.sub":
      "See who's holding a port, trace who launched it, then terminate the orphaned dev-servers whose parent shell died but are still squatting on the port.",
    "hero.allReleases": "All releases →",

    "dl.macArm": "Apple Silicon · .dmg",
    "dl.macIntel": "Intel · .dmg",
    "dl.experimental": "Experimental",
    "dl.winDetail": "Installer · .exe",

    "showcase.title": "App interface",
    "mock.proc": "procs",
    "mock.ports": "ports",
    "mock.suspect": "suspects",
    "mock.fav": "starred",
    "mock.col.port": "Port",
    "mock.col.type": "Type",
    "mock.col.app": "App",
    "mock.col.launcher": "Launched by",
    "mock.col.pid": "PID",
    "mock.col.action": "Action",
    "mock.cat.script": "script",
    "mock.reason.orphan": "orphan · PPID=1",
    "mock.kill": "Kill",

    "features.title": "Not just another port viewer",
    "features.lead":
      "The point is zombie detection and the launcher chain — telling the processes doing real work apart from the orphans worth reaping.",
    "feat.detect.title": "Zombie detection",
    "feat.detect.body":
      "Tiered confidence (confirmed / likely / possible) combining orphan, orphan-chain and dead-session signals. launchd / Homebrew services are auto-exempted — no false alarms on processes that are actually serving.",
    "feat.chain.title": "Launcher chain",
    "feat.chain.body":
      "Walks the parent chain to show exactly who spawned a process: Terminal → zsh → npm → vite. Tell at a glance whether you started it or it's an orphan from nowhere.",
    "feat.sweep.title": "One-tap sweep",
    "feat.sweep.body":
      "Batch-terminate every Confirmed + Likely zombie and free their ports in one go. Starred whitelist entries are always exempt and never killed by mistake.",
    "feat.cross.title": "Cross-platform",
    "feat.cross.body":
      "Lives in the macOS menu bar and the Windows system tray; closing the window just hides it, never interrupting your flow. Bilingual UI, with the suspect count live in the tray (menu-bar title on macOS, tooltip on Windows).",

    "install.title": "Install & first launch",
    "install.lead": "The app is unsigned. Here are the standard steps to get past the OS gatekeeper.",
    "install.tab.mac": "macOS",
    "install.tab.win": "Windows",
    "install.mac.s1":
      "Open the downloaded <code>.dmg</code>, drag Portreaper into Applications, and launch it from there — don't run it straight from the dmg (that triggers App Translocation and the steps below won't stick).",
    "install.mac.s2":
      "If the first launch says “cannot verify the developer”: go to System Settings → Privacy & Security and click “Open Anyway” at the bottom.",
    "install.mac.s3":
      "Or in Applications, <strong>right-click → Open</strong> and confirm “Open” in the dialog.",
    "install.mac.s4":
      "If it instead says the app is <strong>“damaged and can't be opened”</strong> (a common error for unsigned apps — “Open Anyway” and right-click → Open usually won't help here), running this in Terminal to strip the quarantine attribute is the reliable fix:",
    "install.copy": "Copy",
    "install.win.s1":
      "Run the <code>.exe</code> installer. If SmartScreen appears, click “More info → Run anyway”.",
    "install.win.s2":
      "Antivirus may flag it — the app needs to enumerate and terminate processes, behavior heuristic engines often mark. Add it to your trust list if needed.",
    "install.win.expTitle": "Experimental",
    "install.win.expBody":
      "The Windows build is so far only CI-compiled and unit-tested, not yet broadly validated across machines. Feedback in the issue tracker is welcome.",

    "footer.repo": "GitHub repo",
    "footer.license": "MIT License © 2026 fhf",
    "footer.built": "Built with Tauri",

    "version.label": "Latest",
    "copy.done": "Copied",
  },
};
