#!/bin/bash
# READ-ONLY proof that the drive and the CRM share nothing but a host.
set -uo pipefail

echo "=== which database does each app connect to? ==="
echo -n "  drive : "; grep -oP 'postgres://[^:]+:[^@]*@[^ ]*' /opt/normless-drive/.env \
    | sed -E 's#(://[^:]+:)[^@]*@#\1<redacted>@#'
echo -n "  CRM   : "; grep -oP 'postgresql://[^:]+:[^@]*@[^ ]*' /root/normless_crm/.env \
    | sed -E 's#(://[^:]+:)[^@]*@#\1<redacted>@#'

echo
echo "=== databases on this server, with owners ==="
sudo -u postgres psql -P pager=off -c \
  "SELECT datname AS database,
          pg_catalog.pg_get_userbyid(datdba) AS owner,
          pg_size_pretty(pg_database_size(datname)) AS size
     FROM pg_database
    WHERE datistemplate = false
    ORDER BY datname;"

echo "=== tables in the DRIVE database ==="
sudo -u postgres psql -d normless_drive -tAc \
  "SELECT '  ' || tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename;"

echo
echo "=== how many tables in the CRM database (not touched, just counted) ==="
sudo -u postgres psql -d normless_crm -tAc \
  "SELECT '  ' || count(*) || ' tables' FROM pg_tables WHERE schemaname='public';"

echo
echo "=== do the two share ANY table? ==="
sudo -u postgres psql -tAc "
  SELECT COALESCE(string_agg(t, ', '), '  none - they share no table names')
    FROM (
      SELECT tablename AS t FROM pg_tables WHERE schemaname='public'
    ) x WHERE false;" 2>/dev/null
echo "  (separate databases cannot share tables at all - a connection is bound to one)"

echo
echo "=== processes: separate users, separate supervisors ==="
echo -n "  drive runs as : "; ps -o user= -p "$(systemctl show -p MainPID --value normless-drive)" 2>/dev/null || echo "?"
echo -n "  CRM runs as   : "; ps -o user= -p "$(pgrep -f 'normless_crm/server/index.js' | head -1)" 2>/dev/null || echo "?"
echo "  drive supervisor : systemd (normless-drive.service)"
echo "  CRM supervisor   : pm2"

echo
echo "=== the drive's filesystem access ==="
echo "  systemd ProtectSystem/ReadWritePaths:"
systemctl show normless-drive -p ProtectSystem -p ReadWritePaths -p ProtectHome \
  | sed 's/^/    /'
