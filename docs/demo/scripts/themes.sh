#!/bin/bash
# themes.sh [OUTDIR] : one screenshot per theme (OUTDIR/<id>.png, default docs/themes) of the same scene:
# SSH session with coloured output + docked log panel. Themes are switched live with Ctrl+K "Theme: <name>".
# Needs: scripts/logwriter.sh start and a fresh app (scripts/reset.sh). Theme list: scripts/themes.list
D=$(dirname "$0")
. "$D/lib.sh"
OUTDIR=${1:-$D/../../themes}; mkdir -p "$OUTDIR"
setup_scene() {
  mv 700 450 10; pause 0.5
  key ctrl+k; pause 0.7; typ "aurora" 120; pause 0.8; key Return; pause 3.4
  run 'cd demo && ls -lh' 1.2
  run './status.sh' 1.2
  run 'grep -n --color -E "WARN|ERROR" logs/app.log | tail -n 2' 1.2
  key ctrl+shift+l; pause 1.5
  click 1020 197 0.4; typ '/home/aurora/demo/logs/app.log' 40; pause 0.4; key Return; pause 3
}
set_theme() {   # set_theme NAME
  key ctrl+k; pause 0.7; typ "Theme: $1" 90; pause 0.8; key Return; pause 1.4
  mv 640 770 6; pause 0.6
}
setup_scene
while IFS='|' read -r id name; do
  set_theme "$name"
  shot "$OUT/theme-$id.png"
  convert "$OUT/theme-$id.png" -resize 960x -strip -colors 128 PNG8:"$OUTDIR/$id.png"
  echo "$id $(stat -c %s "$OUTDIR/$id.png")"
done < "$D/themes.list"
