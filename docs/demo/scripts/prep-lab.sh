#!/bin/bash
# Prepare the test account `aurora` (local test sshd) with clean demo files and a nice prompt.
# Run as root once. Restore afterwards: cp /home/aurora/.bashrc.demo-bak /home/aurora/.bashrc
set -e
H=/home/aurora
[ -f $H/.bashrc.demo-bak ] || cp $H/.bashrc $H/.bashrc.demo-bak
cat > $H/.bashrc <<'EOF'
case $- in *i*) ;; *) return;; esac
set -- $SSH_CONNECTION; ip=$3
names=(lab web-01 web-02 db-primary db-replica fw-berlin print-srv app-hh-01 app-hh-02 mon-hh stage-app stage-db bastion-ldn k8s-node-1 k8s-node-2 gitea nas win-dc-01 win-ts-01 sw-core-01 kiosk-01)
case $ip in 127.0.0.1) hn=lab;; 127.0.0.*) hn=${names[$((${ip##*.} - 9))]};; *) hn=lab;; esac
PS1='\[\e[1;32m\]aurora\[\e[0m\]@\[\e[1;36m\]'$hn'\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '
alias ls='ls --color=auto'
alias ll='ls -lh --color=auto'
alias grep='grep --color=auto'
export HISTFILE=/dev/null
EOF
mkdir -p $H/demo/releases $H/demo/backups $H/demo/logs
cd $H/demo
echo '#!/bin/bash' > status.sh
cat >> status.sh <<'EOF'
printf '\e[1mService health check\e[0m\n'
printf '  \e[32m[ OK ]\e[0m nginx        active   uptime 41d\n'
printf '  \e[32m[ OK ]\e[0m postgresql    active   12 connections\n'
printf '  \e[33m[WARN]\e[0m disk /var     81%% used\n'
printf '  \e[32m[ OK ]\e[0m backup        last run 03:00\n'
printf '  \e[31m[FAIL]\e[0m certbot-renew timer inactive\n'
EOF
chmod +x status.sh
echo "server { listen 443 ssl; server_name example.com; }" > nginx.conf
echo "# Aurora demo" > README.md
head -c 2400000 /dev/zero > backups/db-2026-10-21.sql.gz
head -c 2350000 /dev/zero > backups/db-2026-10-20.sql.gz
head -c 18000 /dev/zero > releases/app-4.2.1.tar.gz
head -c 17500 /dev/zero > releases/app-4.2.0.tar.gz
cat > logwriter.sh <<'EOF'
#!/bin/bash
# appends a realistic log line every 0.6 s
f=/home/aurora/demo/logs/app.log
msgs=("GET /api/v1/hosts 200 12ms" "GET /health 200 1ms" "POST /api/v1/login 200 48ms" "GET /static/app.js 304 2ms" "WARN slow query (412 ms) on table sessions" "GET /api/v1/groups 200 9ms" "ERROR upstream timeout while connecting to 10.0.4.12:5432" "INFO cache refreshed (1432 entries)" "POST /api/v1/hosts 201 31ms" "WARN retrying request (attempt 2/3)" "INFO worker 3 finished job #8841" "GET /metrics 200 5ms")
i=0
while true; do
  m=${msgs[$((RANDOM % ${#msgs[@]}))]}
  echo "$(date '+%Y-%m-%d %H:%M:%S') $m" >> $f
  sleep 0.6
done
EOF
chmod +x logwriter.sh
for i in $(seq 1 40); do echo "$(date '+%Y-%m-%d %H:%M:%S') INFO startup step $i" >> logs/app.log; done
chown -R aurora:aurora $H/demo
