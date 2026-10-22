#!/bin/bash
# connect.gif: Ctrl+K palette -> open SSH session on aurora-lab -> run a few commands
. "$(dirname "$0")/lib.sh"
mv 760 420 20
rec_start connect 25
pause 1.2
mv 640 300 20; pause 0.4
key ctrl+k; pause 1.0
typ "aurora" 140; pause 1.4
key Return                      # opens the SSH session
pause 3.2
run 'cd demo' 1.0
run 'ls -lh' 2.0
run './status.sh' 2.8
run 'grep -n --color WARN logs/app.log | tail -3' 2.2
run 'ls backups releases' 2.8
rec_stop
to_gif connect 12 960 128 0.4
