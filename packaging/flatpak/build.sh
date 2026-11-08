#!/usr/bin/env bash
# Builds the Linux launcher as one Flatpak bundle (`horizon-launcher.flatpak`): the release workflow's Linux run uses
# it, and a developer can run it in the checkout.
#
#   packaging/flatpak/build.sh OUTDIR
#
# Needs flatpak and flatpak-builder on the machine. The runtime, SDK and extensions it names are fetched from Flathub
# into the user's Flatpak installation when missing (about 1.5 GB the first time); the bundle records Flathub as its
# runtime repository, so `flatpak install horizon-launcher.flatpak` gets the runtime from there.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
out=$(mkdir -p "${1:?OUTDIR}" && cd "$1" && pwd)

case "$(uname -m)" in
x86_64 | aarch64) ;;
*)
  echo "packaging/flatpak/build.sh: no launcher build for $(uname -m)" >&2
  exit 1
  ;;
esac

# The installation flatpak-builder uses: the user's, so no root is needed; root's own build (a container without
# unprivileged namespaces, where only root can make them) uses the system's.
if [ "$(id -u)" = 0 ]; then
  install=--system
else
  install=--user
fi
if ! flatpak remotes "$install" --columns=name | grep -qx flathub; then
  flatpak remote-add "$install" --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The source tree as the build sees it: the checkout without the build folders, the Git metadata or the prepared
# vanilla data, which the sandbox has no use for and would only make the archive large.
tar -C "$repo" -czf "$work/horizon-source.tar.gz" \
  --exclude=./.git --exclude=./target --exclude=./.local --exclude=./launcher/target \
  --exclude=./.flatpak-builder .
# The release this bundle is built for, when one knows it (`HORIZON_LAUNCHER_TAG` and its version, as the release
# workflow has them): the launcher then names itself `Event Horizon <version>` instead of a build by hand.
manifest="$work/manifest.yml"
sed -e "s|@LAUNCHER_TAG@|${HORIZON_LAUNCHER_TAG:-}|" \
  -e "s|@LAUNCHER_VERSION@|${HORIZON_LAUNCHER_VERSION:-}|" \
  -e "s|@LAUNCHER_BUILT@|${HORIZON_LAUNCHER_BUILT:-}|" \
  "$here/dev.playhorizon.HorizonLauncher.yml" >"$manifest"
# The Azure app the launcher signs in with (`HORIZON_MSA_CLIENT_ID`, the release workflow's variable): a bundle built
# without it can list an account but cannot start the game, so a missing one is said out loud.
if [ -n "${HORIZON_MSA_CLIENT_ID:-}" ]; then
  sed -i "s|@MSA_CLIENT_ID@|$HORIZON_MSA_CLIENT_ID|" "$manifest"
else
  echo "packaging/flatpak/build.sh: HORIZON_MSA_CLIENT_ID is not set: this launcher cannot sign in to Microsoft" >&2
  sed -i '/@MSA_CLIENT_ID@/d' "$manifest"
fi

# `--install-deps-from`: the runtime, SDK and the two extensions, into this user's installation, when missing.
flatpak-builder "$install" --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
  --state-dir "$work/state" --repo "$out/repo" "$work/build" "$manifest" >"$out/flatpak-builder.log" 2>&1 ||
  {
    echo "packaging/flatpak/build.sh: flatpak-builder failed; its log:" >&2
    tail -40 "$out/flatpak-builder.log" >&2
    exit 1
  }
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$out/repo" "$out/horizon-launcher.flatpak" dev.playhorizon.HorizonLauncher master
rm -rf "$out/repo"
echo "packaging/flatpak/build.sh: $out/horizon-launcher.flatpak ($(du -h "$out/horizon-launcher.flatpak" | cut -f1))"
