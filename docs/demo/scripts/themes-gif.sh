#!/bin/bash
# themes.gif: the themes scene cycling through 8 themes via Ctrl+K "Theme: <name>".
# Needs: scripts/logwriter.sh start and a fresh app (scripts/reset.sh). Scene setup is done before recording.
D=$(dirname "$0")
. "$D/lib.sh"
mv 700 450 10; pause 0.5
key ctrl+k; pause 0.7; typ "aurora" 120; pause 0.8; key Return; pause 3.4
run 'cd demo && ls -lh' 1.2
run './status.sh' 1.2
run 'grep -n --color -E "WARN|ERROR" logs/app.log | tail -n 2' 1.2
key ctrl+shift+l; pause 1.5
click 1020 197 0.4; typ '/home/aurora/demo/logs/app.log' 40; pause 0.4; key Return; pause 3
bash "$D/logwriter.sh" stop; pause 1.5     # static log panel keeps the GIF small
rec_start themes 25
pause 1.2
for name in "Dracula" "Tokyo Night" "GitHub Light" "Gruvbox" "Paper" "Synthwave" "Nord"; do
  key ctrl+k; pause 0.5; typ "Theme: $name" 70; pause 0.5; key Return; pause 0.4
  mv 640 770 6; pause 1.4
done
rec_stop
# every theme has its own colours -> palette per frame keeps them accurate
ffmpeg -y -loglevel error -ss 0.3 -i "$OUT/themes.mkv" \
  -vf "setpts=PTS/${SPEED:-1.6},fps=${FPS:-6},scale=${W2:-720}:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=${COLORS:-128}:stats_mode=full[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
  -loop 0 "$OUT/themes.gif"
ls -l "$OUT/themes.gif"
