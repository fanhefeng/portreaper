// 测试共享的 ProcessEntry 夹具 —— serde 契约镜像在测试侧的**唯一**手抄点。
// 评审发现：此前 App.test / model.test / describe.test 三个文件各抄一份完整
// 字段集，引擎契约加字段要同步改三处（model.test 还专门测「字段缺失退化」，
// 说明这层脆弱性是已知的）。收敛后加字段只改这里；各测试文件用语义化 override
// 表达自己的场景（chrome 夹具 / server.js 夹具 / 可 kill 的 Confirmed 嫌疑）。
//
// 命名刻意避开 *.test.*：vitest 的 include 只收 src/**/*.test.{ts,tsx}，
// 本文件是夹具库，不含任何测试，不能被当作测试文件收集。
import type { ProcessEntry } from "./model";

/** 缺省形态：一行可被 kill 的 Confirmed 嫌疑（vite dev server 孤儿）。 */
export function makeEntry(over: Partial<ProcessEntry> = {}): ProcessEntry {
  return {
    pid: 4242,
    ppid: 1,
    ports: [5173],
    command: "node",
    full_command: "node /Users/x/proj/node_modules/vite/bin/vite.js",
    exe_path: "/opt/homebrew/bin/node",
    app_label: "proj · vite.js",
    app_category: "dev-script",
    parent_chain: [],
    launcher_label: "launchd",
    user: "x",
    tty: "",
    elapsed_secs: 3600,
    start_unix: 1000,
    cpu_percent: 0,
    cpu_percent_tree: 0,
    mem_mb: 10,
    state: "S",
    is_zombie_suspect: true,
    confidence: "confirmed",
    zombie_reasons: ["ppid1_orphan", "dev_server_keyword"],
    is_whitelisted: false,
    // 引擎随每行产出的白名单键（前端直读、不重推）。exe_path 含路径分隔符 ⇒ 用它。
    whitelist_key: "/opt/homebrew/bin/node",
    duplicate_of: null,
    ...over,
  };
}
