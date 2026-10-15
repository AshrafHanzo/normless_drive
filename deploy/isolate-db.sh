#!/bin/bash
# Stops each application's Postgres role from reaching the other's database.
#
# Postgres grants CONNECT on every database to PUBLIC by default, so a brand
# new role can open any database on the server. For two unrelated apps on one
# host that is the whole isolation story missing: a SQL-injection bug in
# either one would reach the other's data.
#
# Revoking from PUBLIC does not lock out an owner — owners keep access
# regardless — and each role is granted its own database explicitly anyway,
# so this cannot strand either application.

set -euo pipefail

echo "=== before ==="
sudo -u postgres psql -P pager=off -c \
  "SELECT datname,
          pg_catalog.pg_get_userbyid(datdba) AS owner,
          datacl
     FROM pg_database
    WHERE datname IN ('normless_crm', 'normless_drive');"

echo
echo "=== revoking PUBLIC connect, granting each role its own database ==="
sudo -u postgres psql -q <<'SQL'
REVOKE CONNECT ON DATABASE normless_crm   FROM PUBLIC;
REVOKE CONNECT ON DATABASE normless_drive FROM PUBLIC;

GRANT  CONNECT ON DATABASE normless_crm   TO normless;
GRANT  CONNECT ON DATABASE normless_drive TO normless_drive;
SQL

echo
echo "=== after ==="
sudo -u postgres psql -P pager=off -c \
  "SELECT datname,
          pg_catalog.pg_get_userbyid(datdba) AS owner,
          datacl
     FROM pg_database
    WHERE datname IN ('normless_crm', 'normless_drive');"

echo
echo "=== verifying both applications still work, and cannot cross over ==="

check() {
    local role="$1" db="$2" expect="$3" password="$4"
    if PGPASSWORD="$password" psql -h 127.0.0.1 -U "$role" -d "$db" -tAc "SELECT 1" >/dev/null 2>&1; then
        actual=allowed
    else
        actual=denied
    fi
    if [ "$actual" = "$expect" ]; then
        echo "  OK   $role -> $db : $actual"
    else
        echo "  FAIL $role -> $db : $actual (expected $expect)"
    fi
}

check normless_drive normless_drive allowed "$DRIVE_PASSWORD"
check normless_drive normless_crm   denied  "$DRIVE_PASSWORD"

if [ -n "${CRM_PASSWORD:-}" ]; then
    check normless normless_crm   allowed "$CRM_PASSWORD"
    check normless normless_drive denied  "$CRM_PASSWORD"
else
    echo "  (CRM_PASSWORD not supplied — skipping the CRM's own checks)"
fi
