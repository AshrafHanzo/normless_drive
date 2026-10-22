#!/bin/bash
# READ-ONLY post-deploy verification.
set -uo pipefail

Q() { sudo -u postgres psql -d normless_drive -tAc "$1" 2>/dev/null; }

echo "=== tables ==="
Q "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename;"

echo
echo "=== phone hints (should look like ...506) ==="
Q "SELECT COALESCE(phone_hint,'(none)') FROM tg_sessions;"

echo
echo "=== accounts and their permissions ==="
Q "SELECT u.email || ' | ' || u.role || '/' || u.status
        || ' | upload=' || p.can_upload
        || ' share='   || p.can_share
        || ' quota='   || p.storage_quota_bytes
     FROM users u LEFT JOIN user_permissions p ON p.user_id = u.id
    ORDER BY u.created_at;"

echo
echo "=== folders ==="
Q "SELECT count(*) || ' folder(s)' FROM folders;"

echo
echo "=== uploads recorded ==="
Q "SELECT count(*) || ' upload(s), ' || COALESCE(sum(size_bytes),0) || ' bytes' FROM uploads;"

echo
echo "=== API reachability ==="
for path in /health /api/auth/me /api/folders; do
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 "https://drive.normless.in$path")
    printf '  %-18s HTTP %s\n' "$path" "$code"
done

echo
echo "=== the CRM, which must be untouched ==="
printf '  ops.normless.store  HTTP %s\n' \
  "$(curl -s -o /dev/null -w '%{http_code}' --max-time 8 https://ops.normless.store/)"
printf '  pm2                 %s\n' "$(pm2 jlist 2>/dev/null | grep -o '"status":"[a-z]*"' | head -1)"
