// 「这是什么」知识库（纯逻辑，无 React/Tauri 依赖；从 App.tsx 拆出以便单测）。
import type { I18nKey, Lang } from "./i18n";
import type { ProcessEntry } from "./model";

/**
 * 常见进程知识库：让非技术用户一眼知道「这是什么软件、干什么用的」。
 * 顺序即优先级（具体的在前，node/python 等泛化的在后）；
 * 未命中时退回 desc.<category> 的类别描述。
 *
 * 第 4 元 scope：
 * - 缺省（身份型）：匹配身份字段（app_label + command）。dev 工具自身就是
 *   dev-script，必须能命中。
 * - "path"（路径结构型）：target/debug、code helper 这类身份只存在于完整
 *   命令行 / exe 路径里 —— 用宽 haystack，且不会被项目目录名误触（评审发现）。
 * - "brand"（品牌型）：桌面软件 / 数据库等品牌词。app_label 含项目目录名
 *  （identify.rs 生成「项目 · 脚本」），~/code/spotify-clone 会让 \bspotify\b
 *   在身份字段命中 —— 真品牌进程永远不是 dev-script 类别（后端不变量：
 *   node_modules .app 归 dev-script、真应用归 installed-app），故 dev-script
 *   行直接跳过品牌组，落到泛化运行时描述（评审发现：初版修复漏了 app_label）。
 */
const KNOWN_PROCESSES: ReadonlyArray<
  readonly [RegExp, string, string] | readonly [RegExp, string, string, "path" | "brand"]
> = [
  // —— 开发服务器 / 框架（先于泛化的 node/python；自身就是 dev-script，身份型）——
  [/vite/, "Vite 前端开发服务器", "Vite frontend dev server"],
  [/webpack/, "Webpack 前端开发服务", "Webpack dev server"],
  [/next dev|next-server|next start/, "Next.js 开发服务器", "Next.js dev server"],
  [/nuxt/, "Nuxt 开发服务器", "Nuxt dev server"],
  [/uvicorn|gunicorn|fastapi|flask|django/, "Python Web 服务", "Python web service"],
  [/http\.server/, "Python 临时文件服务器", "Python ad-hoc file server"],
  [/jupyter/, "Jupyter 笔记本服务", "Jupyter notebook server"],
  [/storybook/, "Storybook 组件预览服务", "Storybook preview server"],
  // —— 数据库 / 服务（品牌型：node 脚本项目名含 redis/postgres 不该被描述成数据库）——
  [/postgres/, "PostgreSQL 数据库", "PostgreSQL database", "brand"],
  [/mysqld|mariadb/, "MySQL 数据库", "MySQL database", "brand"],
  [/mongod/, "MongoDB 数据库", "MongoDB database", "brand"],
  [/\bredis\b/, "Redis 数据库", "Redis database", "brand"],
  [/nginx/, "Nginx Web 服务器", "Nginx web server", "brand"],
  [/caddy/, "Caddy Web 服务器", "Caddy web server", "brand"],
  [/docker|containerd/, "Docker 容器服务", "Docker container service", "brand"],
  [/ollama/, "Ollama 本地 AI 模型服务", "Ollama local AI model server", "brand"],
  // —— 常见桌面软件（带 \b 词界防止子串误匹配；品牌型 —— 桌面 App 永远不是 dev-script）——
  [/wechat|weixin/, "微信", "WeChat messenger", "brand"],
  [/wxwork|wework/, "企业微信", "WeCom", "brand"],
  [/qqmusic/, "QQ 音乐", "QQ Music", "brand"],
  [/\bqq\b/, "QQ", "QQ messenger", "brand"],
  [/dingtalk/, "钉钉", "DingTalk", "brand"],
  [/feishu|\blark\b/, "飞书", "Lark / Feishu", "brand"],
  [/cloudmusic|neteasemusic/, "网易云音乐", "NetEase Cloud Music", "brand"],
  [/wemeet|tencentmeeting/, "腾讯会议", "Tencent Meeting", "brand"],
  [/todesk/, "ToDesk 远程控制", "ToDesk remote desktop", "brand"],
  [/clash|v2ray|xray|sing-box|shadowsocks|trojan/, "网络代理工具", "network proxy tool", "brand"],
  [/raycast/, "Raycast 快捷启动工具", "Raycast launcher", "brand"],
  [/alfred/, "Alfred 快捷启动工具", "Alfred launcher", "brand"],
  [/\bspotify\b/, "Spotify 音乐", "Spotify music", "brand"],
  [/\bsteam\b/, "Steam 游戏平台", "Steam gaming platform", "brand"],
  [/onedrive/, "OneDrive 网盘同步", "OneDrive sync", "brand"],
  [/dropbox/, "Dropbox 网盘同步", "Dropbox sync", "brand"],
  [/baidunetdisk/, "百度网盘", "Baidu Netdisk", "brand"],
  // "code helper" 只出现在 exe 路径（VS Code 渲染/扩展子进程）—— 走宽 haystack
  [/code helper|visual studio code/, "VS Code 代码编辑器", "VS Code editor", "path"],
  [/\bcursor\b/, "Cursor 代码编辑器", "Cursor editor", "brand"],
  [/iterm/, "iTerm 终端", "iTerm terminal", "brand"],
  [/\bwarp\b/, "Warp 终端", "Warp terminal", "brand"],
  // —— macOS 系统组件（品牌型同理）——
  [/controlcenter/, "macOS 控制中心（系统组件）", "macOS Control Center (system)", "brand"],
  [/rapportd/, "苹果设备互联服务（接力 / 隔空）", "Apple continuity service", "brand"],
  [/sharingd/, "macOS 共享服务", "macOS sharing service", "brand"],
  [/airplay/, "隔空播放服务", "AirPlay service", "brand"],
  // —— 泛化运行时（永远放最后；\b 词界防止把无关二进制误标）——
  // cargo / target/(debug|release) 只出现在 exe 路径或完整命令行 —— 走宽 haystack。
  // 分隔符两路都匹配（Windows 是 target\debug），与后端 is_dev_build_artifact 对齐。
  [/cargo|target[\\/](debug|release)/, "Rust 开发程序", "Rust dev program", "path"],
  [/\bnode\b|\bnpm\b|\bpnpm\b|\byarn\b|\bbun\b/, "Node.js 程序", "Node.js program"],
  [/\bpython/, "Python 程序", "Python program"],
  [/\bjava\b|gradle|tomcat/, "Java 程序", "Java program"],
  [/\bruby\b|\brails\b/, "Ruby 程序", "Ruby program"],
  [/\bphp\b/, "PHP 程序", "PHP program"],
];

/** 类别 → 兜底描述 key（知识库未命中时） */
export const DESC_KEYS: Record<string, I18nKey> = {
  "installed-app": "desc.installed-app",
  system: "desc.system",
  "dev-script": "desc.dev-script",
  "automation-instance": "desc.automation-instance",
  "user-binary": "desc.user-binary",
  unknown: "desc.unknown",
};

/** 品牌型模式要跳过的类别 —— 后端不变量：真品牌进程永远归 installed-app，
 *  这些类别的身份来自命令行 / 项目路径，品牌词命中必是误触
 *  （~/code/spotify-clone 的 dev server、`--user-data-dir=/tmp/steam-test`
 *  的无头浏览器）。automation-instance 与 dev-script 同理，见文件头注释。 */
const IDENTITY_FROM_COMMAND = new Set(["dev-script", "automation-instance"]);

/** app_label 形如 "dev-server.js · node"：拆主名 + 次级说明（Row 与 Detail 共用） */
export function splitLabel(appLabel: string): { name: string; sub: string | null } {
  const i = appLabel.indexOf(" · ");
  return i >= 0
    ? { name: appLabel.slice(0, i), sub: appLabel.slice(i + 3) }
    : { name: appLabel, sub: null };
}

/** 「这是什么」：知识库命中 → 友好名；未命中 → 类别描述兜底（null 由调用方翻译） */
export function describeEntry(e: ProcessEntry, lang: Lang): string | null {
  // 身份字段：app_label（含脚本/项目身份）+ command（运行时短名）。
  // 品牌型不含 exe_path / 完整路径，且 dev-script 类别直接跳过（见 KNOWN_PROCESSES 注释）。
  const identityHay = `${e.app_label} ${e.command}`.toLowerCase();
  // 路径结构型模式（target/debug、code helper，标 "path"）才用含完整命令行 + exe 路径的
  // 宽 haystack —— 这类身份只存在于路径里，且不会被项目目录名误触（评审发现：窄化后
  // Rust 产物 / VS Code 子进程的友好描述整体丢失）。
  const pathHay = `${identityHay} ${e.full_command} ${e.exe_path}`.toLowerCase();
  for (const [re, zh, enText, scope] of KNOWN_PROCESSES) {
    if (scope === "brand" && IDENTITY_FROM_COMMAND.has(e.app_category)) continue;
    const hay = scope === "path" ? pathHay : identityHay;
    if (re.test(hay)) return lang === "zh" ? zh : enText;
  }
  return null;
}
