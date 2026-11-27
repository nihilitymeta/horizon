# Event Horizon releases

Event Horizon's builds are published as GitHub releases of this repository, for Windows x64 and Linux x86_64. They are
built by the `event horizon release` workflow (`.github/workflows/event-horizon-release.yml`) on GitHub-hosted runners.
Nothing is built on a schedule: a release exists only when a version tag is pushed or the workflow is started by hand.

## What a release holds

Each platform has one archive, `event-horizon-windows-x64.zip` or `event-horizon-linux-x86_64.tar.gz`. It holds:

| Program | What it is |
|---|---|
| `horizon-launcher` | the launcher |
| `horizon-client` | the game (built from the `event-horizon` package) |
| `horizon-ctl` | drives a running game, for in-game checks |
| `cargo-horizon-mod` | makes, checks and packs mods |
| `THIRD_PARTY_NOTICES.md` | licences of the bundled libraries |
| `horizon-native-mods/event_horizon_lua/` | the Lua host's package, which the game loads from beside its executable |
| `horizon-native-mods/event_horizon_dev_tools/` | the developer tools' package; the game needs it to start with `--dev-tools` |

The release also has `SHA256SUMS`, with the SHA-256 of each archive. The launcher checks a downloaded archive against
the SHA-256 that GitHub lists for it, and refuses an archive that has none or whose digest does not match.

A release holds only programs. Game data (the exporters and the data they prepare) is built from Minecraft's own files,
which cannot be built on GitHub's runners, so it comes from a separate source.

## Cut a release

The version is the tag. Its form is `eh-v` followed by the version, for example `eh-v0.1.0`.

- **Push a tag** on the commit to release:

  ```sh
  git tag eh-v0.1.0 <commit>
  git push origin eh-v0.1.0
  ```

- **Run it by hand**: on the Actions tab, open `event horizon release`, choose Run workflow, and enter the tag in the
  `tag` field. The tag must start with `eh-v`; the run refuses anything else.

The release is created at the commit the run builds, under the tag given. A build takes a while, since it compiles the
whole workspace; its progress is on the run's page.

Nothing is scheduled. The workflow has no cron trigger, so no release appears unless one of the two above starts it.

## Platforms

Linux archives are built on `ubuntu-24.04`, not `ubuntu-latest`: the programs link the runner's glibc, so they run only
on systems at least as new as that one. To add a platform, add a row to the build matrix; a Linux row needs the apt
packages the workflow installs.

## Microsoft sign-in

The launcher needs an app registration for Microsoft sign-in. It is read from the repository variable
`HORIZON_MSA_CLIENT_ID` when the launcher is built. If the variable is unset, the build still succeeds, the workflow
prints a warning, and the launcher can't sign in.
