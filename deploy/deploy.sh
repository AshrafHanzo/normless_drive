#!/bin/bash
# Builds and installs Normless Drive. Safe to re-run — this is also the
# update path.
#
# Only ever writes inside /opt/normless-drive. Touches no other service,
# no other database, and no existing nginx site.

set -euo pipefail

APP_DIR=/opt/normless-drive
SRC_DIR="$APP_DIR/src"
REPO=https://github.com/AshrafHanzo/normless_drive.git
APP_USER=normlessdrive

export PATH="/root/.cargo/bin:$PATH"

echo "=== toolchain ==="
rustc --version
node --version
# Vite 7 needs Node 20.19+ or 22.12+. Building with an older one fails deep
# inside rollup with an error that does not mention the version at all.
node -e 'const [maj,min]=process.versions.node.split(".").map(Number);
         if (maj<20 || (maj===20 && min<19)) { console.error("Node 20.19+ required"); process.exit(1); }
         console.log("node version OK");'

echo
echo "=== source ==="
if [ -d "$SRC_DIR/.git" ]; then
    git -C "$SRC_DIR" fetch --quiet origin
    git -C "$SRC_DIR" reset --hard --quiet origin/main
    echo "updated to $(git -C "$SRC_DIR" rev-parse --short HEAD)"
else
    git clone --quiet "$REPO" "$SRC_DIR"
    echo "cloned at $(git -C "$SRC_DIR" rev-parse --short HEAD)"
fi

echo
echo "=== building the server (release) ==="
cd "$SRC_DIR/server"
cargo build --release --quiet
echo "built $(ls -lh target/release/normless-drive | awk '{print $5}')"

echo
echo "=== building the web app ==="
cd "$SRC_DIR/web"
npm ci --no-audit --no-fund --silent 2>&1 | tail -3 || npm install --no-audit --no-fund --silent 2>&1 | tail -3
npm run build 2>&1 | tail -5

echo
echo "=== installing ==="
install -d -o "$APP_USER" -g "$APP_USER" "$APP_DIR/bin" "$APP_DIR/web"

# Install to a temp name then rename: a rename is atomic, so a running
# process is never reading a half-copied binary.
install -m 0755 -o "$APP_USER" -g "$APP_USER" \
    "$SRC_DIR/server/target/release/normless-drive" "$APP_DIR/bin/.normless-drive.new"
mv -f "$APP_DIR/bin/.normless-drive.new" "$APP_DIR/bin/normless-drive"

rm -rf "$APP_DIR/web/dist.old"
[ -d "$APP_DIR/web/dist" ] && mv "$APP_DIR/web/dist" "$APP_DIR/web/dist.old"
cp -r "$SRC_DIR/web/dist" "$APP_DIR/web/dist"
chown -R "$APP_USER:$APP_USER" "$APP_DIR/web"
rm -rf "$APP_DIR/web/dist.old"

echo "installed:"
ls -lh "$APP_DIR/bin/normless-drive"
echo "  web assets: $(find "$APP_DIR/web/dist" -type f | wc -l) files"

echo
echo "=== done ==="
echo "next: ensure $APP_DIR/.env exists, then: systemctl restart normless-drive"
