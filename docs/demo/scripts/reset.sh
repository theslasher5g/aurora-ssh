#!/bin/bash
# reset.sh [THEME] - stop the app, rebuild the demo inventory (all groups expanded), restart + unlock.
D=$(dirname "$0")
[ -f /tmp/demo/app.pid ] && kill "$(cat /tmp/demo/app.pid)" 2>/dev/null
sleep 1
python3 "$D/mkdata.py" "${DATA:-/tmp/demo/data}" "${1:-nord}"
bash "$D/start-app.sh"
