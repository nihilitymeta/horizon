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

The launcher's Versions page has a **GitHub sources** card with Event Horizon's repository on it, set to
**Update automatically**. At each update check, the launcher installs a new release there on the Horizon version's game
files, and plays it. To install the newest release at once, press **Update**.

Two switches are off in Event Horizon's launcher, so nothing upstream replaces the developer's build:

- **Install new versions automatically** (Versions page): on, it would install and select Horizon's own new releases.
- **Update the launcher automatically** (Settings): on, Horizon's launcher would replace this one.

The command line does the same as the card: `horizon-launcher sources list`, `sources update`,
`sources auto <owner>/<repo> on|off`, `sources add <owner>/<repo>`.

## Telemetry

Event Horizon's game and launcher send nothing to Horizon's servers: no telemetry and no "I'm playing" ping, so
Horizon's counts stay Horizon's. To send to a server of your own, set `HORIZON_TELEMETRY_URL` to its address.

## Microsoft sign-in

The launcher starts the game only for a signed-in account that owns the game. Sign-in needs an Azure app of Event
Horizon's own: never another application's client ID, Horizon's included. You register it once:

1. Register an app in Azure, with personal Microsoft accounts only and public client flows allowed. The steps are in
   [microsoft-account.md](microsoft-account.md#what-ludovic-registers-needs-his-accounts); use your own account and
   the name Event Horizon.
2. Ask Mojang to approve the app for Minecraft, with the form that page links. Until Mojang approves it, sign-in stops
   at the Minecraft step. Approval takes days to weeks.
3. Give the release builds the app's client ID (a public value, not a secret):

   ```sh
   gh variable set HORIZON_MSA_CLIENT_ID -R whos-carmen/horizon --body <client id>
   ```

4. For a launcher you build yourself, set it when you build or when you run it:

   ```sh
   export HORIZON_MSA_CLIENT_ID=<client id>
   ```

A launcher built without it says "This build of the launcher has no Microsoft sign-in", and the release workflow
warns about it.
