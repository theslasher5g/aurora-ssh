#!/bin/bash
# regif.sh NAME [FPS] [WIDTH] [COLORS] [TRIM_START]  - re-encode $OUT/NAME.mkv to GIF (env SPEED=1.3 speeds up)
. "$(dirname "$0")/lib.sh"
to_gif "$@"
ffprobe -v error -show_entries format=duration -of csv=p=0 "$OUT/$1.gif"
