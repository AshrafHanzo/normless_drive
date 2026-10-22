#!/bin/bash
# Stops nginx proxying /s/ to the application.
#
# A share link is a page a person opens, so /s/{token} must reach the
# single-page app. Its data comes from /api/s/ instead. While /s/ was
# proxied, opening a link downloaded JSON instead of showing a page.
#
# Edits the live file in place rather than replacing it: certbot has
# already rewritten that file to add the TLS server block, and overwriting
# it would drop the certificate configuration.

set -euo pipefail

FILE=/etc/nginx/sites-available/drive.normless.in
BACKUP="$FILE.bak.$(date +%s)"

cp "$FILE" "$BACKUP"
echo "backed up to $BACKUP"

echo "--- before ---"
grep -n 'location ~' "$FILE" || true

# Match the alternation group loosely so this works whatever order the
# alternatives are in.
sed -i -E 's#location ~ \^/\(api\|health\|s\|d\)#location ~ ^/(api|health)#' "$FILE"

echo "--- after ---"
grep -n 'location ~' "$FILE" || true

if nginx -t 2>&1 | tail -1 | grep -q successful; then
    systemctl reload nginx
    echo "nginx reloaded"
else
    echo "CONFIG INVALID — restoring the backup and leaving nginx alone"
    cp "$BACKUP" "$FILE"
    nginx -t
    exit 1
fi
