//! `scripts/registry.lua`: a scripted mod's declarations written in Lua
//! instead of JSON files (feature `lua`). The script runs once, when the
//! mod's content is read, in a Luau state of its own with nothing but the
//! `registry` table and a `require` that reaches the mod's own registry
//! modules (`@registry/items` is `scripts/registry/items.lua`): no other
//! files (mlua's filesystem `require` is replaced, `loadstring` refuses),
//! no network, a memory cap and a time budget. What it registers becomes content files in memory, checked and
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
//! registry.lang("en_us", { ["entity.my_mod.crawler"] = "Crawler" })
//! require("@registry/blocks") -- scripts/registry/blocks.lua, run once
//! registry.file("assets/my_mod/models/block/hive.json", { parent = "minecraft:block/cube_all" })
//! ```
use mlua::{Lua, Table, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The script, run when the content is read.
pub const REGISTRY_SCRIPT: &str = "scripts/registry.lua";
/// The folder of the modules it may `require`, and the alias they go by.
pub const MODULES: &str = "scripts/registry/";
const ALIAS: &str = "registry";
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

/// What the script registered so far: the files by path and their bytes in
/// all, checked against the content caps as each is added (the Lua memory
/// cap does not cover this side), and the `registry.lang` entries, by
/// locale and key, with the text and the call that gave it.
#[derive(Default)]
struct Registered {
    files: BTreeMap<String, Vec<u8>>,
    total: u64,
    lang: BTreeMap<String, BTreeMap<String, (String, String)>>,
}
type Files = Arc<Mutex<Registered>>;

/// The mod's registry modules, `scripts/registry/**.lua`: their code by path.
pub type Modules = BTreeMap<String, String>;

/// The registry key of the modules `require` ran: `name -> value`, `false`
/// while one runs.
const LOADED: &str = "registry_loaded";

/// Runs `code` as the registry script of the mod `id` owning `namespaces`
/// (its id among them), with its `modules`: the files it registered, by path.
pub fn run(
    code: &str,
    modules: &Modules,
    id: &str,
    namespaces: &[String],
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    run_within(code, modules, id, namespaces, TIME_BUDGET)
}

/// [`run`] with its own time budget.
pub fn run_within(
    code: &str,
    modules: &Modules,
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
    install(&lua, id, namespaces, &files, modules).map_err(at)?;
    // `=name`: errors and `registry.lang`'s call sites read
    // `scripts/registry.lua:2`, not `[string "scripts/registry.lua"]:2`.
    lua.load(code)
        .set_name(format!("={REGISTRY_SCRIPT}"))
        .exec()
        .map_err(at)?;
    lua.remove_interrupt();
    let lang = std::mem::take(&mut files.lock().map_err(|_| "registry".to_owned())?.lang);
    // Each locale's entries, from every module, as one lang file.
    for (locale, entries) in lang {
        let path = format!("assets/{id}/lang/{locale}.json");
        if files
            .lock()
            .map_err(|_| "registry".to_owned())?
            .files
            .contains_key(&path)
        {
            return Err(at(mlua::Error::runtime(format!(
                "{path}: registry.lang({locale:?}) and registry.asset(\"lang/{locale}\") or registry.file give the same lang file: use one"
            ))));
        }
        let json: serde_json::Map<String, serde_json::Value> = entries
            .into_iter()
            .map(|(key, (text, _))| (key, serde_json::Value::String(text)))
            .collect();
        let bytes = serde_json::to_vec_pretty(&json).map_err(|e| format!("{path}: {e}"))?;
        add(&files, path, bytes).map_err(at)?;
    }
    let out = files
        .lock()
        .map_err(|_| "registry".to_owned())?
        .files
        .clone();
    Ok(out)
}

/// Where the Lua code calling into Rust is: `scripts/registry/items.lua:12`.
fn site(lua: &Lua) -> String {
    lua.inspect_stack(1, |d| {
        let src = d.source().short_src.map(|s| s.into_owned());
        match (src, d.current_line()) {
            (Some(src), Some(line)) => format!("{src}:{line}"),
            (Some(src), None) => src,
            _ => "?".to_owned(),
        }
    })
    .unwrap_or_else(|| "?".to_owned())
}

/// A locale as the lang files are named: `en_us`, `lol_us`, `enws`.
fn plain_locale(locale: &str) -> bool {
    (2..=16).contains(&locale.len())
        && locale
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The `registry` global.
fn install(
    lua: &Lua,
    id: &str,
    namespaces: &[String],
    files: &Files,
    modules: &Modules,
) -> mlua::Result<()> {
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
    // `registry.lang(locale, entries)`: names for the lang file of
    // `locale`, merged from every call into `assets/<id>/lang/<locale>.json`
    // when the script ends. A key given twice is an error, naming both calls.
    let f = files.clone();
    registry.set(
        "lang",
        lua.create_function(move |lua, (locale, entries): (String, Table)| {
            if !plain_locale(&locale) {
                return Err(mlua::Error::runtime(format!(
                    "registry.lang({locale:?}): a locale as lang files are named (en_us): lower-case letters, digits and underscores"
                )));
            }
            let at = site(lua);
            let mut added = Vec::new();
            for pair in entries.pairs::<Value, Value>() {
                let (key, text) = pair?;
                let (Value::String(key), Value::String(text)) = (&key, &text) else {
                    return Err(mlua::Error::runtime(format!(
                        "registry.lang({locale:?}): keys and texts are strings, not {} = {}",
                        key.type_name(),
                        text.type_name()
                    )));
                };
                let utf8 = |s: &mlua::LuaString| {
                    s.to_str().map(|s| s.to_owned()).map_err(|_| {
                        mlua::Error::runtime(format!(
                            "registry.lang({locale:?}): a string that is not UTF-8"
                        ))
                    })
                };
                added.push((utf8(key)?, utf8(text)?));
            }
            let mut files = f.lock().map_err(|_| mlua::Error::runtime("registry"))?;
            let lang = files.lang.entry(locale.clone()).or_default();
            for (key, text) in added {
                if let Some((_, before)) = lang.get(&key) {
                    return Err(mlua::Error::runtime(format!(
                        "registry.lang({locale:?}): {key:?} given twice, at {before} and at {at}"
                    )));
                }
                lang.insert(key, (text, at.clone()));
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("registry", registry)?;
    // mlua's Luau state comes with a `require` that reads `.lua` files from
    // disk, relative to the working directory, and a `loadstring`: the
    // script declares content and reaches no file and no other code but its
    // own modules, which `require` runs from the mod's content.
    lua.globals().set(
        "loadstring",
        lua.create_function(move |_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::runtime(format!(
                "loadstring: {REGISTRY_SCRIPT} declares content and loads no other code"
            )))
        })?,
    )?;
    lua.set_named_registry_value(LOADED, lua.create_table()?)?;
    let modules = Arc::new(modules.clone());
    lua.globals().set(
        "require",
        lua.create_function(move |lua, name: String| {
            // `@registry/items`, `registry/items` or `registry.items`, as the
            // mod's scripts require theirs: `scripts/registry/items.lua` or
            // `scripts/registry/items/init.lua`, run once.
            let dotted = name.strip_prefix('@').unwrap_or(&name).replace('/', ".");
            let plain = !dotted.contains("..")
                && dotted
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.');
            let Some(rest) = dotted
                .strip_prefix(ALIAS)
                .and_then(|r| r.strip_prefix('.'))
                .filter(|r| plain && !r.is_empty())
            else {
                return Err(mlua::Error::runtime(format!(
                    "require({name:?}): {REGISTRY_SCRIPT} requires its own modules only, \
                     @{ALIAS}/<name> for {MODULES}<name>.lua; no other code"
                )));
            };
            let loaded: Table = lua.named_registry_value(LOADED)?;
            match loaded.get::<Value>(dotted.as_str())? {
                Value::Nil => {}
                Value::Boolean(false) => {
                    return Err(mlua::Error::runtime(format!(
                        "require({name:?}): a loop, {dotted} requires itself on the way"
                    )));
                }
                v => return Ok(v),
            }
            let path = rest.replace('.', "/");
            let keys = [
                format!("{MODULES}{path}.lua"),
                format!("{MODULES}{path}/init.lua"),
            ];
            let Some((key, code)) = keys
                .iter()
                .find_map(|k| modules.get(k).map(|code| (k, code)))
            else {
                return Err(mlua::Error::runtime(format!(
                    "require({name:?}): no {} in the mod",
                    keys[0]
                )));
            };
            loaded.set(dotted.as_str(), false)?;
            let v: Value = lua.load(code.as_str()).set_name(format!("={key}")).eval()?;
            let v = if v.is_nil() { Value::Boolean(true) } else { v };
            loaded.set(dotted.as_str(), &v)?;
            Ok(v)
        })?,
    )?;
    Ok(())
}

/// `ns:rest` as given, else the mod's own namespace.
fn split<'a>(name: &'a str, id: &'a str) -> (&'a str, &'a str) {
    match name.split_once(':') {
        Some((ns, rest)) => (ns, rest),
        None => (id, name),
    }
}

/// Keeps a registered file: a plain content path under `assets/` or `data/`
/// (a `scripts/` path would shadow one of the mod's own scripts: the
/// registry declares content, not code), not registered before, and within
/// the content caps, checked here so a script cannot fill memory before
/// `Content::read` would refuse the result.
fn add(files: &Files, path: String, bytes: Vec<u8>) -> mlua::Result<()> {
    use crate::content::{MAX_FILE, MAX_FILES, MAX_TOTAL};
    crate::content::check_path(&path).map_err(mlua::Error::runtime)?;
    if path.starts_with("scripts/") {
        return Err(mlua::Error::runtime(format!(
            "{path}: the registry declares content under assets/ and data/; scripts/ holds the mod's own files"
        )));
    }
    if bytes.len() as u64 > MAX_FILE {
        return Err(mlua::Error::runtime(format!(
            "{path}: over {} MiB",
            MAX_FILE >> 20
        )));
    }
    let mut files = files.lock().map_err(|_| mlua::Error::runtime("registry"))?;
    if files.files.contains_key(&path) {
        return Err(mlua::Error::runtime(format!("{path}: registered twice")));
    }
    if files.files.len() >= MAX_FILES {
        return Err(mlua::Error::runtime(format!(
            "{path}: more than {MAX_FILES} content files"
        )));
    }
    if files.total + bytes.len() as u64 > MAX_TOTAL {
        return Err(mlua::Error::runtime(format!(
            "{path}: the content is over {} MiB",
            MAX_TOTAL >> 20
        )));
    }
    files.total += bytes.len() as u64;
    files.files.insert(path, bytes);
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
        run_modules(code, &[])
    }

    fn run_modules(
        code: &str,
        modules: &[(&str, &str)],
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let modules: Modules = modules
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect();
        run_within(
            code,
            &modules,
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
    fn the_content_caps_stop_a_script_as_it_registers() {
        // One file past the count is refused where it is added, not once a
        // million are in memory; the total of their bytes the same.
        let e = run_within(
            &format!(
                "for i = 1, {} do registry.file('data/ruby/' .. i .. '.json', 'x') end",
                crate::content::MAX_FILES + 1
            ),
            &Modules::new(),
            "ruby",
            &["ruby".to_owned()],
            Duration::from_secs(30),
        )
        .unwrap_err();
        assert!(
            e.contains("more than") && e.contains("content files"),
            "{e}"
        );
        let e = run_within(
            &format!(
                "local big = string.rep('x', {}) for i = 1, {} do registry.file('data/ruby/' .. i .. '.txt', big) end",
                crate::content::MAX_FILE,
                crate::content::MAX_TOTAL / crate::content::MAX_FILE + 1
            ),
            &Modules::new(),
            "ruby",
            &["ruby".to_owned()],
            Duration::from_secs(30),
        )
        .unwrap_err();
        assert!(e.contains("over") && e.contains("MiB"), "{e}");
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
        // A scripts/ path would shadow a script of the mod's own.
        let e = run_ruby("registry.file('scripts/init.lua', 'x')").unwrap_err();
        assert!(
            e.contains("scripts/init.lua") && e.contains("scripts/"),
            "{e}"
        );
        let e = run_ruby("require('../../x')").unwrap_err();
        assert!(e.contains("require") && e.contains("no other code"), "{e}");
        let e = run_ruby("require('@modkit/entity')").unwrap_err();
        assert!(e.contains("own modules only"), "{e}");
        let e = run_ruby("loadstring('registry.block(\"a\", {})')()").unwrap_err();
        assert!(
            e.contains("loadstring") && e.contains("no other code"),
            "{e}"
        );
    }

    #[test]
    fn require_runs_the_mods_registry_modules_once_each() {
        let files = run_modules(
            r#"
            local items = require("@registry/items")
            require("registry/blocks")
            assert(require("registry.items") == items, "the same value, once")
            assert(require("@registry/mobs") == true, "nil returns true")
            registry.item(items.name, { template = "minecraft:stick" })
            "#,
            &[
                (
                    "scripts/registry/items.lua",
                    "registry.data('recipe/a', { type = 'x' }) return { name = 'wand' }",
                ),
                (
                    "scripts/registry/blocks/init.lua",
                    "registry.block('hive', { template = 'minecraft:sculk' })",
                ),
                (
                    "scripts/registry/mobs.lua",
                    "registry.entity_type('crawler', { template = 'minecraft:zombie' })",
                ),
            ],
        )
        .unwrap();
        let paths: Vec<&String> = files.keys().collect();
        assert_eq!(
            paths,
            [
                "data/ruby/horizon/block/hive.json",
                "data/ruby/horizon/entity_type/crawler.json",
                "data/ruby/horizon/item/wand.json",
                "data/ruby/recipe/a.json",
            ]
        );
        // Only the mod's own modules, by name: a missing one, a path out of
        // the folder and a loop are errors naming them.
        let e = run_modules("require('@registry/gone')", &[]).unwrap_err();
        assert!(e.contains("no scripts/registry/gone.lua"), "{e}");
        let e = run_modules("require('@registry/../init')", &[]).unwrap_err();
        assert!(e.contains("own modules only"), "{e}");
        let e = run_modules("require('@registry')", &[]).unwrap_err();
        assert!(e.contains("own modules only"), "{e}");
        let e = run_modules(
            "require('@registry/a')",
            &[
                ("scripts/registry/a.lua", "require('@registry/b')"),
                ("scripts/registry/b.lua", "require('@registry/a')"),
            ],
        )
        .unwrap_err();
        assert!(e.contains("a loop"), "{e}");
        // An error in a module names the module and its line.
        let e = run_modules(
            "require('@registry/items')",
            &[("scripts/registry/items.lua", "\nerror('boom')")],
        )
        .unwrap_err();
        assert!(
            e.contains("scripts/registry/items.lua:2") && e.contains("boom"),
            "{e}"
        );
    }

    #[test]
    fn lang_entries_from_every_module_merge_into_one_file_per_locale() {
        let files = run_modules(
            r#"
            require("@registry/items")
            registry.lang("en_us", { ["block.ruby.hive"] = "Hive" })
            registry.lang("de_de", { ["block.ruby.hive"] = "Stock" })
            "#,
            &[(
                "scripts/registry/items.lua",
                r#"registry.lang("en_us", { ["item.ruby.wand"] = "Wand" })"#,
            )],
        )
        .unwrap();
        let json =
            |path: &str| -> serde_json::Value { serde_json::from_slice(&files[path]).unwrap() };
        assert_eq!(
            json("assets/ruby/lang/en_us.json"),
            serde_json::json!({ "block.ruby.hive": "Hive", "item.ruby.wand": "Wand" })
        );
        assert_eq!(
            json("assets/ruby/lang/de_de.json"),
            serde_json::json!({ "block.ruby.hive": "Stock" })
        );
        // A key twice names both calls, even with the same text.
        let e = run_modules(
            "require('@registry/items')\nregistry.lang('en_us', { ['item.ruby.wand'] = 'Wand' })",
            &[(
                "scripts/registry/items.lua",
                "registry.lang('en_us', { ['item.ruby.wand'] = 'Wand' })",
            )],
        )
        .unwrap_err();
        assert!(
            e.contains("given twice")
                && e.contains("scripts/registry/items.lua:1")
                && e.contains("scripts/registry.lua:2"),
            "{e}"
        );
        // With registry.asset for the same lang file: use one.
        let e = run_ruby(
            "registry.lang('en_us', { a = 'A' })\nregistry.asset('lang/en_us', { b = 'B' })",
        )
        .unwrap_err();
        assert!(e.contains("use one"), "{e}");
        // Another namespace's lang file is another file.
        run_ruby(
            "registry.lang('en_us', { a = 'A' })\nregistry.asset('gems:lang/en_us', { b = 'B' })",
        )
        .unwrap();
        let e = run_ruby("registry.lang('en-US', { a = 'A' })").unwrap_err();
        assert!(e.contains("a locale"), "{e}");
        let e = run_ruby("registry.lang('en_us', { a = 1 })").unwrap_err();
        assert!(e.contains("strings"), "{e}");
        let e = run_ruby("registry.lang('en_us', { 'A' })").unwrap_err();
        assert!(e.contains("strings"), "{e}");
    }
}
