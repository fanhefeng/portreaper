// describeEntry 知识库回归（评审 H 级发现）：
// 品牌型模式不得被项目目录名误触 —— identify.rs 生成的 app_label 是
// 「项目目录名 · 脚本名」，~/code/spotify-clone 会让 \bspotify\b 在身份字段
// 命中，把一个孤儿 dev server 误描述成「Spotify 音乐」（用户可能因此不敢杀）。
// 修复语义：dev-script 类别跳过品牌组（真品牌进程永远不是 dev-script —— 后端不变量）。
import { describe, it, expect } from "vite-plus/test";
import { describeEntry } from "./describe";
import type { ProcessEntry } from "./model";

function entry(over: Partial<ProcessEntry>): ProcessEntry {
  return {
    pid: 1,
    ppid: 1,
    ports: [5173],
    command: "node",
    full_command: "node /Users/x/proj/server.js",
    exe_path: "/opt/homebrew/bin/node",
    app_label: "proj · server.js",
    app_category: "dev-script",
    parent_chain: [],
    launcher_label: "?",
    user: "",
    tty: "",
    elapsed_secs: 3600,
    start_unix: 1000,
    cpu_percent: 0,
    cpu_percent_tree: 0,
    mem_mb: 0,
    state: "S",
    is_zombie_suspect: true,
    confidence: "confirmed",
    zombie_reasons: [],
    is_whitelisted: false,
    whitelist_key: "/opt/homebrew/bin/node",
    duplicate_of: null,
    ...over,
  };
}

describe("describeEntry brand scope", () => {
  it("项目目录名含品牌词的 dev-script 不被误描述成该品牌", () => {
    const e = entry({
      app_label: "spotify-clone · server.js",
      full_command: "node /Users/x/code/spotify-clone/server.js",
    });
    // 落到泛化运行时描述，而非「Spotify 音乐」
    expect(describeEntry(e, "zh")).toBe("Node.js 程序");
    expect(describeEntry(e, "en")).toBe("Node.js program");
  });

  it("微信/redis 等品牌词项目名同理不误触", () => {
    expect(
      describeEntry(
        entry({
          app_label: "wechat-bot · index.js",
          full_command: "node /Users/x/code/wechat-bot/index.js",
        }),
        "zh",
      ),
    ).toBe("Node.js 程序");
    expect(
      describeEntry(
        entry({
          app_label: "redis-clone · server.js",
          full_command: "node /Users/x/code/redis-clone/server.js",
        }),
        "zh",
      ),
    ).toBe("Node.js 程序");
  });

  it("真品牌进程（非 dev-script 类别）仍正常命中品牌描述", () => {
    const spotify = entry({
      app_label: "Spotify",
      command: "Spotify",
      full_command: "/Applications/Spotify.app/Contents/MacOS/Spotify",
      exe_path: "/Applications/Spotify.app/Contents/MacOS/Spotify",
      app_category: "installed-app",
    });
    expect(describeEntry(spotify, "zh")).toBe("Spotify 音乐");

    const redis = entry({
      app_label: "redis-server",
      command: "redis-server",
      full_command: "/opt/homebrew/bin/redis-server *:6379",
      exe_path: "/opt/homebrew/bin/redis-server",
      app_category: "user-binary",
    });
    expect(describeEntry(redis, "zh")).toBe("Redis 数据库");
  });

  it("dev 工具自身（身份型）与路径结构型在 dev-script 下不受品牌跳过影响", () => {
    // vite 本来就是 dev-script —— 必须保持命中
    const vite = entry({
      app_label: "proj · vite.js",
      full_command: "node /Users/x/proj/node_modules/vite/bin/vite.js",
    });
    expect(describeEntry(vite, "zh")).toBe("Vite 前端开发服务器");

    // Rust 产物（path 型）也是 dev-script —— 必须保持命中
    const cargo = entry({
      app_label: "mytool",
      command: "mytool",
      full_command: "/Users/x/rust/mytool/target/debug/mytool",
      exe_path: "/Users/x/rust/mytool/target/debug/mytool",
    });
    expect(describeEntry(cargo, "zh")).toBe("Rust 开发程序");
  });
});
