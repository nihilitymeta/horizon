//! Lua scripts as mods (docs/lua-bindings.md). A content mod that carries
//! `scripts/init.lua` beside its `data/` and `assets/` is also a code mod: the
//! game runs the script in a Luau state of its own, and the script registers
//! functions for the hook points (`horizon.on("server_tick", fn)`), the same
//! events trait mods and mixin bundles hear. Nothing is compiled and nothing
//! patches the game, so two scripted mods never conflict in the build.
//!
//! The runtime is a [`Hooks`] implementation like a trait mod's, so the
//! generated dispatch calls it in order with the other mods. A script cannot
//! hold the host: a hook's `horizon.*` calls that act on the game are queued
//! and run by the runtime once the handler returns, in order, with the host
//! the hook was given.
//!
//! Today scripts load from `--dev-mod` folders only. Installed `.hmod`s need
//! the packer and the manifest to accept a `scripts/` entry (the plan in the
//! document).
use crate::mod_api::{
    Hooks, Host, MobSpawnedEvent, Mods, ServerTickEvent, ServerTickPhase, WorldLifecycleEvent,
};
use horizon_mods::{Capability, Content, Package};
use mlua::{Function, Lua, MultiValue, Table, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The file a scripted mod starts from, inside its folder.
pub const ENTRY: &str = horizon_mods::content::SCRIPT_ENTRY;

/// The content mod's manifest, which names the mod.
const MANIFEST: &str = "horizon-mod.toml";
/// The Lua global the game provides.
const GLOBAL: &str = "horizon";
/// The registry key of the handlers table: `name -> { fn, fn, ... }`.
const HANDLERS: &str = "horizon_handlers";
/// The registry key of the queue of actions a handler asked for.
const QUEUE: &str = "horizon_queue";

/// One scripted mod: its folder and its Lua state.
pub struct LuaMod {
    id: String,
    /// The `.hmod` or folder the scripts came from.
    source: PathBuf,
    lua: Lua,
}

impl std::fmt::Debug for LuaMod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaMod")
            .field("id", &self.id)
            .field("source", &self.source)
            .finish()
    }
}

/// The scripts of a mod, by path (`scripts/init.lua`), read once from its
/// `.hmod` or folder.
type Scripts = Arc<HashMap<String, String>>;

/// The scripted mods the game runs, in order, as the trait mod list
/// `entry::main_with` takes: the installed mods in `horizon-mods/` that are
/// on (`horizon-mods.txt`, as the content loader reads it) and the `--dev-mod`
/// folders of the command line, when they have capability `scripts`. One
/// whose scripts fail to load is left out with the error in the log.
pub fn discover(args: impl Iterator<Item = String>) -> Mods {
    let mut dev: Vec<PathBuf> = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let folder = match arg.strip_prefix("--dev-mod=") {
            Some(path) => path.to_owned(),
            None if arg == "--dev-mod" => match args.next() {
                Some(path) => path,
                None => break,
            },
            None => continue,
        };
        let dir = PathBuf::from(folder);
        dev.push(std::path::absolute(&dir).unwrap_or(dir));
    }
    let here = std::env::current_dir().unwrap_or_default();
    let enabled = crate::mod_list::ModList::load(&crate::logging::shared_dir(&here)).enabled;
    let mut paths = horizon_mods::find(&here.join(crate::mod_list::DIR));
    paths.retain(|p| !dev.contains(p));
    paths.extend(dev.iter().cloned());
    let mut mods: Vec<Box<dyn Hooks>> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for path in &paths {
        let Ok(package) = Package::read(path) else {
            continue;
        };
        let m = &package.manifest;
        if !m.capabilities.contains(&Capability::Scripts) {
            if dev.contains(path) && path.join(ENTRY).is_file() {
                log::warn!(
                    "Horizon: {} has {ENTRY} but no capability scripts in {MANIFEST}: not run",
                    path.display()
                );
            }
            continue;
        }
        if !(enabled.contains(&m.id) || dev.contains(path)) || ids.contains(&m.id) {
            continue;
        }
        match LuaMod::load(path) {
            Ok(lua) => {
                log::info!("Horizon: Lua mod {} loaded from {}", lua.id, path.display());
                ids.push(lua.id.clone());
                mods.push(Box::new(lua));
            }
            Err(e) => log::error!("Horizon: Lua mod in {} not loaded: {e}", path.display()),
        }
    }
    mods.into()
}

impl LuaMod {
    /// Load the mod at `path` (a `.hmod` or a mod folder): its id from
    /// `horizon-mod.toml`, its scripts read once, `scripts/init.lua` run.
    pub fn load(path: &Path) -> Result<Self, String> {
        let at = |e: String| format!("{}: {e}", path.display());
        let package = Package::read(path).map_err(at)?;
        let manifest = package.manifest;
        if !manifest.capabilities.contains(&Capability::Scripts) {
            return Err(at(format!("no capability scripts in {MANIFEST}")));
        }
        let content = Content::read(path, &manifest).map_err(at)?;
        let mut reader = content.reader().map_err(at)?;
        let mut scripts = HashMap::with_capacity(content.scripts.len());
        for name in &content.scripts {
            let bytes = reader.read(name).map_err(at)?;
            let code = String::from_utf8(bytes).map_err(|_| at(format!("{name}: not UTF-8")))?;
            scripts.insert(name.clone(), code);
        }
        let scripts: Scripts = Arc::new(scripts);
        let m = LuaMod {
            id: manifest.id.clone(),
            source: path.to_owned(),
            lua: Lua::new(),
        };
        m.install_api(scripts.clone()).map_err(|e| e.to_string())?;
        m.run(&scripts, ENTRY).map_err(|e| e.to_string())?;
        Ok(m)
    }

    /// The mod's id, as its manifest says.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn install_api(&self, scripts: Scripts) -> mlua::Result<()> {
        let lua = &self.lua;
        lua.set_named_registry_value(HANDLERS, lua.create_table()?)?;
        lua.set_named_registry_value(QUEUE, lua.create_table()?)?;
        let api = lua.create_table()?;
        let id = self.id.clone();
        api.set("mod_id", id.as_str())?;
        api.set(
            "log",
            lua.create_function(move |_, msg: String| {
                log::info!("[lua:{id}] {msg}");
                Ok(())
            })?,
        )?;
        api.set(
            "on",
            lua.create_function(|lua, (name, f): (String, Function)| {
                let handlers: Table = lua.named_registry_value(HANDLERS)?;
                let list: Table = match handlers.get::<Option<Table>>(name.as_str())? {
                    Some(list) => list,
                    None => {
                        let list = lua.create_table()?;
                        handlers.set(name.as_str(), &list)?;
                        list
                    }
                };
                list.push(f)?;
                Ok(())
            })?,
        )?;
        // Actions on the game, run after the handler with the hook's host.
        for action in ["broadcast", "summon", "kill_mob"] {
            api.set(
                action,
                lua.create_function(move |lua, args: MultiValue| {
                    let queue: Table = lua.named_registry_value(QUEUE)?;
                    let entry = lua.create_table()?;
                    entry.set("action", action)?;
                    entry.set("args", lua.create_sequence_from(args)?)?;
                    queue.push(entry)?;
                    Ok(())
                })?,
            )?;
        }
        // `require("name")`: `scripts/name.lua` of this mod, run once.
        let loaded = lua.create_table()?;
        api.set(
            "require",
            lua.create_function(move |lua, name: String| {
                if name.contains("..") || name.contains('/') || name.contains('\\') {
                    return Err(mlua::Error::runtime(format!(
                        "require({name:?}): a plain module name"
                    )));
                }
                if let Some(v) = loaded.get::<Option<Value>>(name.as_str())? {
                    return Ok(v);
                }
                let key = format!("scripts/{name}.lua");
                let code = scripts.get(&key).ok_or_else(|| {
                    mlua::Error::runtime(format!("require({name:?}): no {key} in the mod"))
                })?;
                let v: Value = lua.load(code.as_str()).set_name(&key).eval()?;
                let v = if v.is_nil() { Value::Boolean(true) } else { v };
                loaded.set(name.as_str(), &v)?;
                Ok(v)
            })?,
        )?;
        lua.globals().set(GLOBAL, &api)?;
        lua.globals()
            .set("require", api.get::<Function>("require")?)?;
        Ok(())
    }

    /// Runs the script at `name` (`scripts/init.lua`).
    fn run(&self, scripts: &Scripts, name: &str) -> mlua::Result<()> {
        let code = scripts
            .get(name)
            .ok_or_else(|| mlua::Error::runtime(format!("{name}: not in the mod")))?;
        self.lua.load(code.as_str()).set_name(name).exec()
    }

    /// Call the handlers of `hook` with `args`, then run what they queued.
    fn call(&self, host: Option<&mut dyn Host>, hook: &str, args: impl mlua::IntoLuaMulti) {
        if let Err(e) = self.call_inner(host, hook, args) {
            log::error!("[lua:{}] {hook}: {e}", self.id);
        }
    }

    fn call_inner(
        &self,
        host: Option<&mut dyn Host>,
        hook: &str,
        args: impl mlua::IntoLuaMulti,
    ) -> mlua::Result<()> {
        let lua = &self.lua;
        let handlers: Table = lua.named_registry_value(HANDLERS)?;
        let Some(list) = handlers.get::<Option<Table>>(hook)? else {
            return Ok(());
        };
        let args = args.into_lua_multi(lua)?;
        for f in list.sequence_values::<Function>() {
            f?.call::<()>(args.clone())?;
        }
        let queue: Table = lua.named_registry_value(QUEUE)?;
        if queue.raw_len() == 0 {
            return Ok(());
        }
        let actions: Vec<Table> = queue.sequence_values().collect::<mlua::Result<_>>()?;
        queue.clear()?;
        let Some(host) = host else {
            log::warn!(
                "[lua:{}] {hook}: actions need a host; {} dropped",
                self.id,
                actions.len()
            );
            return Ok(());
        };
        for entry in actions {
            let action: String = entry.get("action")?;
            let args: Table = entry.get("args")?;
            self.run_action(host, &action, &args)?;
        }
        Ok(())
    }

    fn run_action(&self, host: &mut dyn Host, action: &str, args: &Table) -> mlua::Result<()> {
        match action {
            "broadcast" => {
                let line: String = args.get(1)?;
                host.broadcast(crate::mod_api::Text::literal(line));
            }
            "summon" => {
                let (dimension, kind): (String, String) = (args.get(1)?, args.get(2)?);
                let pos = [
                    args.get::<f64>(3)?,
                    args.get::<f64>(4)?,
                    args.get::<f64>(5)?,
                ];
                let id = host.summon(&dimension, &kind, pos, Default::default());
                log::debug!("[lua:{}] summon {kind} -> {id:?}", self.id);
            }
            "kill_mob" => {
                let dimension: String = args.get(1)?;
                let id: u32 = args.get(2)?;
                host.kill_mob(&dimension, id);
            }
            other => log::warn!("[lua:{}] unknown action {other}", self.id),
        }
        Ok(())
    }
}

impl Hooks for LuaMod {
    fn world_lifecycle(&self, host: &mut dyn Host, event: &mut WorldLifecycleEvent<'_>) {
        let phase = format!("{:?}", event.phase);
        self.call(Some(host), "world_lifecycle", phase);
    }
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        let (phase, running) = match event.phase {
            ServerTickPhase::ServerTickStart { world_running } => ("start", world_running),
            ServerTickPhase::ServerTickEnd { world_running } => ("end", world_running),
        };
        self.call(Some(host), "server_tick", (phase, running));
    }
    fn mob_spawned(&self, host: &mut dyn Host, event: &mut MobSpawnedEvent<'_>) {
        let MobSpawnedEvent {
            dimension,
            kind,
            pos,
            natural,
            ..
        } = *event;
        self.call(
            Some(host),
            "mob_spawned",
            (dimension, kind, pos[0], pos[1], pos[2], natural),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_TEXT: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["scripts"]
experimental = ["scripts"]
"#;

    fn hmod(name: &str, files: &[(&str, &[u8])]) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("horizon-lua-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ruby.hmod");
        std::fs::write(
            &path,
            Package::write_files(MANIFEST_TEXT, None, files).unwrap(),
        )
        .unwrap();
        (dir, path)
    }

    #[test]
    fn a_scripted_hmod_loads_and_its_scripts_see_each_other() {
        let (dir, path) = hmod(
            "ok",
            &[
                (
                    "scripts/init.lua",
                    b"local n = require('util')\nassert(n == 42, 'util')\nassert(require('util') == 42)\nhorizon.on('server_tick', function() end)\nhorizon.log('ready ' .. horizon.mod_id)",
                ),
                ("scripts/util.lua", b"return 42"),
            ],
        );
        let m = LuaMod::load(&path).unwrap();
        assert_eq!(m.id(), "ruby");
        let handlers: Table = m.lua.named_registry_value(HANDLERS).unwrap();
        let ticks: Table = handlers.get("server_tick").unwrap();
        assert_eq!(ticks.len().unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_module_or_a_failing_script_is_an_error_with_the_file_named() {
        let (dir, path) = hmod("missing", &[("scripts/init.lua", b"require('nothing')")]);
        let e = LuaMod::load(&path).unwrap_err();
        assert!(e.contains("scripts/nothing.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();

        let (dir, path) = hmod("broken", &[("scripts/init.lua", b"error('boom')")]);
        let e = LuaMod::load(&path).unwrap_err();
        assert!(e.contains("boom") && e.contains("scripts/init.lua"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
