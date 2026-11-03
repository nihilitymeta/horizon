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
        "`mob_hurt(mob, damage_type, causing, direct, amount)`, by kind; `player_hurt(player, ...)` for a player",
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
    ("player_tick", "`player_tick(player)`"),
    (
        "living_fall",
        "`mob_fall(mob, distance, multiplier)`, by kind; `player_fall(player, distance, multiplier)`",
    ),
    ("living_heal", "`mob_heal(mob, amount)`"),
    (
        "living_knockback",
        "`mob_knockback(mob, damage_type, causing, direct, strength, x, z)`, by kind; `player_knockback(...)`",
    ),
    ("player_command", "`player_command(player, line)`"),
    (
        "item_crafted",
        "`item_crafted(player, recipe, item)`, by item",
    ),
    (
        "item_dropped",
        "`item_dropped(player, item, count)`, by item",
    ),
    (
        "loot_drops",
        "`loot_drops(source, drops)`, by block or by kind",
    ),
    (
        "hears_projectile_impacts",
        "true while a script watches `projectile_impact`",
    ),
    (
        "projectile_impact",
        "`projectile_impact(projectile, kind, owner, hit)`, by projectile kind",
    ),
    (
        "projectile_launch",
        "`projectile_launched(projectile, kind, player)`, by projectile kind",
    ),
    (
        "explosion_detonate",
        "`explosion(dimension, center, blocks)`",
    ),
    (
        "player_changed_dimension",
        "`player_changed_dimension(player, from, to)`",
    ),
    ("player_bed_enter", "`player_bed_enter(player, x, y, z)`"),
    ("player_exp_change", "`player_exp_change(player, amount)`"),
    (
        "player_container_open",
        "`player_container_open(player, menu, pos)`",
    ),
    (
        "living_equipment_change",
        "`mob_equipped(mob, slot, from, to)`, by kind",
    ),
    (
        "entity_join_level",
        "`entity_joined(dimension, id, kind, x, y, z, loaded)`, by kind",
    ),
    (
        "hurt_enemy",
        "`item_hurt_enemy(item, player, target)`, by item",
    ),
    (
        "living_entity_use_item_start",
        "`item_use_start(item, player, hand, duration)`, by item",
    ),
];

/// The host helpers the Lua runtime calls, and the `horizon.*` function or
/// hook argument each stands behind (`docs/lua-bindings.md`).
const LUA_HELPERS: &[(&str, &str)] = &[
    (
        "acting_player",
        "`player` in the hooks of a player's action",
    ),
    (
        "attachments",
        "`data` on a mob, `horizon.world_data()`, `horizon.player_data(player)`",
    ),
    ("block", "`horizon.block(x, y, z)`"),
    ("block_property", "`horizon.block_property(x, y, z, key)`"),
    ("broadcast", "`horizon.broadcast(text)`"),
    (
        "carried_stack",
        "`horizon.inventory(player)` for the acting player, `horizon.set_count(n)` in `item_tick`",
    ),
    (
        "config_dir",
        "`horizon.config()`, `horizon.save_config(t)` (`config/<id>.json`, read by the runtime itself)",
    ),
    ("data", "`horizon.data(path)`"),
    ("day_time", "`horizon.time().day_time`"),
    (
        "dimension",
        "`dimension` in the hooks, `horizon.time().dimension`",
    ),
    (
        "edit_blocks",
        "`horizon.fill(x1, y1, z1, x2, y2, z2, id[, props])`",
    ),
    ("feedback", "`horizon.feedback(text)`"),
    (
        "give_experience",
        "`horizon.give_experience(player, points)`",
    ),
    ("give_items", "`horizon.give_items(id, count)`"),
    ("give_levels", "`horizon.give_levels(player, levels)`"),
    ("give_stack", "`horizon.give_items(player, id, count)`"),
    ("held", "`held` in the hooks, `player.held`"),
    ("kill_mob", "`horizon.kill_mob(dimension, id)`"),
    (
        "level_players",
        "`horizon.players()`, `horizon.player(who)`, and a player named to any helper",
    ),
    ("light", "`horizon.light(x, y, z)`"),
    (
        "mob_effect",
        "`horizon.effect(id, name, ticks[, amplifier])`",
    ),
    ("mob_view", "`horizon.mob(id)`, `mob` in the mob hooks"),
    ("mob_views", "`horizon.mobs()`"),
    ("mod_block_entity_data", "`data` in the block hooks"),
    (
        "particles",
        "`horizon.particles(name, x, y, z[, count, spread, speed])`",
    ),
    (
        "play_sound",
        "`horizon.sound(name, x, y, z[, volume, pitch])`",
    ),
    (
        "player_effect",
        "`horizon.effect(player, name, ticks[, amplifier])`",
    ),
    ("player_raycast", "`horizon.raycast(player, distance)`"),
    (
        "player_stacks",
        "`horizon.inventory(player)`, `horizon.count_items(player, id)`, `horizon.carried_items(player)`",
    ),
    ("push_mob", "`horizon.push_mob(dimension, id, x, y, z)`"),
    ("remove_mob", "`horizon.remove_mob(dimension, id)`"),
    ("run_command", "`horizon.run_command(line)`"),
    (
        "set_block",
        "`horizon.set_block(x, y, z, id[, props][, flags])`",
    ),
    (
        "set_block_property",
        "`horizon.set_block_property(key, value[, flags])`",
    ),
    (
        "set_mob_health",
        "`horizon.set_mob_health(dimension, id, health)`",
    ),
    (
        "set_mod_block_entity_data",
        "`data` written back after a block hook",
    ),
    ("set_nickname", "`horizon.set_nickname(player[, name])`"),
    (
        "spawn_falling_block",
        "`horizon.falling_block(x, y, z, id[, vx, vy, vz])`",
    ),
    (
        "state_named",
        "the block id and properties of `horizon.set_block` and `horizon.falling_block`",
    ),
    ("state_name", "`source.block` of `loot_drops`"),
    (
        "summon",
        "`horizon.summon(dimension, kind, x, y, z)`, `horizon.drop_item(x, y, z, id[, count])`",
    ),
    (
        "surface_heights",
        "`horizon.surface_height(x, z[, heightmap])`",
    ),
    ("take_from", "`horizon.take_items(player, id, count)`"),
    ("take_items", "`horizon.take_items(id, count)`"),
    (
        "teleport_mob",
        "`horizon.teleport_mob(dimension, id, x, y, z)`",
    ),
    ("tell", "`horizon.tell(player, text)`"),
    (
        "title",
        "`horizon.title(player, text)`, `horizon.subtitle`, `horizon.action_bar`",
    ),
    (
        "title_times",
        "`horizon.title_times(player, fade_in, stay, fade_out)`",
    ),
    ("weather", "`horizon.time().rain` and `.thunder`"),
];

/// The helpers `LUA_HELPERS` names that the runtime reaches without a call
/// on the host: the attachments through the attachment types, the config
/// folder computed as the host does.
#[cfg(test)]
const LUA_HELPERS_INDIRECT: &[&str] = &["attachments", "config_dir"];

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
    let lua_helpers: BTreeMap<&str, &str> = LUA_HELPERS.iter().copied().collect();
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
    out.push_str("\n## Host helpers\n\n| Helper | API | Lua | What |\n|---|---|---|---|\n");
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
        let lua = lua_helpers.get(name).copied().unwrap_or("");
        out.push_str(&format!("| `{name}` | {api} | {lua} | {} |\n", cell(&text)));
    }
    out.push_str(&format!(
        "\n{count} hook points, {} helpers; {} of the hooks and {} of the helpers reaching Lua.\n",
        helpers.len(),
        hooks
            .iter()
            .filter(|h| h["name"].as_str().is_some_and(|n| lua.contains_key(n)))
            .count(),
        helpers
            .iter()
            .filter(|h| h["name"]
                .as_str()
                .is_some_and(|n| lua_helpers.contains_key(n)))
            .count()
    ));
    for (name, _) in LUA_HOOKS {
        if !hooks.iter().any(|h| h["name"].as_str() == Some(name)) {
            return Err(format!("LUA_HOOKS names {name}, which is not a hook point"));
        }
    }
    for (name, _) in LUA_HELPERS {
        if !helpers.iter().any(|h| h["name"].as_str() == Some(name)) {
            return Err(format!("LUA_HELPERS names {name}, which is not a helper"));
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

    /// The host helpers the Lua runtime calls (`host.name(...)` or
    /// `h.name(...)` in `lua_mods.rs`, outside its tests) are the ones the
    /// reference says stand behind a `horizon.*` function, no more and no
    /// fewer (beside the two reached without a call).
    #[test]
    fn the_helpers_the_runtime_calls_are_the_ones_the_reference_names() {
        use syn::visit::Visit;
        struct Calls(std::collections::BTreeSet<String>);
        impl<'ast> Visit<'ast> for Calls {
            fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
                if let syn::Expr::Path(p) = &*call.receiver
                    && p.path.is_ident("host") | p.path.is_ident("h")
                {
                    self.0.insert(call.method.to_string());
                }
                syn::visit::visit_expr_method_call(self, call);
            }
            fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
                if m.ident != "tests" {
                    syn::visit::visit_item_mod(self, m);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let src =
            std::fs::read_to_string(root.join("crates/horizon-client/src/lua_mods.rs")).unwrap();
        let file = syn::parse_file(&src).unwrap();
        let mut calls = Calls(Default::default());
        calls.visit_file(&file);
        let catalog: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("native-hooks.json")).unwrap())
                .unwrap();
        let helpers: std::collections::BTreeSet<String> = catalog["helpers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["name"].as_str().unwrap().to_owned())
            .collect();
        let mut called: std::collections::BTreeSet<String> =
            calls.0.intersection(&helpers).cloned().collect();
        called.extend(super::LUA_HELPERS_INDIRECT.iter().map(|s| s.to_string()));
        let named: std::collections::BTreeSet<String> = super::LUA_HELPERS
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(called, named);
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
