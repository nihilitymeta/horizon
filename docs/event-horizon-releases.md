# Event Horizon Releases

Event Horizon publishes its builds as GitHub releases of `whos-carmen/horizon`, for Windows x64. A
mod developer installs the launcher from a release once; after that, the launcher installs each new release by itself.
Nobody compiles the engine to play it.

## What a release holds

The archive, `event-horizon-windows-x64.zip`, holds:

| Program | What it is |
|---|---|
| `horizon-launcher` | Event Horizon's launcher |
| `horizon-client` | the game |
| `horizon-ctl` | drives a running game, for the modkit's in-game checks |
| `cargo-horizon-mod` | makes, checks and packs mods |

`SHA256SUMS` lists its checksum. Another platform is another row in the workflow's matrix: Linux x86_64 builds
on GitHub's runners as it is, with the system libraries the workflow installs for it.

A release holds only programs. The game's data files (the exporters and the data they prepare) come from one of
Horizon's own releases: they are built with Minecraft's files, which can't be on GitHub's runners.

## Publish a release

1. Check `engine-work` as for any push ([fork-workflow.md](fork-workflow.md#5-check-it)).
2. Tag it and push the tag:

   ```sh
   git tag eh-v0.1.0 engine-work
   git push origin eh-v0.1.0
   ```

The `event horizon release` workflow builds the release and publishes it. It takes a while: it compiles the whole
workspace. Its progress is on the repository's Actions tab.

Version tags start with `eh-v`, so they never mix with upstream's `v…` tags. You decide when to release; nothing is
released on its own.

## What a developer does

1. Download `event-horizon-windows-x64.zip` from the newest release, unpack it, and start `horizon-launcher.exe`.
   Windows may warn that the program is unrecognized, since it isn't code-signed: choose More info, then Run anyway.
2. Sign in with a Microsoft account that owns Minecraft: Java Edition.
3. Install one Horizon version, for its game files: Versions, the newest Nightly, Install.

The launcher's Versions page lists Event Horizon's builds in a section of their own, above Horizon's: a release as
**Event Horizon 0.1.0**, a build you made from the checkout as **Event Horizon · local <commit>**. Its **GitHub
sources** card has Event Horizon's repository on it, set to **Update automatically**: at each update check, the
launcher installs a new release on the Horizon version's game files, and plays it. To install the newest release at
once, press **Update** on the card. The Home page's Play button says **Update** only when the source that installed the
build has a newer release: a build you made yourself, and Horizon's newer Nightlies, are never updates of one.

The **Horizon Modkit** card, at the top of the Mods page, installs the modkit that Event Horizon's mods need: its
newest `horizon_modkit.hmod` goes into every instance that plays an Event Horizon build, turned on, so a mod that
depends on it runs with no `--dev-mod`. The card shows the version installed and the newest release, and has its own
**Update automatically** switch. The modkit publishes a release when its own `v…` tag is pushed (the modkit's
`.github/workflows/release.yml`). From the command line: `horizon-launcher modkit`, `modkit update`,
`modkit auto on|off`, and `modkit install <file>` for a `horizon_modkit.hmod` you built yourself.

Two switches are off by default in Event Horizon's launcher:

- **Install new versions automatically** (Versions page): leave it off. On, it would install and select Horizon's own
  new releases over Event Horizon's.
- **Update the launcher automatically** (Settings): the launcher updates itself from Event Horizon's releases, never
  Horizon's, when a newer `eh-v` release is out. Turn it on to have that happen by itself; off, the Settings page offers
  the Update button. A launcher you built yourself never updates itself.

The command line does the same as the card: `horizon-launcher sources list`, `sources update`,
`sources auto <owner>/<repo> on|off`, `sources add <owner>/<repo>`.

## Telemetry

Event Horizon's game and launcher send nothing to Horizon's servers: no telemetry and no "I'm playing" ping, so
Horizon's counts stay Horizon's. To send to a server of your own, set `HORIZON_TELEMETRY_URL` to its address.

## Microsoft sign-in

The launcher starts the game only for a signed-in account that owns the game, and signing in needs Event Horizon's
own app registration. [Setting up Microsoft sign-in](event-horizon-sign-in.md) walks through it. Until it is done, a
release's launcher can't sign in, and the release workflow warns about it.
