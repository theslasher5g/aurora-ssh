#!/bin/bash
# logwriter.sh start|stop - appends a log line every 0.6 s to /home/aurora/demo/logs/app.log (as user aurora)
case "$1" in
  start)
    setsid su aurora -c '/home/aurora/demo/logwriter.sh' >/dev/null 2>&1 &
    echo $! > /tmp/demo/lw.pid ;;
  stop)
    pkill -u aurora -f demo/logwriter.sh ;;
esac
