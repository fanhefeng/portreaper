#!/bin/bash
# Store 截图用的演示进程（维护者工具，不随扩展提交）。
#
# 造 6 个被 launchd 收养的孤儿 dev server，正好覆盖截图里要出现的全部徽标：
#   e2e-suite    无端口（`no port`）
#   api-gateway  :3000
#   docs-site    :4321，随后 SIGSTOP（`stopped`）
#   web-app      :5173
#   storefront   :5180 + :5181（互为 `dup of`）
#
# 四个不显然、缺一不可的点（每个都踩过，见 docs/RAYCAST-MAINTAINING.md）：
#   - 必须用绝对脚本路径启动，且路径形如 /Users/<user>/…/<项目>/src/…，
#     否则 extract_project_name 取不到项目名，标签退化成 `dev-server.js · node`。
#   - 每个进程从自己的项目目录启动，全从同一 cwd 启动会被判成互为重复实例。
#   - 放在 /tmp 不行：extract_project_name 只认 /Users/ 下的路径。
#   - 脚本名 dev-server.js 同时充当截图里的搜索过滤词，把开发机上的真实进程挡在画面外。
#     启动前先确认它在本机零命中。
#
# 用法: scripts/raycast-screenshot-fixtures.sh start | stop
set -euo pipefail

ROOT="$HOME/pr-demo-screens"
CLI="$HOME/Library/Application Support/com.raycast.macos/extensions/portreaper/bin/portreaper-cli"

start() {
  if pgrep -f "dev-server.js" >/dev/null; then
    echo "已有 dev-server.js 进程在跑，先 stop" >&2
    exit 1
  fi
  if [ -x "$CLI" ] && [ "$("$CLI" scan --json --cpu=skip 2>/dev/null | grep -c dev-server)" != 0 ]; then
    echo "本机已有命中 dev-server 的真实进程，截图会把它带进画面，先处理掉" >&2
    exit 1
  fi
  for p in web-app api-gateway docs-site e2e-suite storefront; do
    mkdir -p "$ROOT/$p/src"
    cat > "$ROOT/$p/src/dev-server.js" <<'JS'
// dev-server.js — screenshot fixture
const port = process.argv[2];
process.on("SIGTERM", () => process.exit(0)); // 捕获信号，才做得出 stopped 那一档
if (port && port !== "--no-listen") {
  require("node:net").createServer(() => {}).listen(Number(port), "127.0.0.1");
}
setInterval(() => {}, 1 << 30);
JS
  done
  launch() { (cd "$ROOT/$1/src" && nohup node "$ROOT/$1/src/dev-server.js" "$2" >/dev/null 2>&1 &); }
  launch web-app 5173
  launch api-gateway 3000
  launch docs-site 4321
  launch e2e-suite --no-listen
  launch storefront 5180
  launch storefront 5181
  sleep 12 # 跨过 10s 宽限期，否则全是 possible
  kill -STOP "$(pgrep -f 'dev-server.js 4321')"
  echo "6 个演示进程已就绪（docs-site 已挂起）。用完记得 stop。"
}

stop() {
  pkill -CONT -f "dev-server.js" 2>/dev/null || true
  pkill -f "dev-server.js" 2>/dev/null || true
  rm -rf "$ROOT"
  echo "演示进程已清理，$ROOT 已删除。"
}

case "${1:-}" in
  start) start ;;
  stop) stop ;;
  *) echo "用法: $0 start | stop" >&2; exit 2 ;;
esac
