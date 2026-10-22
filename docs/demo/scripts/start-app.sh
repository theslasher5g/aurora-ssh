#!/bin/bash
# start-app.sh [THEME]  - (re)start Aurora on :81 with the demo data, set the theme, unlock the vault.
# Env: APP=/tmp/demo/aurora DATA=/tmp/demo/data PW=<vault password>
export DISPLAY=:81
APP=${APP:-/tmp/demo/aurora}; DATA=${DATA:-/tmp/demo/data}; PW=${PW:-SuperSecret#2026}
if [ -f /tmp/demo/app.pid ]; then kill "$(cat /tmp/demo/app.pid)" 2>/dev/null; sleep 1; fi
if [ -n "$1" ]; then
  python3 - "$DATA/library.json" "$1" <<'EOF'
import json, sys
p = sys.argv[1]; d = json.load(open(p)); d['settings']['theme'] = sys.argv[2]
d['settings']['last_session'] = []; json.dump(d, open(p, 'w'))
EOF
fi
LIBGL_ALWAYS_SOFTWARE=1 AURORA_DATA_DIR=$DATA $APP >/tmp/demo/app.log 2>&1 &
echo $! > /tmp/demo/app.pid
for i in $(seq 1 40); do WIN=$(xdotool search --name '^Aurora' | head -1); [ -n "$WIN" ] && break; sleep 0.5; done
sleep 4
xdotool windowmove "$WIN" 0 0 windowsize "$WIN" 1280 800; sleep 1.5
xdotool windowfocus "$WIN"; sleep 0.5; xdotool mousemove 295 367 click 1; sleep 0.5
xdotool type --delay 100 "$PW"; sleep 0.5; xdotool key Return; sleep 4
xdotool mousemove 1000 780
