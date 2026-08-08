#!/bin/bash
# Raycast Store 的 metadata 截图生成器（维护者工具，不随扩展提交）。
#
# 为什么需要脚本而不是「手动截图」：这件事有三个会静默毁掉成果的坑，
# 每一个都踩过一次，靠记忆规避不可靠 ——
#
#   1. 全屏截图后裁剪 ⇒ 桌面上其他窗口（终端里的私密内容）被一并带进图里。
#      本脚本只按 Raycast 窗口矩形取像素。
#   2. `window 1` 未必是主窗口 ⇒ ⌘K 动作面板、输入法候选条都是**独立窗口**，
#      取错矩形就会框进桌面内容。本脚本取面积最大的那个窗口。
#   3. 窗口圆角外仍是背景像素 ⇒ 直接贴到背景板上会露出四个角的桌面残影。
#      本脚本用圆角遮罩把窗口外的一切剔除，再合成。
#
# 另有一个与本脚本无关但同样会毁图的坑（调用方负责）：
#   `ray develop` 的 watcher 自己就是个无端口孤儿，会以 `raycast · node`
#   出现在嫌疑列表里。扩展一旦载入 Raycast，watcher 就可以停掉、命令仍可用 ——
#   截图前先 `pkill -f "ray develop"`。
#
# 依赖: ImageMagick (`brew install imagemagick`)、已授予终端「屏幕录制」权限。
# 用法: scripts/capture-raycast-metadata.sh <输出路径.png>
#
# 输出: 2000x1250 (16:10) sRGB PNG —— Raycast Store 对 metadata 截图的规格。
set -euo pipefail

OUT="${1:?用法: $0 <输出路径.png>}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

command -v magick >/dev/null || { echo "缺少 ImageMagick：brew install imagemagick" >&2; exit 1; }

# Raycast Beta 与稳定版进程名不同；两者都试，取跑着的那个。
APP=""
for candidate in "Raycast Beta" "Raycast"; do
  if osascript -e "tell application \"System Events\" to exists process \"$candidate\"" 2>/dev/null | grep -q true; then
    APP="$candidate"; break
  fi
done
[ -n "$APP" ] || { echo "Raycast 没在运行" >&2; exit 1; }

# 坑 2：取**面积最大**的窗口，而不是 window 1。
GEO=$(osascript <<AS
tell application "System Events" to tell process "$APP"
  set best to ""
  set bestArea to 0
  repeat with w in windows
    set sz to size of w
    set a to (item 1 of sz) * (item 2 of sz)
    if a > bestArea then
      set bestArea to a
      set pos to position of w
      set best to ((item 1 of pos) as text) & "," & ((item 2 of pos) as text) & "," & ((item 1 of sz) as text) & "," & ((item 2 of sz) as text)
    end if
  end repeat
  return best
end tell
AS
)
[ -n "$GEO" ] || { echo "取不到 $APP 的窗口矩形（窗口没显示？）" >&2; exit 1; }

# 坑 1：只截窗口矩形，不截全屏。
screencapture -x -R"$GEO" "$WORK/win.png"

PW=$(magick identify -format "%w" "$WORK/win.png")
PH=$(magick identify -format "%h" "$WORK/win.png")
R=26  # 圆角半径（Retina 像素）

# 坑 3：黑底白圆角矩形作 alpha —— 白=保留、黑=剔除，窗口圆角外的桌面像素归零。
magick -size "${PW}x${PH}" xc:black -fill white \
  -draw "roundrectangle 0,0 $((PW-1)),$((PH-1)) $R,$R" "$WORK/mask.png"
magick "$WORK/win.png" "$WORK/mask.png" \
  -alpha off -compose CopyOpacity -composite PNG32:"$WORK/rounded.png"

# 品牌渐变背景（取自应用图标：深青 → 深藏蓝）+ 投影 + 居中合成
magick -size 2000x1250 gradient:'#0d7d93-#16202e' PNG32:"$WORK/bg.png"
# 坑 4（评审发现）：窗口截图可能比画布还大 —— Retina 上一个铺开的 Raycast 窗口
# 轻松超过 2000px。`-composite` 居中合成时只会把超出画布的部分裁掉，产出尺寸
# 仍是规规矩矩的 2000x1250，ray lint 也照过，只是内容被切了边看不出来。
# 合成前先缩进安全区（留 100px 给投影扩散）；'>' 表示只缩不放，窗口本来就小就原样保留。
magick PNG32:"$WORK/rounded.png" -resize '1800x1100>' \
  \( +clone -background black -shadow 55x28+0+16 \) \
  +swap -background none -layers merge +repage PNG32:"$WORK/shadowed.png"
magick PNG32:"$WORK/bg.png" PNG32:"$WORK/shadowed.png" \
  -gravity center -composite -alpha remove -alpha off "$OUT"

magick identify -format "%f  %wx%h  %[colorspace]\n" "$OUT"
