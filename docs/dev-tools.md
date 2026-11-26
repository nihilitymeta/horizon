# Developer tools

The developer tools are a set of in-game windows and chat commands for people who
make mods or debug the game. They draw over the world, show an entity's data, its
goals, senses and path, chart a number over time, show a block's facts, and run Lua
in a console. They are off unless you turn them on, and their commands are for the
world's owner only.

Scripts that run the Lua console are described in [Lua mods](lua-modding.md).

## Turning the tools on

The tools start only when one of these is set:

- the environment variable `HORIZON_DEV_TOOLS=1`, or
- the launch argument `--dev-tools`.

Nothing is registered or loaded otherwise (`crates/event-horizon-dev-tools/src/lib.rs`,
`ENV`, `FLAG`, `requested`).

The tools are a built-in server-side mod. Because every hook is server-side, a
client without the tools can still join a host that runs them (`lib.rs`, `BUILT`).

## The window and the key

Pressing **F8** (the `key.devTools` action) frees the pointer. While it is free, the
mouse and keyboard go to the tools' window, not to the game. Press F8 again to give
them back (`crates/event-horizon-dev-tools/src/input.rs`;
`crates/horizon-client-input/src/actions.rs`, `DevTools`, default `F8`).

`key.devTools` is listed on the Key Binds screen only while the tools are on
(`lib.rs`, `mods`, calling `show_dev_tools_key(true)`).

The window's zoom follows the window's height. Set `HORIZON_DEV_TOOLS_SCALE` to
override it (`input.rs`, `SCALE_ENV`).

## Who may use the commands

Every command is for the world's owner, and only the owner. The sender's uuid must be
the single-player owner's. Any other player, a guest of any level, or an operator
gets the reply `horizon.dev_tools.owner_only` and nothing runs
(`crates/event-horizon-dev-tools/src/commands.rs`, `serve`, `is_owner`).

On a dedicated server there is no owner, so no one is served: the commands refuse
everyone and the Lua console does not run.

## Commands

The commands are registered in the server's command tree. Each usage is listed here
as the game registers it (`commands.rs`, `USAGES`).

| Command | What it does |
|---|---|
| `/inspect` | Lists the entities being watched and how to use the tools. |
| `/inspect <targets>` | Opens the data window for the entities: their data as an NBT tree, with a filter and a watch button for each number. |
| `/inspect clear` | Closes the data windows. |
| `/goals <targets>` | Opens the goals window: the mob's goal selectors and their history. `/goals clear` closes it. |
| `/senses <targets>` | Opens the senses window: the mob's target, sight and nearby players. `/senses clear` closes it. |
| `/nav <targets>` | Opens the navigation window: the mob's path, also drawn in the world. `/nav clear` closes it. |
| `/watch <targets> <path>` | Charts a numeric NBT path of the entities over time. |
| `/watch clear` | Removes every watch chart. |
| `/mobai log <targets>` | Writes each tick's AI snapshot of the mobs as a JSON line, under `dev-tools/logs`. |
| `/mobai log stop` | Closes those log files. |
| `/blockinfo` | Lists the watched blocks. |
| `/blockinfo <x> <y> <z>` | Opens a block window for that block in the owner's dimension. Coordinates may be absolute, negative, or relative with `~`. |
| `/blockinfo clear` | Closes every block window. |
| `/lua` | Prints the usage line. It does not open the console. |
| `/lua <code>` | Runs a Lua chunk in the console's sandbox and prints its output: the last 20 printed lines, its returned value, or its error. |
| `/console` | Opens the Lua console window. |
| `/console close` | Closes the console window. |
| `/console clear` | Clears the console's output. |
| `/luareload` | Lists the dev-path Lua mods that can be reloaded. |
| `/luareload <id>` | Reloads that mod's scripts from its folder. |

The `targets` argument is a selector that names the entities (`commands.rs`,
`selector`). The commands that take `<rest>` read the rest of the line.

Limits of the commands (`commands.rs`):

- `/lua` prints at most 20 lines, each cut to 256 characters.
- A `/lua` run stops after 500 ms (`event-horizon-lua/src/console.rs`, `CHUNK_BUDGET`).

## The Lua console

The console window is one shared Lua state, separate from every mod's state. It is
not a mod. You type a chunk in the window or with `/lua <code>`:

- The window keeps at most 500 output lines and 100 history entries
  (`crates/event-horizon-dev-tools/src/console.rs`, `MAX_LINES`, `MAX_HISTORY`).
- Each server tick runs at most 4 queued chunks. The rest wait for the next tick
  (`console.rs`, `PER_TICK`).
- A chunk runs only while the world has an owner.
- In the console, `mc.cmd(line)` runs a server command as the server console, at
  level 4. The `/lua` command checks the owner before it runs a chunk
  (`crates/event-horizon-lua/src/console.rs`, module notes).

## Reloading a dev mod

`/luareload <id>` re-reads a dev-path mod's folder and declares its scripts again in
a fresh state. It swaps that state in only when the new declarations equal the old
ones. Records that are fixed at start-up (hooks, blocks, items, parts, commands,
goals, settings schema and so on) cannot change while the game runs. A change to one
of them is refused with the names of what changed, and the old code keeps running.
Timers set with `horizon.after` do not survive a reload. Installed `.hlua` archives
are not reloaded (`crates/event-horizon-lua/src/reload.rs`).

## Known limits

- A dedicated server has no owner, so it has no Lua console and no tool command runs
  on it. The data the windows read is filled, but nobody reads it there.
- The Lua console runs only while the world has an owner.
