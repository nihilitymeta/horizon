#!/bin/sh
# The launcher inside its Flatpak. It uses the player's own data folder, so the versions, worlds and settings are the
# ones a distribution build sees rather than the sandbox's copy, and the JRE the bundle carries is the one it offers
# as an installed runtime (the sandbox has no distribution's java). The launcher is at /app, which it reads as
# package-managed: it never replaces itself, and `flatpak update` updates it.
export HORIZON_LAUNCHER_HOME="${HORIZON_LAUNCHER_HOME:-$HOME/.local/share/horizon-launcher}"
export JAVA_HOME=/app/jre
exec /app/lib/horizon-launcher/horizon-launcher "$@"
