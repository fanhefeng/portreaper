#!/bin/bash
# Portreaper 解除隔离助手 / Gatekeeper quarantine remover
#
# 为什么存在：应用未签名公证，浏览器下载的 dmg 带 com.apple.quarantine，
# 拖出的 App 会被 Gatekeeper 报「已损坏，无法打开」。本脚本代替用户在
# 终端手敲 xattr 命令。
#
# 本脚本自身也随 dmg 下载、同样带隔离标记 —— 双击被拦时右键 → 打开即可
# （.command 走「身份不明的开发者」对话框，右键可放行；这与 ad-hoc 签名
# App 的「已损坏」不同，后者右键也无效 —— 这正是需要本脚本的原因）。
set -u

APP="/Applications/Portreaper.app"

echo "================================================="
echo "  Portreaper 解除隔离助手 / Quarantine Remover"
echo "================================================="
echo

if [ ! -d "$APP" ]; then
  echo "✗ 没找到 $APP"
  echo "  请先把 Portreaper 拖进「应用程序」文件夹，再运行本脚本。"
  echo "  Drag Portreaper into /Applications first, then run this again."
  echo
  read -r -n 1 -s -p "按任意键退出 / press any key to exit "
  echo
  exit 1
fi

xattr -dr com.apple.quarantine "$APP" 2>/dev/null

# 递归复查：quarantine 还在（极少见：目录权限问题）就退回手动路径
if xattr -lr "$APP" 2>/dev/null | grep -q com.apple.quarantine; then
  echo "✗ 解除失败。请在终端手动执行 / Removal failed — run manually:"
  echo "  xattr -dr com.apple.quarantine \"$APP\""
  echo
  read -r -n 1 -s -p "按任意键退出 / press any key to exit "
  echo
  exit 1
fi

echo "✓ 已解除隔离，正在启动… / Quarantine removed, launching…"
open "$APP"
echo
echo "App 常驻菜单栏（无 Dock 图标），本窗口可以关闭了。"
echo "Portreaper lives in the menu bar (no Dock icon); you may close this window."
