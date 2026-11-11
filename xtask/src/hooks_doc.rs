//! `cargo run -p xtask -- hooks-doc`: `docs/api/hook-points.md`, the reference
//! of every hook point and host helper in `native-hooks.json`, with the Lua
//! handler each hook reaches, from the catalog and the `native_*` functions'
//! own docs, so the list never trails the code. `--check` fails when the file
//! is not what the sources say.
use super::lua_model::{
    Class, Conv, Field, Host, Kind, Model, classify, declaration_served, event_keys, listened_flag,
};
use horizon_mod_api_gen::{
    catalog_hooks, doc_comments, free_function_source, impl_functions, one_function,
};
use std::{collections::BTreeMap, fs, path::Path};
use syn::Item;

/// Hooks whose Lua cell stays exactly this curated text: the two
/// declaration-served hooks (served from the `horizon.block_entity`
/// declarations: they build no event table and take no `horizon.on`
/// registration) and the three listened game queries (no event table of
/// their own: the game answers them from their siblings' registries, so a
/// script never registers for them).
const LUA_VERBATIM: &[(&str, &str)] = &[
    (
        "block_entity",
        "`horizon.block_entity(block, { ticks, data })`: declaration-served, no `horizon.on` registration",
    ),
    (
        "block_entity_ticks",
        "`horizon.block_entity(block, { ticks = true })`: declaration-served, no `horizon.on` registration",
    ),
    (
        "entity_tick_listened",
        "answered while an `entity_tick_pre` or `entity_tick_post` handler exists",
    ),
    (
        "living_equipment_listened",
        "true while a `living_equipment_change` handler exists",
    ),
    (
        "hears_projectile_impacts",
        "true while a script watches `projectile_impact`",
    ),
];

/// Extra sentences appended after a generated Lua cell, where the curated
/// table knew something the model does not spell out: the queued actions a
/// hostless hook runs, the reads beside a hook, and the
/// always-present-field notes. Everything else the curated row said now
/// comes from the model itself.
const LUA_NOTES: &[(&str, &str)] = &[
    (
        "data_pack_files",
        "appended to `event.files` in load order; reloads the scripts on `/reload`",
    ),
    ("random_tick", "`data` is not a native field"),
    ("tick_block", "`data` is not a native field"),
    (
        "player_logged_in",
        "the full table while they are on; `uuid` and the told `name` otherwise",
    ),
    (
        "player_logged_out",
        "the full table while they are on; `uuid` and the told `name` otherwise",
    ),
    (
        "player_respawned",
        "the full table while they are on; `uuid` and the told `name` otherwise",
    ),
    ("player_died", "`cause` is the death message's key, or nil"),
    (
        "register_commands",
        "declare commands with `horizon.command(usage, level, fn)`",
    ),
    (
        "player_tick",
        "every player every tick; `horizon.players()` is the whole list and `horizon.acting_player()` is the ticking player",
    ),
    ("player_bed_enter", "`pos` is the bed head, always present"),
    (
        "player_container_open",
        "`pos` is the block's `{x, y, z}`, nil for no block",
    ),
    (
        "player_render_listened",
        "a query hook, not an answer: the game poses no player or mob for the render hooks unless a handler sets `event.listened`",
    ),
    (
        "experience_orb_pickup_listened",
        "a query hook, not an answer: the pickup never runs unless a handler sets `event.listened`",
    ),
    (
        "living_equipment_change",
        "also fires for players, kind `minecraft:player`; `native_on_equip` is the other hook",
    ),
    (
        "right_click_block",
        "`horizon.cancel()` takes the click for that hand: the other hand still gets its turn",
    ),
    ("command", "drops the line on cancel"),
    (
        "projectile_impact",
        "branch on `event.entity`: on an entity hit `block` and `face` are still `{0, 0, 0}`, and on a block hit `entity` is nil and `entity_kind` is `\"\"`",
    ),
    ("projectile_launch", "`player` is the table, or nil"),
];

/// Join backticked names the curated cells' way: `a`, `b` and `c`.
fn ticked(names: &[String]) -> String {
    let names: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => format!(
            "{} and {}",
            names[..names.len() - 1].join(", "),
            names[names.len() - 1]
        ),
    }
}

/// Whether the script can change an event-table field: the `write_tag` rule
/// of `lua_hooks.rs` (which `=>` markers the emitted arms carry), so the
/// reference and the runtime cannot drift. The level-tick `data` and the
/// block-entity tick's entity table are written back by their bespoke arms
/// (which carry no tag), so they count as writable too.
fn writable(conv: &Conv, ty: &str, key: Option<&str>) -> bool {
    super::lua_hooks::write_tag(conv, ty, key).is_some()
        || matches!(conv, Conv::LevelData | Conv::BlockEntity)
}

/// The event-table keys of one model field: the same keys the `horizon.d.lua`
/// event types derive from (flat seeds write their table keys, and every
/// text field reads alongside its `{name}_component` sibling).
fn field_keys(f: &Field) -> Vec<String> {
    event_keys(f).into_iter().map(|(name, _)| name).collect()
}

/// The event-table keys a cell's field list names: every model field's keys
/// (flat seeds write their table keys, text fields add their `_component`
/// sibling), then the extras' and the answer field's names.
fn cell_keys(model: &Model) -> Vec<String> {
    let mut keys: Vec<String> = model.fields.iter().flat_map(field_keys).collect();
    for extra in &model.extras {
        keys.push(extra.name.clone());
    }
    if let Some(out) = &model.out {
        keys.push(out.name.clone());
    }
    keys
}

/// The Lua cell of one generated hook from the model: `an event table with
/// <fields>; kind: <domain>|none; <how a script cancels or changes it>`.
/// `perform_command` answers `(player, args)` instead of an event table, so
/// it names that shape; the frame hooks draw rather than hear.
fn lua_cell(model: &Model) -> String {
    if model.name == "perform_command" {
        return "calls `fn(player, args)` of `horizon.command`; kind: command words, the longest usage first; a matched usage always cancels, even when argument parsing refuses or the handler errors".to_owned();
    }
    let keys = cell_keys(model);
    let mut cell = if keys.is_empty() {
        "an empty event table".to_owned()
    } else {
        format!("an event table with {}", ticked(&keys))
    };
    cell.push_str(&match &model.kind {
        Kind::None => "; kind: none".to_owned(),
        Kind::Field => "; kind: entity kind (`kind`)".to_owned(),
        Kind::Entity { field, opt } => {
            let mut s = format!("; kind: kind of `{field}`");
            if *opt {
                // An `opt_entity` id resolves to no kind when it is nil, so
                // only bare handlers run. The use-item hooks' `entity` is a
                // mob's id, nil for the player's own use: a kind registration
                // hears mob uses, never the player.
                if model.name.starts_with("living_entity_use_item_") {
                    s.push_str(
                        "; mob uses filter by kind; the player's own use reaches bare handlers",
                    );
                } else {
                    s.push_str(&format!("; a nil `{field}` reaches bare handlers"));
                }
            }
            s
        }
        Kind::Loot => "; kind: loot block or entity kind".to_owned(),
        Kind::Block { field } => format!("; kind: block (`{field}`)"),
        Kind::BlockEntity { field } => format!("; kind: block-entity id (`{field}`)"),
        Kind::Mob { field } => format!("; kind: mob kind (`{field}`)"),
    });
    // A uuid-text field reads as its `8-4-4-4-12` text, not a player table:
    // name it on every hook, so no hook can lose the fact again.
    for f in &model.fields {
        if matches!(f.conv, Conv::Uuid) {
            cell.push_str(&format!("; `{}` is uuid text", f.name));
        } else if matches!(f.conv, Conv::OptUuid) {
            cell.push_str(&format!("; `{}` is uuid text, or nil", f.name));
        }
    }
    // What the handler may set, in the model's own terms.
    let mut powers: Vec<String> = Vec::new();
    if let Some(out) = &model.out {
        powers.push(format!(
            "set `event.{}` to answer (nil leaves it to the next handler or vanilla)",
            out.name
        ));
    }
    if model.contract == "collect" {
        let edits = edits_of(model);
        if !edits.is_empty() {
            powers.push(format!(
                "edit {} in turn; every handler runs",
                ticked(&edits)
            ));
        }
    } else if !matches!(model.kind, Kind::Mob { .. }) {
        let sets = sets_of(model);
        if !sets.is_empty() {
            powers.push(format!("set {}", ticked(&sets)));
        }
    }
    if matches!(model.kind, Kind::Mob { .. }) {
        powers.push(
            "queue the goal/body actions (`add_goal`, `add_target_goal`, `clear_goals`, `clear_target_goals`, `set_body_rotation`); no host, so other actions are dropped"
                .to_owned(),
        );
    }
    if model.contract == "cancel" {
        // The level-tick arms answer `handled()`, not `cancel()`: `cancel()`
        // is not the level-tick cancel and the tick keeps running.
        if model.host == Host::LevelWrite {
            powers.push("`horizon.handled()` skips the block's own tick".to_owned());
        } else {
            powers.push("`horizon.cancel()` stops it".to_owned());
        }
    }
    if model.frame {
        let mut calls: Vec<String> = Vec::new();
        for f in &model.fields {
            if matches!(f.conv, Conv::HudBatch | Conv::Corners) {
                calls.push(format!("`{}`", f.name));
            }
        }
        let calls = match calls.len() {
            0 => String::new(),
            1 => calls[0].clone(),
            _ => format!(
                "{} and {}",
                calls[..calls.len() - 1].join(", "),
                calls[calls.len() - 1]
            ),
        };
        powers.push(format!(
            "draws under the frame budget; dot-call {calls}, the tables expire with the call"
        ));
    }
    if powers.is_empty() {
        powers.push("it only hears".to_owned());
    }
    cell.push_str("; ");
    cell.push_str(&powers.join("; "));
    // What the host does with the queued calls.
    match model.host {
        Host::GameMut | Host::LevelWrite => {}
        Host::None if matches!(model.kind, Kind::Mob { .. }) => {}
        Host::None => cell.push_str("; no host: queued actions are dropped"),
        Host::GameRef | Host::Level | Host::LevelMut | Host::LevelTicks => {
            cell.push_str("; reads only: queued actions are dropped")
        }
    }
    if model.host == Host::LevelWrite {
        cell.push_str(
            "; the queued level writes run (`set_block_property`, `schedule_tick`, `summon`)",
        );
    }
    cell
}

/// The `event.<field>` paths the handler may set, for the change clause: the
/// pre-login gate answers through `<field>.refuse`, and the block-entity
/// tick's entity table through `entity.data`.
fn sets_of(model: &Model) -> Vec<String> {
    let mut sets = Vec::new();
    for f in &model.fields {
        let ty = model
            .dispatch_params
            .iter()
            .find(|p| p.name == f.name)
            .map(|p| p.ty.as_str())
            .unwrap_or("");
        if !writable(&f.conv, ty, f.key.as_deref()) {
            continue;
        }
        if matches!(f.conv, Conv::PreLogin) {
            sets.push(format!("event.{}.refuse", f.name));
        } else if matches!(f.conv, Conv::BlockEntity) {
            sets.push("event.entity.data".to_owned());
        } else {
            sets.push(format!("event.{}", f.name));
        }
        // A text write is the string, the component table, or both: name
        // the sibling too.
        if matches!(f.conv, Conv::TextVal | Conv::TextOpt) {
            sets.push(format!("event.{}_component", f.name));
        }
    }
    for extra in &model.extras {
        // The level-tick `data` is written back with the before-template
        // rule; the other extras are seeded notes.
        if matches!(extra.conv, Conv::LevelData) {
            sets.push(format!("event.{}", extra.name));
        }
    }
    sets
}

/// The `event.<field>` paths a collect hook's handlers edit in turn.
fn edits_of(model: &Model) -> Vec<String> {
    sets_of(model)
}

/// One hook's Lua cell and What text: the generated cell (plus its note,
/// if any) for an event table, the curated text for a declaration or a
/// listened query, and an empty cell with the exposure reason beside the
/// native doc for a hook that does not reach Lua.
fn lua_row(
    name: &str,
    class: &Class,
    verbatim: &BTreeMap<&str, &str>,
    notes: &BTreeMap<&str, &str>,
    text: String,
) -> Result<(String, String), String> {
    match class {
        Class::Generated(model) => {
            if declaration_served(name) || listened_flag(name).is_some() {
                let cell = verbatim.get(name).copied().ok_or_else(|| {
                    format!("{name} takes no script handlers but LUA_VERBATIM names no text for it")
                })?;
                Ok((cell.to_owned(), text))
            } else {
                if verbatim.contains_key(name) {
                    return Err(format!(
                        "{name} is a generated event table but LUA_VERBATIM covers it"
                    ));
                }
                let mut cell = lua_cell(model);
                if let Some(note) = notes.get(name) {
                    cell.push_str("; ");
                    cell.push_str(note);
                }
                Ok((cell, text))
            }
        }
        other => {
            let text = format!("{} Not in Lua: {}.", text, exposure_of(name, other));
            Ok((String::new(), text))
        }
    }
}

/// The exposure reason of a hook that does not reach Lua, in the short
/// phrase of `docs/api/hook-exposure.md` (which the `lua-hooks` generator
/// owns): the borrow, the missing converter, the live handle, the raw
/// packets, or the level-tick host.
fn exposure_of(name: &str, class: &Class) -> String {
    let strip = |reason: &str, kind: &str| {
        reason
            .strip_prefix(&format!("{name}: {kind}: "))
            .unwrap_or(reason)
            .to_owned()
    };
    match class {
        Class::Generated(_) => String::new(),
        Class::Adapter(whys) => {
            let mut reasons: Vec<String> = whys.iter().map(|w| strip(w, "adapter")).collect();
            reasons.sort();
            reasons.dedup();
            reasons.join("; ")
        }
        Class::Tick => "a `ticks` parameter (level-tick host)".to_owned(),
        Class::Cannot(why) => why
            .split("; ")
            .map(|w| strip(w, "unsupported"))
            .collect::<Vec<_>>()
            .join("; "),
        Class::Handle(ty) => format!("a {ty} handle needs its own design"),
        Class::Protocol => "raw packets need their own design".to_owned(),
    }
}

/// The host helpers the Lua runtime calls, and the `horizon.*` function or
/// hook argument each stands behind (`docs/lua-bindings.md`).
const LUA_HELPERS: &[(&str, &str)] = &[
    (
        "acting_player",
        "`horizon.acting_player()`, and `player` in the hooks of a player's action",
    ),
    (
        "attachments",
        "`horizon.world_data()`, `horizon.player_data(player)`, `horizon.entity_data(id)` (an entity id the client copy never saw still reads as an empty table: a table is no proof the entity exists on a client)",
    ),
    ("block", "`horizon.block(x, y, z)`"),
    ("block_property", "`horizon.block_property(x, y, z, key)`"),
    ("broadcast", "`horizon.broadcast(text)`"),
    (
        "carried_stack",
        "the acting player's own slots behind `horizon.inventory(player)`",
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
    (
        "has_permission",
        "`horizon.has_permission(player, node, default_level)`",
    ),
    (
        "held",
        "`player.held` in the command handler's player table (`perform_command`), in the `{item, count, plain}` hand shape",
    ),
    (
        "held_stack",
        "`held` on the `horizon.player(s)` tables and `horizon.acting_player()`, as a full `Item` table",
    ),
    ("kick", "`horizon.kick(player, reason)`"),
    ("kill_mob", "`horizon.kill_mob(dimension, id)`"),
    (
        "level_players",
        "`horizon.players()`, `horizon.player(who)`, and a player named to any helper",
    ),
    ("light", "`horizon.light(x, y, z)`"),
    (
        "open_menu",
        "`horizon.open_menu(player, rows, title, items)`",
    ),
    (
        "open_menu_with_input",
        "`horizon.open_menu_with_input(player, rows, title, items, input)`",
    ),
    ("set_menu_items", "`horizon.set_menu_items(player, items)`"),
    ("close_menu", "`horizon.close_menu(player)`"),
    ("menu_contents", "`horizon.menu_contents(player)`"),
    ("menu_cursor", "`horizon.menu_cursor(player)`"),
    (
        "mob_effect",
        "`horizon.effect(id, name, ticks[, amplifier])`",
    ),
    ("mob_view", "`horizon.mob(id)`"),
    ("mob_views", "`horizon.mobs()`"),
    ("mod_block_entity_data", "`data` in the block hooks"),
    (
        "net_send_to_player",
        "`horizon.net_send_to_player(player, channel, data)`",
    ),
    (
        "net_send_to_server",
        "`horizon.net_send_to_server(channel, data)`",
    ),
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
    ("player_permission", "`horizon.player_permission(player)`"),
    ("player_raycast", "`horizon.raycast(player, distance)`"),
    (
        "player_stacks",
        "`horizon.inventory(player)`, `horizon.count_items(player, id)`, `horizon.carried_items(player)`",
    ),
    ("push_mob", "`horizon.push_mob(dimension, id, x, y, z)`"),
    ("remove_mob", "`horizon.remove_mob(dimension, id)`"),
    ("run_command", "`horizon.run_command(line)`"),
    (
        "send_custom_payload_to_player",
        "`horizon.send_custom_payload_to_player(player, channel, data)`",
    ),
    (
        "set_block",
        "`horizon.set_block(x, y, z, id[, props][, flags])`",
    ),
    (
        "set_block_property",
        "`horizon.set_block_property(key, value[, flags])` in the level hooks",
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
    ("state_name", "`event.block` of `loot_drops`"),
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
    (
        "teleport_player",
        "`horizon.teleport_player(player, dimension, pos, yaw, pitch)`",
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

/// The helpers `LUA_HELPERS` names that the runtime reaches without a textual
/// call on the host in the scanned files: the attachments through the
/// attachment types, the config folder computed as the host does, and the
/// block-hook helpers the generated level arms call through the level host
/// (the visitor only sees the `host` and `h` receivers).
#[cfg(test)]
const LUA_HELPERS_INDIRECT: &[&str] = &[
    "attachments",
    "config_dir",
    "mod_block_entity_data",
    "set_block_property",
    "set_mod_block_entity_data",
];

/// The helpers listed by the pending hook fragments, in file order.
fn fragment_helpers(root: &Path) -> Result<Vec<serde_json::Value>, String> {
    let dir = root.join("tools/native-hook-fragments");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let mut helpers = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let fragment: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(list) = fragment["helpers"].as_array() {
            helpers.extend(list.iter().cloned());
        }
    }
    Ok(helpers)
}

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
    // The function a target names: `native_<name>` mostly, but a fragment may name it otherwise
    // (`destroy_block` is `native_machine_destroy_block` in machine_breaking.json).
    fn target_fn(target: &str) -> &str {
        target.rsplit("::").next().unwrap_or(target)
    }
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
    let verbatim: BTreeMap<&str, &str> = LUA_VERBATIM.iter().copied().collect();
    let notes: BTreeMap<&str, &str> = LUA_NOTES.iter().copied().collect();
    let lua_helpers: BTreeMap<&str, &str> = LUA_HELPERS.iter().copied().collect();
    // Every hook in exactly one class, from the same model that emits the
    // runtime and `horizon.d.lua`: the Lua cell of a generated hook comes
    // from its model, and a hook of any other class keeps an empty cell
    // with its exposure reason beside the native doc.
    let classes = classify(root)?;
    let class_of = |name: &str| {
        classes.iter().find(|row| row.name == name).ok_or_else(|| {
            format!("{name} is a hook point but the classifier names no class for it")
        })
    };
    let mut out = String::new();
    out.push_str("<!-- Generated by `cargo run -p xtask -- hooks-doc` from native-hooks.json and the native_* functions' docs: edit those, then regenerate. -->\n");
    out.push_str("# Hook points and host helpers\n\n");
    out.push_str("Every hook point a native mod (`docs/native-mods.md`) or a trait mod (`crates/horizon-client/src/mod_api.rs`) may listen to, \
with the API level it arrived in, how it is taken (`listen`: it only hears; `cancel`: a mod may stop it; `collect`: every mod edits a value in turn; `replace`: the first answer wins), \
and the handler a scripted mod (`docs/lua-bindings.md`) registers for it, where one exists. Then every helper the host offers them. \
A hook with an empty Lua cell does not reach Lua: its row says why (`docs/api/hook-exposure.md` lists every such hook with its reason). \
The full doc of each is on its `native_<name>` function.\n\n");
    out.push_str(
        "## Hook points\n\n| Hook | Side | API | Taken | Lua | What |\n|---|---|---|---|---|---|\n",
    );
    let mut hooks = catalog_hooks(root, &catalog)?;
    hooks.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut count = 0;
    let mut lua_cells = 0;
    for hook in &hooks {
        let name = hook["name"].as_str().ok_or("hook name missing")?;
        let target = hook["target"].as_str().ok_or("target missing")?;
        let contract = hook["mod_api"]["contract"].as_str().unwrap_or("listen");
        let text = docs_of(target, target_fn(target), &mut parsed)?;
        let api = api_of(&text).map_or("—".to_owned(), |n| n.to_string());
        let class = class_of(name)?;
        let (lua, text) = lua_row(name, &class.class, &verbatim, &notes, text)?;
        if !lua.is_empty() {
            lua_cells += 1;
        }
        // `classify` already refused a hook with no side tag.
        let side = hook["side"].as_str().unwrap_or("—");
        out.push_str(&format!(
            "| `{name}` | {side} | {api} | {contract} | {} | {} |\n",
            cell(&lua),
            cell(&text)
        ));
        count += 1;
    }
    out.push_str("\n## Host helpers\n\n| Helper | API | Lua | What |\n|---|---|---|---|\n");
    let mut helpers = catalog["helpers"]
        .as_array()
        .ok_or("catalog helpers must be an array")?
        .clone();
    // Helpers in pending fragments (tools/native-hook-fragments/) are the host's too, with no API
    // number yet, as `catalog_hooks` adds pending hooks.
    helpers.extend(fragment_helpers(root)?);
    helpers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    for helper in &helpers {
        let name = helper["name"].as_str().ok_or("helper name missing")?;
        let target = helper["target"].as_str().ok_or("helper target missing")?;
        let text = docs_of(target, target_fn(target), &mut parsed)?;
        let api = api_of(&text).map_or("—".to_owned(), |n| n.to_string());
        let lua = lua_helpers.get(name).copied().unwrap_or("");
        out.push_str(&format!("| `{name}` | {api} | {lua} | {} |\n", cell(&text)));
    }
    out.push_str(&format!(
        "\n{count} hook points, {} helpers; {lua_cells} of the hooks and {} of the helpers reaching Lua.\n",
        helpers.len(),
        helpers
            .iter()
            .filter(|h| h["name"]
                .as_str()
                .is_some_and(|n| lua_helpers.contains_key(n)))
            .count()
    ));
    for (name, _) in LUA_VERBATIM.iter().chain(LUA_NOTES.iter()) {
        if !hooks.iter().any(|h| h["name"].as_str() == Some(name)) {
            return Err(format!(
                "the Lua cell table names {name}, which is not a hook point"
            ));
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
    /// Every generated hook has a non-empty Lua cell, and every hook of
    /// another class has none: the reference never forgets a hook the
    /// runtime reaches, and never invents one it does not.
    #[test]
    fn every_generated_hook_has_a_lua_cell_and_no_other_hook_does() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let classes = super::classify(&root).unwrap();
        let verbatim: std::collections::BTreeMap<&str, &str> =
            super::LUA_VERBATIM.iter().copied().collect();
        let notes: std::collections::BTreeMap<&str, &str> =
            super::LUA_NOTES.iter().copied().collect();
        let mut generated = 0;
        let mut other = 0;
        for row in &classes {
            let (lua, _) = super::lua_row(
                row.name.as_str(),
                &row.class,
                &verbatim,
                &notes,
                String::new(),
            )
            .unwrap();
            match &row.class {
                super::Class::Generated(_) => {
                    assert!(
                        !lua.is_empty(),
                        "{} reaches Lua but its cell is empty",
                        row.name
                    );
                    generated += 1;
                }
                _ => {
                    assert!(
                        lua.is_empty(),
                        "{} does not reach Lua but has a cell",
                        row.name
                    );
                    other += 1;
                }
            }
        }
        assert!(
            generated > 400,
            "only {generated} generated hooks; the coverage check is vacuous"
        );
        assert!(
            other > 50,
            "only {other} non-generated hooks; the coverage check is vacuous"
        );
    }

    /// Every hook that does not reach Lua carries its exposure reason: the
    /// reader can tell "not exposed" from "forgot to document".
    #[test]
    fn non_generated_hooks_carry_their_exposure_reason() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let classes = super::classify(&root).unwrap();
        for row in &classes {
            if matches!(&row.class, super::Class::Generated(_)) {
                continue;
            }
            let reason = super::exposure_of(row.name.as_str(), &row.class);
            assert!(!reason.is_empty(), "{} has no exposure reason", row.name);
        }
        let reason_of = |name: &str| {
            let row = classes.iter().find(|row| row.name == name).unwrap();
            super::exposure_of(name, &row.class)
        };
        assert!(
            reason_of("screen_draw").contains("render state"),
            "draw refusal names render state"
        );
        assert!(
            reason_of("server_packet").contains("raw packets"),
            "packet refusal names raw packets"
        );
        assert!(
            reason_of("update_shape").contains("level-tick host"),
            "tick refusal names the level-tick host"
        );
        assert!(
            reason_of("menu_button").contains("handle"),
            "handle refusal names the handle"
        );
        assert!(
            !reason_of("neighbor_changed").is_empty(),
            "adapter refusal is non-empty"
        );
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
        let mut calls = Calls(Default::default());
        // The runtime's host calls live in the hand-written hooks and in
        // the generated runtime's helpers (whose macros expand into the
        // hand-written `impl` block); both files count.
        for relative in [
            "crates/event-horizon-lua/src/lua_mods.rs",
            "crates/event-horizon-lua/src/generated_runtime.rs",
        ] {
            let src = std::fs::read_to_string(root.join(relative)).unwrap();
            let file = syn::parse_file(&src).unwrap();
            calls.visit_file(&file);
        }
        let catalog: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("native-hooks.json")).unwrap())
                .unwrap();
        let helpers: std::collections::BTreeSet<String> = catalog["helpers"]
            .as_array()
            .unwrap()
            .iter()
            .chain(&super::fragment_helpers(&root).unwrap())
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

    /// Every hook with a kind names its kind token's domain phrase in its
    /// cell (`perform_command` its command words), and every kind-less hook
    /// says none. The phrases live in the test's words, not the renderer's,
    /// so a changed phrase fails here.
    #[test]
    fn a_hook_with_a_kind_says_so() {
        // `perform_command` answers `(player, args)`, so it is no event
        // table, but its cell still names its command words.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let classes = super::classify(&root).unwrap();
        let row = classes
            .iter()
            .find(|r| r.name == "perform_command")
            .unwrap();
        let super::Class::Generated(model) = &row.class else {
            panic!("perform_command should generate");
        };
        assert!(
            super::lua_cell(model).contains("kind: command words"),
            "perform_command names its command words"
        );
        for (name, model, cell) in event_tables() {
            let domain: String = match &model.kind {
                super::Kind::None => "kind: none".to_owned(),
                super::Kind::Field => "kind: entity kind (`kind`)".to_owned(),
                super::Kind::Entity { field, .. } => format!("kind: kind of `{field}`"),
                super::Kind::Loot => "kind: loot block or entity kind".to_owned(),
                super::Kind::Block { field } => format!("kind: block (`{field}`)"),
                super::Kind::BlockEntity { field } => {
                    format!("kind: block-entity id (`{field}`)")
                }
                super::Kind::Mob { field } => format!("kind: mob kind (`{field}`)"),
            };
            assert!(cell.contains(&domain), "{name} names its kind domain");
            if !matches!(model.kind, super::Kind::None) {
                assert!(!cell.contains("kind: none"), "{name} hides its kind");
            }
        }
    }

    /// Every hook with a generated event-table cell: its name, model and
    /// cell. The declaration-served hooks and listened queries keep verbatim
    /// cells (no event table), and `perform_command` answers `(player, args)`.
    fn event_tables() -> Vec<(String, super::Model, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let classes = super::classify(&root).unwrap();
        let mut out = Vec::new();
        for row in &classes {
            let super::Class::Generated(model) = &row.class else {
                continue;
            };
            if super::declaration_served(row.name.as_str())
                || super::listened_flag(row.name.as_str()).is_some()
                || row.name == "perform_command"
            {
                continue;
            }
            out.push((row.name.clone(), model.clone(), super::lua_cell(model)));
        }
        assert!(
            out.len() > 400,
            "only {} event tables; the per-hook checks are vacuous",
            out.len()
        );
        out
    }

    /// Every text field's `{name}_component` sibling appears in the cell's
    /// field list and in a set clause: the reference cannot disagree with
    /// the event type again.
    #[test]
    fn text_fields_bring_their_component_sibling() {
        let mut checked = 0;
        for (name, model, cell) in event_tables() {
            for f in &model.fields {
                if !matches!(f.conv, super::Conv::TextVal | super::Conv::TextOpt) {
                    continue;
                }
                checked += 1;
                assert!(
                    cell.contains(&format!("`{}_component`", f.name)),
                    "{name} names `{}_component`",
                    f.name
                );
                assert!(
                    super::sets_of(&model)
                        .iter()
                        .any(|s| s == &format!("event.{}_component", f.name)),
                    "{name} lets the script set `event.{}_component`",
                    f.name
                );
            }
        }
        assert!(
            checked > 0,
            "no text field found; the sibling check is vacuous"
        );
    }

    /// Every uuid-text field says so in its cell: a uuid string is not a
    /// player table, and no hook may lose the fact again.
    #[test]
    fn uuid_text_fields_say_so() {
        let mut checked = 0;
        for (name, model, cell) in event_tables() {
            for f in &model.fields {
                if !matches!(f.conv, super::Conv::Uuid | super::Conv::OptUuid) {
                    continue;
                }
                checked += 1;
                assert!(
                    cell.contains(&format!("`{}` is uuid text", f.name)),
                    "{name} says `{}` is uuid text",
                    f.name
                );
            }
        }
        assert!(
            checked > 0,
            "no uuid-text field found; the uuid check is vacuous"
        );
    }

    /// Every generated cell's field list is its model's field list: the
    /// flat keys, the text-component siblings, the extras and the answer
    /// out. The expectation derives from the model layer, never from the
    /// cell renderer, so a dropped field fails here.
    #[test]
    fn every_cell_lists_its_models_fields() {
        for (name, model, cell) in event_tables() {
            let mut expected: Vec<String> = Vec::new();
            for f in &model.fields {
                expected.extend(super::event_keys(f).into_iter().map(|(name, _)| name));
            }
            for extra in &model.extras {
                expected.push(extra.name.clone());
            }
            if let Some(out) = &model.out {
                expected.push(out.name.clone());
            }
            assert_eq!(
                super::cell_keys(&model),
                expected,
                "{name} lists exactly its model's keys"
            );
            for key in &expected {
                assert!(cell.contains(&format!("`{key}`")), "{name} names `{key}`");
            }
        }
    }

    /// Every hook says how a script cancels or changes it: the cancel hooks
    /// name `horizon.cancel()` (the level ticks `horizon.handled()`), every
    /// replace hook names the answer field it seeds nil, and every collect
    /// hook names the field its handlers edit in turn.
    #[test]
    fn every_hook_says_how_a_script_cancels_or_changes_it() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let classes = super::classify(&root).unwrap();
        for row in &classes {
            let super::Class::Generated(model) = &row.class else {
                continue;
            };
            if super::declaration_served(row.name.as_str())
                || super::listened_flag(row.name.as_str()).is_some()
                || row.name == "perform_command"
            {
                continue;
            }
            let cell = super::lua_cell(model);
            if model.contract == "cancel" {
                if model.host == super::Host::LevelWrite {
                    assert!(
                        cell.contains("horizon.handled()"),
                        "{} names handled(), not cancel()",
                        row.name
                    );
                    assert!(
                        !cell.contains("horizon.cancel()"),
                        "{} must not offer cancel()",
                        row.name
                    );
                } else {
                    assert!(
                        cell.contains("horizon.cancel()"),
                        "{} names horizon.cancel()",
                        row.name
                    );
                }
            }
            if let Some(out) = &model.out {
                assert!(
                    cell.contains(&format!("event.{}", out.name)),
                    "{} names its answer field",
                    row.name
                );
            }
            if model.contract == "collect" {
                assert!(
                    cell.contains("in turn"),
                    "{} says its handlers edit in turn",
                    row.name
                );
            }
        }
    }

    /// The curated tables stay exact: every verbatim hook is a declaration
    /// or a listened query (and every one of those has its text), and every
    /// note hangs off a real event-table hook.
    #[test]
    fn the_curated_tables_stay_exact() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let classes = super::classify(&root).unwrap();
        let mut special: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut tables: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for row in &classes {
            if !matches!(&row.class, super::Class::Generated(_)) {
                continue;
            }
            if super::declaration_served(row.name.as_str())
                || super::listened_flag(row.name.as_str()).is_some()
            {
                special.insert(row.name.clone());
            } else {
                tables.insert(row.name.clone());
            }
        }
        let verbatim: std::collections::BTreeSet<String> = super::LUA_VERBATIM
            .iter()
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(
            verbatim, special,
            "verbatim rows are exactly the declarations and listened queries"
        );
        for (name, _) in super::LUA_NOTES {
            assert!(
                tables.contains(*name),
                "note {name} hangs off no event-table hook"
            );
        }
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
