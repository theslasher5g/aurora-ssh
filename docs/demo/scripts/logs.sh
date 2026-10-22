#!/bin/bash
# logs.gif: SSH session + log panel docked at the right (Ctrl+Shift+L), following a file that keeps growing.
# Needs: scripts/logwriter.sh start  (appends a line every 0.6 s) and a fresh app: scripts/reset.sh
. "$(dirname "$0")/lib.sh"
mv 700 450 10
rec_start logs 25
pause 0.8
key ctrl+k; pause 0.7; typ "aurora" 120; pause 0.8; key Return; pause 3.4
run 'cd demo && ls -lh logs' 1.6
key ctrl+shift+l; pause 1.6                              # dock the log panel
click 1020 197 0.4                                       # source field
typ '/home/aurora/demo/logs/app.log' 60; pause 0.6
key Return; pause 4.0                                    # lines start to scroll
click 1020 210 0.4; typ 'WARN' 150; pause 3.0            # filter (matches only)
key ctrl+a; typ 'ERROR|timeout' 120; pause 3.0
key ctrl+a; key BackSpace; pause 1.5
click 880 249 0.6; pause 3.0                             # "Warnings" quick filter
click 806 249 0.6; pause 3.0                             # back to "All"
rec_stop
SPEED=1.3 to_gif logs 10 960 48 0.4
