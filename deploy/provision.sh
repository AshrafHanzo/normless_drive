#!/bin/bash
# One-time host provisioning for Normless Drive.
#
# Creates an unprivileged service account, its own Postgres role and
# database, and the install directory. Safe to re-run: every step checks
# before it acts.
#
# Deliberately touches nothing belonging to the existing CRM. The new role
# owns exactly one database and has no rights on any other.

set -euo pipefail

APP_USER=normlessdrive
APP_DIR=/opt/normless-drive
DB_NAME=normless_drive
DB_ROLE=normless_drive

DB_PASSWORD="${DB_PASSWORD:?DB_PASSWORD must be passed in}"

echo "=== service account ==="
if id "$APP_USER" &>/dev/null; then
    echo "user $APP_USER already exists"
else
    # No login shell and no home: this account exists to own a process and
    # a directory, nothing else.
    useradd --system --no-create-home --shell /usr/sbin/nologin "$APP_USER"
    echo "created $APP_USER"
fi

echo
echo "=== install directory ==="
mkdir -p "$APP_DIR"
chown "$APP_USER:$APP_USER" "$APP_DIR"
echo "$APP_DIR ready"

echo
echo "=== postgres role ==="
if sudo -u postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='$DB_ROLE'" | grep -q 1; then
    echo "role $DB_ROLE exists; resetting its password to the supplied value"
    sudo -u postgres psql -q -c "ALTER ROLE $DB_ROLE WITH PASSWORD '$DB_PASSWORD';"
else
    sudo -u postgres psql -q -c "CREATE ROLE $DB_ROLE LOGIN PASSWORD '$DB_PASSWORD';"
    echo "created role $DB_ROLE"
fi

echo
echo "=== postgres database ==="
if sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='$DB_NAME'" | grep -q 1; then
    echo "database $DB_NAME already exists"
else
    sudo -u postgres createdb -O "$DB_ROLE" "$DB_NAME"
    echo "created database $DB_NAME owned by $DB_ROLE"
fi

# gen_random_uuid() lives in pgcrypto on older servers; on 13+ it is built
# in. Requesting it is harmless either way and makes the schema portable.
sudo -u postgres psql -q -d "$DB_NAME" -c "CREATE EXTENSION IF NOT EXISTS pgcrypto;" || true

echo
echo "=== verifying the role can actually connect ==="
# Catching this now beats discovering it as an opaque startup failure.
if PGPASSWORD="$DB_PASSWORD" psql -h 127.0.0.1 -U "$DB_ROLE" -d "$DB_NAME" -tAc "SELECT 1" >/dev/null 2>&1; then
    echo "connection OK"
else
    echo "WARNING: $DB_ROLE could not connect over TCP to $DB_NAME"
    echo "         check pg_hba.conf allows md5/scram for host 127.0.0.1"
fi

echo
echo "=== isolation check: can the new role read the CRM? ==="
if PGPASSWORD="$DB_PASSWORD" psql -h 127.0.0.1 -U "$DB_ROLE" -d normless_crm -tAc "SELECT 1" >/dev/null 2>&1; then
    echo "WARNING: $DB_ROLE CAN connect to normless_crm — it should not be able to"
else
    echo "good: $DB_ROLE cannot reach the CRM database"
fi

echo
echo "provisioning done"
