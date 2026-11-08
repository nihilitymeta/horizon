#!/bin/sh
# The launcher inside its Flatpak. It runs on the player's own folders — the data (versions, worlds, accounts), the
# config (the data location, the selected source) and the data folder whose menus and icons it keeps up to date — so it
# is the profile a distribution build uses rather than the sandbox's empty copy. It also points at the JRE the bundle
# carries. The launcher is at /app, which it reads as package-managed: it never replaces itself, and a newer bundle is
# installed over it.
export HORIZON_LAUNCHER_HOME="${HORIZON_LAUNCHER_HOME:-$HOME/.local/share/horizon-launcher}"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
export JAVA_HOME=/app/jre
exec /app/lib/horizon-launcher/horizon-launcher "$@"
