#!/bin/bash
# READ-ONLY health snapshot of everything already running on this host.
#
# Run before and after deploying Normless Drive and diff the output. The
# point is to be able to prove the CRM was untouched, rather than assume it.
set -uo pipefail

echo "### timestamp"
date -Is

echo
echo "### pm2 processes"
pm2 jlist 2>/dev/null \
  | python3 -c "import sys,json;[print(f\"{p['name']:20} {p['pm2_env']['status']:10} restarts={p['pm2_env']['restart_time']}\") for p in json.load(sys.stdin)]" \
  2>/dev/null || pm2 list 2>/dev/null | head -20

echo
echo "### systemd services of interest"
for unit in nginx postgresql@16-main normless-drive; do
    state=$(systemctl is-active "$unit" 2>/dev/null || echo "not-found")
    printf '%-24s %s\n' "$unit" "$state"
done

echo
echo "### listening ports"
ss -tlnp 2>/dev/null | awk 'NR>1 {print $4, $6}' | sort -u

echo
echo "### nginx config validity"
nginx -t 2>&1 | tail -2

echo
echo "### nginx server_names configured"
grep -rhoP '^\s*server_name\s+\K[^;]+' /etc/nginx/sites-enabled/ 2>/dev/null | sort -u

echo
echo "### CRM reachability (localhost:5000)"
code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:5000/ 2>/dev/null || echo "no-response")
echo "HTTP $code"

echo
echo "### CRM database size and table count"
sudo -u postgres psql -tAc \
  "SELECT pg_size_pretty(pg_database_size('normless_crm'));" 2>/dev/null
sudo -u postgres psql -tAd normless_crm -c \
  "SELECT count(*) || ' tables' FROM pg_tables WHERE schemaname='public';" 2>/dev/null

echo
echo "### disk"
df -h / | tail -1

echo
echo "### memory"
free -h | sed -n '2p'
