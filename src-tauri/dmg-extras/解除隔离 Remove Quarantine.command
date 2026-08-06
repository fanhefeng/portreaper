#!/bin/bash
# Portreaper 解除隔离助手 / Gatekeeper quarantine remover
#
# 为什么存在：应用未签名公证，浏览器下载的 dmg 带 com.apple.quarantine，
# 拖出的 App 会被 Gatekeeper 报「已损坏，无法打开」。本脚本代替用户在
# 终端手敲 xattr 命令。
#
# 本脚本自身也随 dmg 下载、同样带隔离标记。双击被拦时的放行路径按系统分：
# macOS 14 及以前：右键 → 打开（走「身份不明的开发者」对话框）；
# macOS 15 起：Apple 移除了右键旁路 —— 双击被拦后到 系统设置 → 隐私与安全性
# 底部点「仍要打开」。这与 ad-hoc 签名 App 的「已损坏」不同，后者上述两条
# 路都无效 —— 这正是需要本脚本的原因。
set -u

# 找不到 /Applications 时再探 ~/Applications（无管理员权限的用户装在这里）
APP="/Applications/Portreaper.app"
if [ ! -d "$APP" ] && [ -d "$HOME/Applications/Portreaper.app" ]; then
  APP="$HOME/Applications/Portreaper.app"
fi

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
if ! open "$APP"; then
  echo "  启动没成功 —— 请到「应用程序」里手动打开 Portreaper。"
  echo "  Launch failed — open Portreaper from the Applications folder manually."
fi
echo
echo "App 常驻菜单栏（无 Dock 图标），本窗口可以关闭了。"
echo "Portreaper lives in the menu bar (no Dock icon); you may close this window."
