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
#   tools/deploy_local_mac.sh --launcher [--version X.Y.Z]
#
# --launcher builds Event Horizon's launcher (launcher/, release) and installs
# it as ~/Applications/Horizon.app, signed ad hoc, swapping the bundle in one
# rename so a running launcher keeps its files. Its version is launcher/
# Cargo.toml's; --version overrides it. A launcher built here never updates
# itself (game_sources.rs). Microsoft sign-in needs HORIZON_MSA_CLIENT_ID in the
# environment (docs/event-horizon-sign-in.md); without it the build says so.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BUILD=true
SELECT=true
PROFILE=fast
LAUNCHER=false
VERSION=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) BUILD=false; shift ;;
        --no-select) SELECT=false; shift ;;
        --profile) PROFILE=$2; shift 2 ;;
        --launcher) LAUNCHER=true; shift ;;
        --version) VERSION=$2; shift 2 ;;
        -h|--help)
            sed -n '3,12p' "$0"
            exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done

[[ "$(uname -s)" == Darwin ]] || { echo "macOS only (see tools/deploy_test_launcher.sh)" >&2; exit 1; }
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

if [[ "$LAUNCHER" == true ]]; then
    if [[ -z "$VERSION" ]]; then
        VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/launcher/Cargo.toml" | head -1)"
    fi
    [[ -n "$VERSION" ]] || { echo "Error: no version in launcher/Cargo.toml; pass --version" >&2; exit 1; }
    [[ -n "${HORIZON_MSA_CLIENT_ID:-}" ]] || echo "Note: HORIZON_MSA_CLIENT_ID is not set: this launcher can't sign in (docs/event-horizon-sign-in.md)." >&2
    APP="$HOME/Applications/Horizon.app"
    STAGING="$APP.staging"
    echo "==> Building the launcher as $VERSION (release)..."
    (cd "$ROOT/launcher" && HORIZON_LAUNCHER_VERSION="$VERSION" cargo build --release)
    BIN="$ROOT/launcher/target/release/horizon-launcher"
    echo "==> Staging $STAGING..."
    rm -rf "$STAGING"
    mkdir -p "$STAGING/Contents/MacOS" "$STAGING/Contents/Resources"
    cp "$BIN" "$STAGING/Contents/MacOS/horizon-launcher"
    chmod 755 "$STAGING/Contents/MacOS/horizon-launcher"
    cp "$ROOT/packaging/icons/horizon.icns" "$STAGING/Contents/Resources/horizon.icns"
    sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/macos/Info.plist" > "$STAGING/Contents/Info.plist"
    printf 'APPL????' > "$STAGING/Contents/PkgInfo"
    plutil -lint "$STAGING/Contents/Info.plist" >/dev/null
    codesign --force --sign - "$STAGING"
    codesign --verify --strict "$STAGING"
    "$STAGING/Contents/MacOS/horizon-launcher" --version
    # One rename each way: a launcher running from the old bundle keeps its files.
    [[ -e "$APP" ]] && mv "$APP" "$APP.old"
    mv "$STAGING" "$APP"
    rm -rf "$APP.old"
    echo "==> Installed $APP. Restart the launcher if it is open."
    exit 0
fi

if [[ "$BUILD" == true ]]; then
    echo "==> Building horizon-client, horizon-ctl, horizon-prepare and horizon-server ($PROFILE)..."
    # horizon-ctl is horizon-client-lib's binary.
    cargo build --profile "$PROFILE" -p event-horizon -p horizon-client-lib --bins -p horizon-prepare -p horizon-server
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

# The newest prepared version lends its resources (an APFS clone costs no
# space) and, where this tree has none, its exporters bundle.
REF=""
for candidate in "$VERSIONS"/*/; do
    candidate="${candidate%/}"
    [[ "$candidate" == "$TARGET" ]] && continue
    [[ -d "$candidate/.local/horizon-client" ]] || continue
    if [[ -z "$REF" || "$candidate" -nt "$REF" ]]; then
        REF="$candidate"
    fi
done

# xtask package wants the Java exporters' bundle in exporters/ (gitignored):
# compiled here when this tree holds the imported game files, else borrowed.
if [[ ! -f "$ROOT/exporters/manifest.json" ]]; then
    if [[ -f "$ROOT/.local/minecraft/26.3/manifest.json" ]]; then
        echo "==> Compiling the exporters..."
        "$ROOT/target/$PROFILE/horizon-prepare" compile-exporters --root "$ROOT"
    elif [[ -n "$REF" && -f "$REF/exporters/manifest.json" ]]; then
        echo "==> Borrowing the exporters bundle from $(basename "$REF")..."
        rm -rf "$ROOT/exporters"
        cp -c -R "$REF/exporters" "$ROOT/exporters"
    else
        echo "Error: no exporters/manifest.json; run horizon-prepare --prepare, then compile-exporters" >&2
        exit 1
    fi
fi

echo "==> Packaging $TAG..."
# xtask package replaces the folder and copies the three tools from beside the binary.
cargo run -q -p xtask -- package "$BIN" "$TARGET"
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
