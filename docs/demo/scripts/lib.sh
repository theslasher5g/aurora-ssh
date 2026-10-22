#!/bin/bash
# Shared helpers for the demo drivers (source this file). Needs: xdotool, ffmpeg, ImageMagick.
# Environment: DISPLAY=:81 (Xvfb 1600x1000), app window 1280x800 at 0,0.
export DISPLAY=${DISPLAY:-:81} LC_ALL=C.UTF-8
W=${W:-1280}; H=${H:-800}
OUT=${OUT:-/tmp/demo/rec}; mkdir -p "$OUT"
WIN=$(xdotool search --name '^Aurora' | head -1)

pause() { sleep "$1"; }

# mv X Y [STEPS]  - smooth (ease in/out) mouse move from the current position
mv() {
  local x1=$1 y1=$2 n=${3:-28} i t s x0 y0
  eval "$(xdotool getmouselocation --shell)"; x0=$X; y0=$Y
  for ((i = 1; i <= n; i++)); do
    t=$((i * 1000 / n)); s=$((t * t * (3000 - 2 * t) / 1000000))
    xdotool mousemove $((x0 + (x1 - x0) * s / 1000)) $((y0 + (y1 - y0) * s / 1000))
    sleep 0.016
  done
}
# click X Y : move there smoothly, short beat, click
click() { mv "$1" "$2"; sleep 0.6; xdotool click 1; sleep "${3:-0.6}"; }
# typ TEXT [DELAY_MS]  - human-like typing
typ() { xdotool type --delay "${2:-90}" -- "$1"; }
# key KEYS...
key() { xdotool key "$@"; sleep 0.3; }
# run COMMAND : type a shell command + Enter, then wait
run() { typ "$1" 90; sleep 0.4; xdotool key Return; sleep "${2:-1.6}"; }

# rec_start NAME FPS  -> records region WxH to $OUT/NAME.mkv in the background
rec_start() {
  ffmpeg -y -loglevel error -f x11grab -framerate "${2:-25}" -draw_mouse 1 -video_size ${W}x${H} -i ${DISPLAY}+0,0 \
    -c:v libx264 -preset ultrafast -crf 14 "$OUT/$1.mkv" </dev/null &
  REC_PID=$!; sleep 0.6
}
rec_stop() { kill -INT $REC_PID; wait $REC_PID 2>/dev/null; }

# to_gif NAME [FPS] [WIDTH] [COLORS] [TRIM_START] -> $OUT/NAME.gif (looping, palette-optimised)
to_gif() {
  local n=$1 fps=${2:-12} w=${3:-960} c=${4:-128} ss=${5:-0}
  ffmpeg -y -loglevel error -ss "$ss" -i "$OUT/$n.mkv" \
    -vf "setpts=PTS/${SPEED:-1},fps=$fps,scale=$w:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=$c:stats_mode=diff[p];[b][p]paletteuse=dither=${DITHER:-none}:diff_mode=rectangle" \
    -loop 0 "$OUT/$n.gif"
  ls -l "$OUT/$n.gif"
}
shot() { import -window root -crop ${W}x${H}+0+0 "$1"; }
