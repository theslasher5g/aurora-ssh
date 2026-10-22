#!/bin/bash
# split.gif: open four SSH sessions, then place them side by side (rail chip -> right-click ->
# "Place next to current session"), type in the panes.
. "$(dirname "$0")/lib.sh"
open_host() { key ctrl+k; pause 0.6; typ "$1" 100; pause 0.6; key Return; pause "${2:-3.4}"; }
place() {  # place CHIP_Y : right-click a rail chip and choose "Place next to current session"
  mv 36 $1 22; pause 0.3; xdotool click 3; pause 0.8
  mv 100 $(($1 + 25)) 14; pause 0.5; xdotool click 1; pause 1.4
}
mv 700 420 10
rec_start split 25
pause 0.8
open_host stage-app 2.4
open_host bastion 2.4
open_host node-1 2.4
open_host node-2 2.6
place 506          # chip 3 -> 2 panes
place 454          # chip 2 -> 3 panes
place 402          # chip 1 -> 4 panes
pause 0.6
click 375 600 0.4; run 'cd demo && ./status.sh' 1.6      # bottom-left pane
click 975 250 0.4; run 'ls -lh demo' 1.6                  # top-right pane
click 975 600 0.4; run 'uptime' 1.8                       # bottom-right pane
rec_stop
SPEED=1.35 to_gif split 10 960 64 0.3
