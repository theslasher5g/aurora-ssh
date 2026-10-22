#!/bin/bash
# inventory.gif: customer -> site -> host tree, expand/collapse, "only this customer" focus, search language.
# Start from a fresh app: scripts/reset.sh
. "$(dirname "$0")/lib.sh"
scroll() { for ((i = 0; i < $2; i++)); do xdotool click "$1"; sleep 0.15; done; }
mv 760 520 10
rec_start inventory 25
pause 0.8
scroll 5 3; pause 0.8; scroll 4 3; pause 0.5          # scroll the full tree down and up
click 282 229 0.5; click 282 281 0.5; click 282 333 0.5; click 282 385 1.2   # collapse the four customers
click 282 229 1.0                                      # expand Acme GmbH -> sites
click 302 317 1.4                                      # expand Frankfurt DC -> hosts
click 302 273 1.8                                      # expand Berlin Office -> hosts
click 282 229 1.0                                      # collapse Acme again
mv 362 386; pause 0.5; xdotool click 3; pause 1.0      # right-click Nordwind AG
mv 440 173 14; pause 0.9; xdotool click 1; pause 1.8   # "Show only this group"
pause 1.6; click 282 277 1.6                          # Hamburg collapses, Munich stays
click 630 52 1.6                                       # leave focus
click 810 52 0.4                                       # search box
typ 'type:ssh' 130; pause 1.8
key ctrl+a; typ 'type:rdp' 130; pause 2.0
key ctrl+a; typ 'env:prod' 130; pause 2.0
key ctrl+a; typ '#postgres' 130; pause 2.0
key ctrl+a; key BackSpace; pause 1.2
rec_stop
SPEED=1.3 to_gif inventory 10 960 64 0.4
