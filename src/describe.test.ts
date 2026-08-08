// describeEntry 知识库回归（评审 H 级发现）：
// 品牌型模式不得被项目目录名误触 —— identify.rs 生成的 app_label 是
// 「项目目录名 · 脚本名」，~/code/spotify-clone 会让 \bspotify\b 在身份字段
// 命中，把一个孤儿 dev server 误描述成「Spotify 音乐」（用户可能因此不敢杀）。
// 修复语义：dev-script 类别跳过品牌组（真品牌进程永远不是 dev-script —— 后端不变量）。
import { describe, it, expect } from "vite-plus/test";
import { describeEntry } from "./describe";
import type { ProcessEntry } from "./model";
import { makeEntry } from "./test-fixtures";

/** 语义化夹具：无启动链信息的 server.js 孤儿（describeEntry 只看身份字段） */
function entry(over: Partial<ProcessEntry>): ProcessEntry {
  return makeEntry({
    full_command: "node /Users/x/proj/server.js",
    app_label: "proj · server.js",
    launcher_label: "?",
    user: "",
    mem_mb: 0,
    zombie_reasons: [],
    ...over,
  });
}

describe("describeEntry identity-pattern word boundaries", () => {
  // 身份型模式（vite/webpack/nuxt）不像品牌型那样对 dev-script 跳过 ——
  // 它们本来就用来描述 dev-script，只能靠 \b 防子串误触。
  it("项目目录名含 vite 子串不被描述成 Vite 开发服务器", () => {
    const e = entry({
      app_label: "invite-portal · server.js",
      full_command: "node /Users/x/code/invite-portal/server.js",
    });
    expect(describeEntry(e, "zh")).toBe("Node.js 程序");
    expect(describeEntry(e, "en")).toBe("Node.js program");
  });

  it("真实 vite / nuxt 进程仍正常命中", () => {
    const v = entry({
      app_label: "myapp · vite",
      full_command: "node /Users/x/myapp/node_modules/.bin/vite --port 5173",
    });
    expect(describeEntry(v, "zh")).toBe("Vite 前端开发服务器");

    const n = entry({
      app_label: "shop · nuxt",
      full_command: "node /Users/x/shop/node_modules/nuxt/bin/nuxt.mjs dev",
    });
    expect(describeEntry(n, "zh")).toBe("Nuxt 开发服务器");
  });
});

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

  it("自动化实例的项目名/临时 profile 含品牌词也不误触", () => {
    // 无头浏览器的身份来自命令行，品牌词只可能来自它拿到的参数 ——
    // --user-data-dir 指向一个叫 steam-test 的临时目录，不代表这是 Steam
    const e = entry({
      app_label: "Chromium",
      command: "Chromium",
      full_command:
        "/Applications/Chromium.app/Contents/MacOS/Chromium --headless " +
        "--remote-debugging-port=9222 --user-data-dir=/tmp/steam-test",
      exe_path: "/Applications/Chromium.app/Contents/MacOS/Chromium",
      app_category: "automation-instance",
    });
    expect(describeEntry(e, "zh")).not.toBe("Steam 游戏平台");
    expect(describeEntry(e, "en")).not.toBe("Steam gaming platform");
  });
});

describe("describeEntry cargo 词界", () => {
  it("项目目录名含 cargo 的 Node 程序不被说成 Rust 开发程序", () => {
    const e = entry({
      app_label: "cargo-cult · server.js",
      full_command: "node /Users/x/code/cargo-cult/server.js",
      exe_path: "/opt/homebrew/bin/node",
    });
    expect(describeEntry(e, "zh")).toBe("Node.js 程序");
  });

  it("住在 ~/.cargo/bin 的二进制不因路径就算 Rust 开发程序", () => {
    const e = entry({
      app_label: "sometool",
      command: "sometool",
      full_command: "/Users/x/.cargo/bin/sometool serve",
      exe_path: "/Users/x/.cargo/bin/sometool",
      app_category: "user-binary",
    });
    expect(describeEntry(e, "zh")).toBeNull();
  });

  it("真正的 cargo 调用仍然命中", () => {
    const run = entry({
      app_label: "myproj · cargo",
      command: "cargo",
      full_command: "cargo run --bin server",
      exe_path: "/Users/x/.rustup/toolchains/stable/bin/cargo",
    });
    expect(describeEntry(run, "zh")).toBe("Rust 开发程序");
  });
});
