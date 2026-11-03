//! `scripts/registry.lua`: a scripted mod's declarations written in Lua
//! instead of JSON files (feature `lua`). The script runs once, when the
//! mod's content is read, in a Luau state of its own with nothing but the
//! `registry` table: no files (mlua's filesystem `require` and `loadstring`
//! are replaced by stubs that refuse), no network, a memory cap and a time
//! budget. What it registers becomes content files in memory, checked and
//! read like the ones in `data/` and `assets/`; a path both in the mod and
//! registered is an error. The game runs it when it loads the mod, and so
//! does the launcher (built with the feature) when it inspects a mod it
//! installs; the registry server and its CI (feature `index` alone) list
//! the mod's files only, so an error in the script is first seen there.
//!
//! ```lua
//! -- scripts/registry.lua: `registry.id` is the mod's id
//! registry.entity_type("crawler", { template = "minecraft:zombie", goals = "none" })
//! registry.block("hive", { template = "minecraft:sculk", item = true })
//! registry.data("loot_table/entities/crawler", { type = "minecraft:entity", pools = {} })
//! registry.data("minecraft:tags/block/mineable/hoe", { values = { "my_mod:hive" } })
//! registry.asset("lang/en_us", { ["entity.my_mod.crawler"] = "Crawler" })
//! registry.file("assets/my_mod/models/block/hive.json", { parent = "minecraft:block/cube_all" })
//! ```
use mlua::{Lua, Table, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The script, run when the content is read.
pub const REGISTRY_SCRIPT: &str = "scripts/registry.lua";
/// The time a script gets, and the memory its state may hold.
pub const TIME_BUDGET: Duration = Duration::from_secs(5);
pub const MEMORY: usize = 64 << 20;
/// How deep a registered table may nest.
const MAX_DEPTH: usize = 64;

/// The declaration kinds under `data/<id>/horizon/`, as `registry.<kind>(name, def)`.
const KINDS: [&str; 6] = [
    "block",
    "item",
    "mob_effect",
    "creative_tab",
    "entity_type",
    "menu",
];

type Files = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;

/// Runs `code` as the registry script of the mod `id` owning `namespaces`
/// (its id among them): the files it registered, by path.
pub fn run(
    code: &str,
    id: &str,
    namespaces: &[String],
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    run_within(code, id, namespaces, TIME_BUDGET)
}

/// [`run`] with its own time budget.
pub fn run_within(
    code: &str,
    id: &str,
    namespaces: &[String],
    budget: Duration,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let at = |e: mlua::Error| format!("{REGISTRY_SCRIPT}: {e}");
    let lua = Lua::new();
    lua.set_memory_limit(MEMORY).map_err(at)?;
    let deadline = Instant::now() + budget;
    lua.set_interrupt(move |_| {
        if Instant::now() > deadline {
            Err(mlua::Error::runtime(
                "ran over its time budget and was stopped (an endless loop?)",
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    let files: Files = Default::default();
    install(&lua, id, namespaces, &files).map_err(at)?;
    lua.load(code)
        .set_name(REGISTRY_SCRIPT)
        .exec()
        .map_err(at)?;
    lua.remove_interrupt();
    let out = files.lock().map_err(|_| "registry".to_owned())?.clone();
    Ok(out)
}

/// The `registry` global.
fn install(lua: &Lua, id: &str, namespaces: &[String], files: &Files) -> mlua::Result<()> {
    let registry = lua.create_table()?;
    registry.set("id", id)?;
    registry.set("namespaces", namespaces.to_vec())?;
    let owned: Arc<Vec<String>> = Arc::new(namespaces.to_vec());
    let id: Arc<str> = id.into();
    // `registry.file(path, value)`: any content path; a table is written as
    // JSON, a string as it is.
    let f = files.clone();
    registry.set(
        "file",
        lua.create_function(move |_, (path, value): (String, Value)| {
            let bytes = bytes_of(&path, value)?;
            add(&f, path, bytes)
        })?,
    )?;
    // `registry.data(name, value)` and `registry.asset(name, value)`:
    // `data/<ns>/<name>.json` and `assets/<ns>/<name>.json`, the namespace
    // from `ns:name` or the mod's id.
    for (function, top) in [("data", "data"), ("asset", "assets")] {
        let f = files.clone();
        let id = id.clone();
        registry.set(
            function,
            lua.create_function(move |_, (name, value): (String, Value)| {
                let (ns, rest) = split(&name, &id);
                let path = format!("{top}/{ns}/{rest}.json");
                let bytes = bytes_of(&path, value)?;
                add(&f, path, bytes)
            })?,
        )?;
    }
    // `registry.block(name, def)` and the other kinds: a declaration in
    // `data/<id>/horizon/<kind>/<name>.json`, in the mod's own namespace.
    for kind in KINDS {
        let f = files.clone();
        let id = id.clone();
        let owned = owned.clone();
        registry.set(
            kind,
            lua.create_function(move |_, (name, def): (String, Table)| {
                let (ns, rest) = split(&name, &id);
                if ns != &*id {
                    let own = owned.iter().any(|n| n == ns);
                    return Err(mlua::Error::runtime(format!(
                        "registry.{kind}({name:?}): a {kind} is declared in the mod's own namespace, {id}{}",
                        if own { " (a listed namespace holds files only)" } else { "" }
                    )));
                }
                let path = format!("data/{ns}/horizon/{kind}/{rest}.json");
                let bytes = bytes_of(&path, Value::Table(def))?;
                add(&f, path, bytes)
            })?,
        )?;
    }
    lua.globals().set("registry", registry)?;
    // mlua's Luau state comes with a `require` that reads `.lua` files from
    // disk, relative to the working directory, and a `loadstring`: the
    // script declares content and reaches no file and no other code.
    for name in ["require", "loadstring"] {
        lua.globals().set(
            name,
            lua.create_function(move |_, _: mlua::MultiValue| -> mlua::Result<()> {
                Err(mlua::Error::runtime(format!(
                    "{name}: {REGISTRY_SCRIPT} declares content and loads no other code"
                )))
            })?,
        )?;
    }
    Ok(())
}

/// `ns:rest` as given, else the mod's own namespace.
fn split<'a>(name: &'a str, id: &'a str) -> (&'a str, &'a str) {
    match name.split_once(':') {
        Some((ns, rest)) => (ns, rest),
        None => (id, name),
    }
}

fn add(files: &Files, path: String, bytes: Vec<u8>) -> mlua::Result<()> {
    crate::content::check_path(&path).map_err(mlua::Error::runtime)?;
    let mut files = files.lock().map_err(|_| mlua::Error::runtime("registry"))?;
    if files.contains_key(&path) {
        return Err(mlua::Error::runtime(format!("{path}: registered twice")));
    }
    files.insert(path, bytes);
    Ok(())
}

/// A registered value as a file: a table as JSON, a string as it is.
fn bytes_of(path: &str, value: Value) -> mlua::Result<Vec<u8>> {
    match value {
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Table(t) => {
            let json = to_json(Value::Table(t), 0)
                .map_err(|e| mlua::Error::runtime(format!("{path}: {e}")))?;
            serde_json::to_vec_pretty(&json)
                .map_err(|e| mlua::Error::runtime(format!("{path}: {e}")))
        }
        other => Err(mlua::Error::runtime(format!(
            "{path}: a table (written as JSON) or a string, not {}",
            other.type_name()
        ))),
    }
}

/// A Lua value as JSON: a table with the keys 1..n in order is an array, an
/// empty one or one with string keys an object; a number that is whole is
/// an integer.
fn to_json(value: Value, depth: usize) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;
    if depth > MAX_DEPTH {
        return Err(format!(
            "nested deeper than {MAX_DEPTH} (a table that holds itself?)"
        ));
    }
    Ok(match value {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(b),
        Value::Integer(i) => J::Number(i.into()),
        Value::Number(n) => {
            if !n.is_finite() {
                return Err(format!("{n} is not a number JSON can hold"));
            }
            if n.fract() == 0.0 && n.abs() < 9007199254740992.0 {
                J::Number((n as i64).into())
            } else {
                J::Number(serde_json::Number::from_f64(n).ok_or_else(|| format!("{n}"))?)
            }
        }
        Value::String(s) => J::String(
            s.to_str()
                .map_err(|_| "a string that is not UTF-8".to_owned())?
                .to_owned(),
        ),
        Value::Table(t) => {
            let len = t.raw_len();
            let mut pairs = Vec::new();
            for pair in t.pairs::<Value, Value>() {
                pairs.push(pair.map_err(|e| e.to_string())?);
            }
            let sequence = len > 0
                && pairs.len() == len
                && pairs
                    .iter()
                    .all(|(k, _)| matches!(k, Value::Integer(i) if (1..=len as i64).contains(i)));
            if sequence {
                let mut items = vec![J::Null; len];
                for (k, v) in pairs {
                    if let Value::Integer(i) = k {
                        items[i as usize - 1] = to_json(v, depth + 1)?;
                    }
                }
                J::Array(items)
            } else {
                let mut map = serde_json::Map::new();
                for (k, v) in pairs {
                    let key = match k {
                        Value::String(s) => s
                            .to_str()
                            .map_err(|_| "a key that is not UTF-8".to_owned())?
                            .to_owned(),
                        Value::Integer(i) => {
                            return Err(format!(
                                "key {i} in a table that is not a plain sequence (keys 1..n) nor all strings"
                            ));
                        }
                        other => return Err(format!("a {} as a key", other.type_name())),
                    };
                    map.insert(key, to_json(v, depth + 1)?);
                }
                J::Object(map)
            }
        }
        other => return Err(format!("a {} cannot be written as JSON", other.type_name())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_ruby(code: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
        run_within(
            code,
            "ruby",
            &["ruby".to_owned(), "gems".to_owned()],
            Duration::from_millis(300),
        )
    }

    #[test]
    fn registered_values_become_json_files_in_the_mods_namespaces() {
        let files = run_ruby(
            r#"
            registry.entity_type("crawler", { template = "minecraft:zombie", attributes = { max_health = 30, speed = 0.25 }, goals = "none", flags = { burns_in_day = false } })
            registry.block("gems:hive", { template = "minecraft:sculk", item = true, tags = {}, pattern = { "FSF", "SAS" } })
            registry.data("loot_table/entities/crawler", { type = "minecraft:entity", pools = { { rolls = 1 } } })
            registry.data("minecraft:tags/block/mineable/hoe", { values = { registry.id .. ":hive" } })
            registry.asset("lang/en_us", { ["entity.ruby.crawler"] = "Crawler" })
            registry.file("assets/ruby/models/block/hive.json", '{"parent": "minecraft:block/cube_all"}')
            "#,
        );
        let e = files.unwrap_err();
        assert!(
            e.contains("registry.block(\"gems:hive\")") && e.contains("listed namespace"),
            "{e}"
        );
        let files = run_ruby(
            r#"
            registry.entity_type("crawler", { template = "minecraft:zombie", attributes = { max_health = 30, speed = 0.25 }, goals = "none", flags = { burns_in_day = false } })
            registry.block("hive", { template = "minecraft:sculk", item = true, tags = {}, pattern = { "FSF", "SAS" } })
            registry.data("loot_table/entities/crawler", { type = "minecraft:entity", pools = { { rolls = 1 } } })
            registry.data("minecraft:tags/block/mineable/hoe", { values = { registry.id .. ":hive" } })
            registry.asset("gems:lang/en_us", { ["entity.ruby.crawler"] = "Crawler" })
            registry.file("assets/ruby/models/block/hive.json", '{"parent": "minecraft:block/cube_all"}')
            "#,
        )
        .unwrap();
        let paths: Vec<&String> = files.keys().collect();
        assert_eq!(
            paths,
            [
                "assets/gems/lang/en_us.json",
                "assets/ruby/models/block/hive.json",
                "data/minecraft/tags/block/mineable/hoe.json",
                "data/ruby/horizon/block/hive.json",
                "data/ruby/horizon/entity_type/crawler.json",
                "data/ruby/loot_table/entities/crawler.json",
            ]
        );
        let json =
            |path: &str| -> serde_json::Value { serde_json::from_slice(&files[path]).unwrap() };
        let crawler = json("data/ruby/horizon/entity_type/crawler.json");
        assert_eq!(crawler["attributes"]["max_health"], 30);
        assert_eq!(crawler["attributes"]["speed"], 0.25);
        assert_eq!(crawler["flags"]["burns_in_day"], false);
        let hive = json("data/ruby/horizon/block/hive.json");
        assert_eq!(hive["tags"], serde_json::json!({}));
        assert_eq!(hive["pattern"], serde_json::json!(["FSF", "SAS"]));
        assert_eq!(
            json("data/minecraft/tags/block/mineable/hoe.json")["values"][0],
            "ruby:hive"
        );
        assert_eq!(
            json("data/ruby/loot_table/entities/crawler.json")["pools"][0]["rolls"],
            1
        );
        assert_eq!(
            json("assets/ruby/models/block/hive.json")["parent"],
            "minecraft:block/cube_all"
        );
    }

    #[test]
    fn a_bad_script_is_an_error_naming_it() {
        let e = run_ruby("registry.block('a', { template = 'x' })\nerror('boom')").unwrap_err();
        assert!(
            e.starts_with("scripts/registry.lua:") && e.contains("boom"),
            "{e}"
        );
        let e = run_ruby("registry.block('a', {})\nregistry.block('a', {})").unwrap_err();
        assert!(e.contains("registered twice"), "{e}");
        let e = run_ruby("registry.file('../x.json', {})").unwrap_err();
        assert!(e.contains("plain"), "{e}");
        let e = run_ruby("registry.data('x', { [2] = 'b' })").unwrap_err();
        assert!(e.contains("key 2"), "{e}");
        let e = run_ruby("while true do end").unwrap_err();
        assert!(e.contains("time budget"), "{e}");
        let e = run_ruby("local t = {} t.t = t registry.data('x', t)").unwrap_err();
        assert!(e.contains("nested deeper"), "{e}");
        assert!(run_ruby("io.open('x')").is_err());
        // The state reaches no file and no other code: mlua's own `require`
        // (which reads .lua files from disk) and `loadstring` are gone.
        let e = run_ruby("require('../../x')").unwrap_err();
        assert!(e.contains("require") && e.contains("no other code"), "{e}");
        let e = run_ruby("loadstring('registry.block(\"a\", {})')()").unwrap_err();
        assert!(
            e.contains("loadstring") && e.contains("no other code"),
            "{e}"
        );
    }
}
