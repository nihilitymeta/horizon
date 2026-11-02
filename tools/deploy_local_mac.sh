#!/usr/bin/env bash
set -euo pipefail

# Installs the working tree's game as a local build of the Horizon launcher on
# macOS: `versions/local-<date>-<commit>` in the launcher's data folder, which
# the launcher lists as "Local Build" (`.local_build`) and starts from its own
# Horizon.app (game_app.rs). The prepared resources (`.local/`) are cloned from
# the newest prepared version so Play works at once; nothing in the launcher's
# folder is touched otherwise. Never run against a Flatpak or Linux home:
# tools/deploy_test_launcher.sh is that script.
#
#   tools/deploy_local_mac.sh [--no-build] [--no-select] [--profile fast|release]

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BUILD=true
SELECT=true
PROFILE=fast
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) BUILD=false; shift ;;
        --no-select) SELECT=false; shift ;;
        --profile) PROFILE=$2; shift 2 ;;
        -h|--help)
            sed -n '3,12p' "$0"
            exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

[[ "$(uname -s)" == Darwin ]] || { echo "macOS only (see tools/deploy_test_launcher.sh)" >&2; exit 1; }
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

if [[ "$BUILD" == true ]]; then
    echo "==> Building horizon-client, horizon-ctl, horizon-prepare and horizon-server ($PROFILE)..."
    # horizon-ctl is horizon-client-lib's binary.
    cargo build --profile "$PROFILE" -p horizon-client --bins -p horizon-prepare -p horizon-server --bin horizon-ctl
fi
BIN="$ROOT/target/$PROFILE/horizon-client"
[[ -f "$BIN" ]] || { echo "Error: $BIN not found" >&2; exit 1; }

COMMIT="$(git rev-parse --short HEAD)"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
DATE="$(date +%Y-%m-%d)"
TAG="local-${DATE}-${COMMIT}"

LAUNCHER_HOME="${HORIZON_LAUNCHER_HOME:-$HOME/Library/Application Support/horizon-launcher}"
VERSIONS="$LAUNCHER_HOME/versions"
TARGET="$VERSIONS/$TAG"
[[ -d "$VERSIONS" ]] || { echo "Error: no launcher data at $VERSIONS (open the launcher once)" >&2; exit 1; }

echo "==> Packaging $TAG..."
# xtask package replaces the folder and copies the three tools from beside the binary.
cargo run -q -p xtask -- package "$BIN" "$TARGET"

# The newest prepared version lends its resources: an APFS clone costs no space.
REF=""
for candidate in $(ls -1dt "$VERSIONS"/*/ 2>/dev/null); do
    candidate="${candidate%/}"
    [[ "$candidate" == "$TARGET" ]] && continue
    if [[ -d "$candidate/.local/horizon-client" ]]; then
        REF="$candidate"
        break
    fi
done
if [[ -n "$REF" ]]; then
    echo "==> Cloning prepared resources from $(basename "$REF")..."
    cp -c -R "$REF/.local" "$TARGET/.local"
else
    echo "==> No prepared version found: run  horizon-launcher prepare --version $TAG"
fi

touch "$TARGET/.installed"
cat > "$TARGET/.local_build" <<EOF
{
  "commit": "$COMMIT",
  "branch": "$BRANCH",
  "date": "$DATE",
  "type": "local"
}
EOF

SETTINGS="$LAUNCHER_HOME/settings.json"
if [[ "$SELECT" == true && -f "$SETTINGS" ]]; then
    echo "==> Selecting $TAG in the launcher..."
    jq --arg tag "$TAG" '.selected = $tag' "$SETTINGS" > "$SETTINGS.tmp"
    mv "$SETTINGS.tmp" "$SETTINGS"
fi

echo "==> Installed $TAG ($BRANCH). Restart the launcher if it is open."
