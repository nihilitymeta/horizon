# Building `engine-work`

What a machine needs, on any platform (upstream's own list plus what the integrated branches add):

| Need | Why | Windows | macOS | Linux |
|---|---|---|---|---|
| Rust 1.90 or newer, from [rustup](https://rustup.rs) | the game, the launcher and the tools are Rust | the default `x86_64-pc-windows-msvc` toolchain | Homebrew `rustup` or rustup.rs | rustup.rs |
| A C/C++ compiler | `lua-bindings` builds Luau from source (`mlua`, vendored); a few C crates | **Visual Studio 2022 Build Tools** with the "Desktop development with C++" workload (MSVC and the Windows SDK; the Rust MSVC toolchain's linker needs it too) | Xcode command line tools (`xcode-select --install`) | `gcc`/`clang` |
| Python 3.10 or newer | build and research scripts | python.org or `winget install Python.Python.3.12` | Homebrew | distro |
| Prepared resources | the game's data and assets, from your own copy of Java Edition 26.3 | see below | see below | see below |
| Linux only: `libasound2-dev libudev-dev` | audio and input | — | — | apt |

No CMake, no Vulkan SDK, no Java at build time. Git, of course.

**Prepared resources.** The game runs on data `horizon-prepare` extracts from Minecraft (never downloaded by
Horizon). The easy way is to install the [Horizon launcher](https://playhorizon.dev), sign in and press Play once:
it prepares everything, and a locally built game then points `--resources` at
`<launcher data>\versions\<version>\.local\horizon-client\26.3` (Windows: `%APPDATA%\horizon-launcher`; macOS:
`~/Library/Application Support/horizon-launcher`; Linux: `~/.local/share/horizon-launcher`). The hard way needs
**JDK 25** and the 26.3 server jar: `cargo run --release -p horizon-prepare -- --prepare --server-jar server.jar`.

**Build and run** (a cold `fast` build is long: [docs/build-times.md](docs/build-times.md) has Windows and Linux
timings; later builds are incremental):

```sh
git clone https://github.com/whos-carmen/horizon.git && cd horizon
git checkout engine-work
cargo build --profile fast -p event-horizon -p horizon-client-lib
target/fast/horizon-client --game-dir <a folder> --resources <the 26.3 folder above> --world "New World" --dev-tools \
  --dev-mod <path to horizon-modkit> --dev-mod <path to horizon-parasite-mod>
```

The GPU needs Vulkan or DirectX 12 on Windows, Metal on a Mac, Vulkan or OpenGL 3.3 on Linux. The HDR code is
Vulkan-only and inert elsewhere; nothing in `engine-work` needs a particular GPU to build.
