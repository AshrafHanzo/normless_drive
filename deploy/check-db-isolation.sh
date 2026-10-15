#!/bin/bash
# READ-ONLY. Reports who owns each database and who may connect to it.
# Changes nothing.
set -euo pipefail

echo "=== database ownership and ACLs ==="
sudo -u postgres psql -P pager=off -c \
  "SELECT datname,
          pg_catalog.pg_get_userbyid(datdba) AS owner,
          COALESCE(datacl::text, '(default: PUBLIC may connect)') AS acl
     FROM pg_database
    WHERE datname IN ('normless_crm', 'normless_drive');"

echo "=== roles on this server ==="
sudo -u postgres psql -P pager=off -c \
  "SELECT rolname, rolsuper, rolcreatedb, rolcanlogin
     FROM pg_roles
    WHERE rolname NOT LIKE 'pg\_%'
    ORDER BY rolname;"

echo "=== which role does the CRM connect as? ==="
grep -oP 'postgresql://\K[^:]+' /root/normless_crm/.env 2>/dev/null \
  | head -1 \
  | sed 's/^/  CRM connects as role: /' \
  || echo "  could not read it"

echo "=== active connections by database and role ==="
sudo -u postgres psql -P pager=off -c \
  "SELECT datname, usename, count(*)
     FROM pg_stat_activity
    WHERE datname IS NOT NULL
    GROUP BY datname, usename
    ORDER BY datname;"
