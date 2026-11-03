//! `cargo run -p xtask -- hooks-doc`: `docs/api/hook-points.md`, the reference
//! of every hook point and host helper in `native-hooks.json`, with the Lua
//! handler each hook reaches, from the catalog and the `native_*` functions'
//! own docs, so the list never trails the code. `--check` fails when the file
//! is not what the sources say.
use horizon_mod_api_gen::{
    catalog_hooks, doc_comments, free_function_source, impl_functions, one_function,
};
use std::{collections::BTreeMap, fs, path::Path};
use syn::Item;

/// The engine hooks a scripted mod hears, and as what (`docs/lua-bindings.md`).
const LUA_HOOKS: &[(&str, &str)] = &[
    ("server_tick", "`server_tick(phase, world_running)`"),
    ("world_lifecycle", "`world_lifecycle(phase)`"),
    ("data_pack_files", "reloads the scripts on `/reload`"),
    (
        "mob_spawned",
        "`mob_spawned(dimension, kind, x, y, z, natural)`, by kind",
    ),
    ("mob_ai_register", "`mob_ai_register(mob)`, by kind"),
    (
        "living_incoming_damage",
        "`mob_hurt(mob, damage_type, causing, direct, amount)`, by kind",
    ),
    (
        "mob_killed",
        "`mob_killed(dimension, mob, killer, data)`, by kind",
    ),
    (
        "mob_died",
        "`mob_died(dimension, mob, cause, data)`, by kind",
    ),
    (
        "interact_entity",
        "`mob_interact(dimension, mob, player, held, data)`, by kind",
    ),
    (
        "entity_tick_post",
        "`mob_tick(dimension, mob, data)`, by kind",
    ),
    (
        "entity_tick_listened",
        "answered while a `mob_tick` handler exists",
    ),
    (
        "random_tick",
        "`random_tick(dimension, block, x, y, z, props, data)`, by block",
    ),
    (
        "tick_block",
        "`tick_block(dimension, block, x, y, z, props, data)`, by block",
    ),
    (
        "right_click_block",
        "`right_click_block(x, y, z, block, face, player, held, data)`",
    ),
    (
        "block_entity",
        "`horizon.block_entity(block, { ticks, data })`",
    ),
    (
        "block_entity_ticks",
        "`horizon.block_entity(block, { ticks = true })`",
    ),
    (
        "block_entity_tick",
        "`block_entity_tick(dimension, block, x, y, z, data)`, by block",
    ),
    (
        "destroy_block",
        "`block_broken(x, y, z, block, player)`, by block",
    ),
    (
        "place_block",
        "`block_placed(x, y, z, block, player, item)`, by block",
    ),
    ("use_item", "`item_use(item, player)`, by item"),
    (
        "use_item_on",
        "`item_use_on(item, player, block, face)`, by item",
    ),
    (
        "attack_entity",
        "`item_hit(item, player, target)`, by held item",
    ),
    (
        "living_entity_use_item_finish",
        "`item_finished(item, player)`, by item",
    ),
    (
        "inventory_tick",
        "`item_tick(item, player, slot, selected)`, by item",
    ),
    (
        "item_picked_up",
        "`item_picked_up(player, item, count)`, by item",
    ),
    ("player_logged_in", "`player_joined(player)`"),
    ("player_logged_out", "`player_left(player)`"),
    ("player_respawned", "`player_respawned(player)`"),
    ("player_died", "`player_died(player, cause)`"),
    ("register_commands", "`horizon.command(usage, level, fn)`"),
    ("perform_command", "`fn(player, args)` of `horizon.command`"),
];

pub fn run(check: bool) -> Result<(), String> {
    generate(&Path::new(env!("CARGO_MANIFEST_DIR")).join(".."), check)
}

/// The doc comment of a function as prose: its lines joined, up to the
/// first blank line.
fn prose(attrs: &[syn::Attribute], name: &str) -> Result<String, String> {
    let docs = doc_comments(attrs, name, true)?;
    let mut out = String::new();
    for line in docs.lines() {
        let line = line.trim_start_matches("///").trim();
        if line.is_empty() {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    Ok(out)
}

/// The API number a doc names (`API 40.`), or none.
fn api_of(prose: &str) -> Option<u32> {
    let mut last = None;
    let mut rest = prose;
    while let Some(i) = rest.find("API ") {
        let after = &rest[i + 4..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u32>() {
            last = Some(n);
        }
        rest = &after[digits.len()..];
    }
    last
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn generate(root: &Path, check: bool) -> Result<(), String> {
    let catalog: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("native-hooks.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // The `native_*` functions, by the file the target names: a game module's
    // methods, or a crate's free functions.
    let mut parsed: BTreeMap<String, Vec<(String, syn::ItemFn)>> = BTreeMap::new();
    let docs_of = |target: &str,
                   native: &str,
                   parsed: &mut BTreeMap<String, Vec<(String, syn::ItemFn)>>|
     -> Result<String, String> {
        let (file, free) = if let Some((krate, path)) = free_function_source(target) {
            let (module, _) = path
                .rsplit_once("::")
                .ok_or_else(|| format!("wrong target {target}"))?;
            (
                root.join("crates")
                    .join(krate)
                    .join("src")
                    .join(format!("{}.rs", module.replace("::", "/"))),
                true,
            )
        } else {
            let rest = target
                .strip_prefix("horizon_client::game::")
                .ok_or_else(|| format!("wrong target {target}"))?;
            // `native_hooks::Game<'_>::native_x`: the module is what comes
            // before the type.
            let module = rest
                .split("::")
                .take_while(|seg| !seg.contains('<') && !seg.starts_with("Game"))
                .collect::<Vec<_>>()
                .join("/");
            (
                root.join("crates/horizon-client/src/game")
                    .join(format!("{module}.rs")),
                false,
            )
        };
        let key = file.display().to_string();
        if !parsed.contains_key(&key) {
            let source = fs::read_to_string(&file).map_err(|e| format!("{key}: {e}"))?;
            let file = syn::parse_file(&source).map_err(|e| format!("{key}: {e}"))?;
            let mut functions = Vec::new();
            for item in file.items {
                match item {
                    Item::Fn(f) if free => functions.push((String::new(), f)),
                    Item::Impl(item) if !free => {
                        // The type implemented (`Game`, `GameLevel`...), so a
                        // name both have resolves to the target's.
                        let self_ty = match &*item.self_ty {
                            syn::Type::Path(p) => p
                                .path
                                .segments
                                .last()
                                .map(|s| s.ident.to_string())
                                .unwrap_or_default(),
                            _ => String::new(),
                        };
                        functions.extend(impl_functions(item).map(|f| (self_ty.clone(), f)));
                    }
                    _ => {}
                }
            }
            parsed.insert(key.clone(), functions);
        }
        let wanted_ty = target
            .split("::")
            .find(|seg| seg.contains('<'))
            .map(|seg| seg.split('<').next().unwrap_or_default().to_owned())
            .unwrap_or_default();
        let f = one_function(
            parsed[&key]
                .iter()
                .filter(|(ty, _)| free || *ty == wanted_ty)
                .map(|(_, f)| f),
            native,
        )?;
        prose(&f.attrs, native)
    };
    let lua: BTreeMap<&str, &str> = LUA_HOOKS.iter().copied().collect();
    let mut out = String::new();
    out.push_str("<!-- Generated by `cargo run -p xtask -- hooks-doc` from native-hooks.json and the native_* functions' docs: edit those, then regenerate. -->\n");
    out.push_str("# Hook points and host helpers\n\n");
    out.push_str("Every hook point a native mod (`docs/native-mods.md`) or a trait mod (`crates/horizon-client/src/mod_api.rs`) may listen to, \
with the API level it arrived in, how it is taken (`listen`: it only hears; `claim`, `cancel`: a mod may stop it; `edit`: every mod changes a value in turn), \
and the handler a scripted mod (`docs/lua-bindings.md`) registers for it, where one exists. Then every helper the host offers them. \
The full doc of each is on its `native_<name>` function.\n\n");
    out.push_str("## Hook points\n\n| Hook | API | Taken | Lua | What |\n|---|---|---|---|---|\n");
    let mut hooks = catalog_hooks(root, &catalog)?;
    hooks.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut count = 0;
    for hook in &hooks {
        let name = hook["name"].as_str().ok_or("hook name missing")?;
        let target = hook["target"].as_str().ok_or("target missing")?;
        let contract = hook["mod_api"]["contract"].as_str().unwrap_or("listen");
        let text = docs_of(target, &format!("native_{name}"), &mut parsed)?;
        let api = api_of(&text).map_or("—".to_owned(), |n| n.to_string());
        let lua = lua.get(name).copied().unwrap_or("");
        out.push_str(&format!(
            "| `{name}` | {api} | {contract} | {} | {} |\n",
            cell(lua),
            cell(&text)
        ));
        count += 1;
    }
    out.push_str("\n## Host helpers\n\n| Helper | API | What |\n|---|---|---|\n");
    let mut helpers = catalog["helpers"]
        .as_array()
        .ok_or("catalog helpers must be an array")?
        .clone();
    helpers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    for helper in &helpers {
        let name = helper["name"].as_str().ok_or("helper name missing")?;
        let target = helper["target"].as_str().ok_or("helper target missing")?;
        let text = docs_of(target, &format!("native_{name}"), &mut parsed)?;
        let api = api_of(&text).map_or("—".to_owned(), |n| n.to_string());
        out.push_str(&format!("| `{name}` | {api} | {} |\n", cell(&text)));
    }
    out.push_str(&format!(
        "\n{count} hook points, {} helpers, {} of the hooks reaching Lua.\n",
        helpers.len(),
        hooks
            .iter()
            .filter(|h| h["name"].as_str().is_some_and(|n| lua.contains_key(n)))
            .count()
    ));
    for (name, _) in LUA_HOOKS {
        if !hooks.iter().any(|h| h["name"].as_str() == Some(name)) {
            return Err(format!("LUA_HOOKS names {name}, which is not a hook point"));
        }
    }
    let path = root.join("docs/api/hook-points.md");
    if check {
        let current = fs::read_to_string(&path).unwrap_or_default();
        if current != out {
            return Err(format!(
                "{} is out of date: run cargo run -p xtask -- hooks-doc",
                path.display()
            ));
        }
    } else {
        fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The hooks the Lua runtime answers (`impl Hooks for LuaMod`) are the
    /// ones the reference says reach Lua, no more and no fewer.
    #[test]
    fn the_hooks_the_runtime_answers_are_the_ones_the_reference_names() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let src =
            std::fs::read_to_string(root.join("crates/horizon-client/src/lua_mods.rs")).unwrap();
        let file = syn::parse_file(&src).unwrap();
        let mut answered = std::collections::BTreeSet::new();
        for item in file.items {
            if let syn::Item::Impl(i) = item
                && let Some((_, path, _)) = &i.trait_
                && path.segments.last().is_some_and(|s| s.ident == "Hooks")
            {
                for it in i.items {
                    if let syn::ImplItem::Fn(f) = it {
                        answered.insert(f.sig.ident.to_string());
                    }
                }
            }
        }
        let named: std::collections::BTreeSet<String> = super::LUA_HOOKS
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(answered, named);
    }

    #[test]
    fn the_api_number_is_the_last_one_named() {
        assert_eq!(super::api_of("A thing. API 40. Later, API 57."), Some(57));
        assert_eq!(super::api_of("No number here"), None);
    }

    #[test]
    fn the_reference_is_what_the_sources_say() {
        super::run(true).unwrap();
    }
}
