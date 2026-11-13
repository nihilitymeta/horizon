//! `cargo run -p xtask -- lua-helpers [--check]`: the Lua bridge of every
//! native host helper `lua_helper_model` classifies as Generated, from
//! the `native_*` signatures (never hand-written), plus its tests
//! (`crates/event-horizon-lua/src/generated_helpers.rs` and
//! `generated_helpers_tests.rs`) and the exposure doc
//! (`docs/api/helper-exposure.md`). `--check` fails when any of its five
//! outputs is not what the model says.
//!
//! Every Generated helper is emitted except the [`EXCLUDED`] denylist
//! (one entry per deliberately unechoed helper, each with a reason the
//! exposure doc prints); `run` fails if an excluded helper stops
//! classifying as Generated (a stale entry) or if an emitted helper
//! overlaps a hand-written Lua name or a reserved `horizon.*` name.
use super::lua_helper_model::{
    GenKind, HelperClass, HelperRow, adapter_ranking, class_counts, classify_helpers,
    luau_function, unblocked_by_top,
};
use std::{collections::BTreeSet, fs, path::Path};

/// The helpers deliberately NOT emitted despite classifying as
/// Generated: one entry per exclusion, with the reason the exposure
/// doc prints. Every Generated helper not here is ENABLED. `on` is
/// not here: the model classifies a reserved name as `Cannot`, not as
/// excluded (so it never reaches the denylist). Likewise the query
/// helpers (a `&mut self` `bool` whose doc starts "Whether"): the model
/// classifies them `Cannot`, so they are never listed here.
pub const EXCLUDED: &[(&str, &str)] = &[
    // Dev/diagnostic helpers: API tags `lua_console_helpers`/
    // `dev_helpers`, the file `native_diagnostics.rs`, and these names.
    ("count_packets", "dev/diagnostic: not for mod scripts"),
    ("level_dimension", "dev/diagnostic: not for mod scripts"),
    ("mob_cost_sampling", "dev/diagnostic: not for mod scripts"),
    ("set_setting", "dev/diagnostic: not for mod scripts"),
    ("spans", "dev/diagnostic: not for mod scripts"),
    ("tps", "dev/diagnostic: not for mod scripts"),
    ("watch_entity_data", "dev/diagnostic: not for mod scripts"),
    (
        "watch_server_packets",
        "dev/diagnostic: not for mod scripts",
    ),
    // Client-side helpers live in [`CLIENT_SIDE`] (issue 50).
    // The two the lead kept out (issue 52, round 3):
    // `setting` joins `mod_id` and `key` onto the data dir with no
    // validation, so a script could read any file with `../`;
    // `run_mod_command` runs a console command, which
    // `run_command_as` already covers.
    (
        "setting",
        "unvalidated path: horizon.config() is the Lua way to keep settings",
    ),
    (
        "run_mod_command",
        "runs a console command: use run_command_as (owner decision)",
    ),
];

/// The Generated helpers whose first parameter `mod_id: &str` is bound
/// to the calling mod, not to the script (issue 61): the runtime passes
/// the mod's own id (the `self.id` of its `LuaMod`, the value `horizon.mod_id`
/// holds), and the Lua signature, the `.d.lua` type and the exposure doc
/// leave the parameter out. The reason is the owner's rule that a mod may
/// write only objectives named `<mod_id>.<name>`: a script that could pass
/// another mod's id could write that mod's objectives. Every other helper's
/// `mod_id` (`mod_screen_open`, `mod_key`, ...) stays a script argument.
/// `run` fails if a name here is not a Generated action whose first
/// parameter is `mod_id: &str` (a stale entry).
pub const BOUND_MOD_ID: &[&str] = &[
    "scoreboard_add_objective",
    "scoreboard_add_score",
    "scoreboard_add_team",
    "scoreboard_remove_objective",
    "scoreboard_remove_team",
    "scoreboard_set_score",
    "scoreboard_team_join",
    "scoreboard_team_leave",
];

/// Whether `name`'s first parameter is bound to the calling mod (see
/// [`BOUND_MOD_ID`]).
pub(crate) fn is_bound_mod_id(name: &str) -> bool {
    BOUND_MOD_ID.contains(&name)
}

/// The Generated helpers that belong to the client state (issue 50):
/// client UI, input, screens, local sound, the window, and the client
/// reads. Emitted in their own lists, `GENERATED_CLIENT_READS` and
/// `GENERATED_CLIENT_ACTIONS`, never in `GENERATED_READS` /
/// `GENERATED_ACTIONS` (those stay server-state, so the enabled server
/// helpers do not move). Every name classifies as Generated, as in
/// [`EXCLUDED`]; a name not here stays server-state by default.
/// `mod_key` reads the client's key bindings, so it is client-state
/// too (issue 52, round 3); `text_allowed` stays server-state (the
/// server's chat filter).
pub const CLIENT_SIDE: &[&str] = &[
    "apply_screen_blur",
    "chat_input",
    "client_block_obstruction",
    "client_entity_move",
    "client_visual_clear",
    "flame_particle",
    "has_alt_down",
    "has_control_down",
    "has_shift_down",
    "key_down",
    "menu_blur_radius",
    "mod_key",
    "mod_screen",
    "mod_screen_close",
    "mod_screen_open",
    "play_local_sound",
    "playing",
    "refresh_biome_colors",
    "screen_pos",
    "screen_title",
    "text_width",
    "window",
];

/// Whether `name` is one of the generated client-state helpers.
pub(crate) fn is_client_side(name: &str) -> bool {
    CLIENT_SIDE.contains(&name)
}

/// Why `name` is excluded, or `None` when it is enabled: the denylist
/// above (see [`EXCLUDED`]).
pub(crate) fn exclusion_of(name: &str) -> Option<&'static str> {
    EXCLUDED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, why)| *why)
}

/// The enabled rows, in catalog (name) order: every helper the model
/// classifies as Generated except the denylist. `hooks_doc` uses the
/// same set for its Lua cells.
pub(crate) fn enabled_rows(rows: &[HelperRow]) -> Vec<HelperRow> {
    rows.iter()
        .filter(|row| matches!(row.class, HelperClass::Generated(_)))
        .filter(|row| exclusion_of(&row.name).is_none())
        .map(|row| HelperRow {
            name: row.name.clone(),
            target: row.target.clone(),
            class: row.class.clone(),
            args: row.args.clone(),
            ret: row.ret.clone(),
            doc: row.doc.clone(),
            api: row.api,
            receiver: row.receiver.clone(),
            receiver_mut: row.receiver_mut,
            ret_borrowed: row.ret_borrowed,
            ret_static: row.ret_static,
        })
        .collect()
}

/// The enabled helper names, in catalog order, from the model.
pub(crate) fn enabled_helper_names(root: &Path) -> Result<Vec<String>, String> {
    let rows = classify_helpers(root)?;
    Ok(enabled_rows(&rows)
        .into_iter()
        .map(|row| row.name)
        .collect())
}

fn run_with_excluded_check(rows: &[HelperRow]) -> Result<(), String> {
    for (name, _) in EXCLUDED {
        let row = rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("{name} is in EXCLUDED but is not a helper"))?;
        if !matches!(row.class, HelperClass::Generated(_)) {
            return Err(format!(
                "{name} is excluded but classifies as {:?}",
                row.class
            ));
        }
        if is_client_side(name) {
            return Err(format!("{name} is both excluded and client-side"));
        }
    }
    for name in CLIENT_SIDE {
        let row = rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("{name} is in CLIENT_SIDE but is not a helper"))?;
        if !matches!(row.class, HelperClass::Generated(_)) {
            return Err(format!(
                "{name} is client-side but classifies as {:?}",
                row.class
            ));
        }
    }
    for name in BOUND_MOD_ID {
        let row = rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("{name} is in BOUND_MOD_ID but is not a helper"))?;
        if !matches!(row.class, HelperClass::Generated(GenKind::Action)) {
            return Err(format!(
                "{name} is bound to the calling mod but is not a generated action: {:?}",
                row.class
            ));
        }
        if !row
            .args
            .first()
            .is_some_and(|a| a.name == "mod_id" && a.rust == "&str")
        {
            return Err(format!(
                "{name} is bound to the calling mod but its first parameter is not `mod_id: &str`"
            ));
        }
    }
    Ok(())
}

pub fn run(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let rows = classify_helpers(&root)?;
    run_with_excluded_check(&rows)?;
    let (generated, already, adapter, handle, cannot) = class_counts(&rows);
    let enabled = enabled_rows(&rows);
    println!(
        "lua-helpers: {generated} generated, {already} already-lua, {adapter} adapter, \
         {handle} handle, {cannot} cannot ({} total; {} enabled, {} excluded)",
        rows.len(),
        enabled.len(),
        generated - enabled.len()
    );
    // The hand-written and generated names stay disjoint, and no
    // emitted name may be a reserved `horizon.*` name (the model
    // classifies those as Cannot already; this fails if a later edit
    // lets one through).
    let lua_names: BTreeSet<&str> = super::hooks_doc::lua_helper_names().into_iter().collect();
    for row in &enabled {
        if lua_names.contains(row.name.as_str()) {
            return Err(format!(
                "{} is both a hand-written Lua helper and generated",
                row.name
            ));
        }
        if super::lua_types::reserved_horizon_name(&row.name) {
            return Err(format!(
                "{} is generated but collides with an existing horizon.* function",
                row.name
            ));
        }
    }
    let bridge = rustfmt(&bridge_of(&enabled, &REAL_BACKEND))?;
    let tests = rustfmt(&tests_of(&enabled, &REAL_BACKEND))?;
    let synthetic = rustfmt(&synthetic_of())?;
    let exposure = exposure_of(&rows, &enabled);
    for (path, out) in [
        (
            root.join("crates/event-horizon-lua/src/generated_helpers.rs"),
            bridge,
        ),
        (
            root.join("crates/event-horizon-lua/src/generated_helpers_tests.rs"),
            tests,
        ),
        (
            root.join("crates/event-horizon-lua/src/generated_helpers_synthetic.rs"),
            synthetic,
        ),
        (root.join("docs/api/helper-exposure.md"), exposure),
    ] {
        if check {
            let current = fs::read_to_string(&path).unwrap_or_default();
            if current != out {
                return Err(format!(
                    "{} is out of date: run cargo run -p xtask -- lua-helpers",
                    path.display()
                ));
            }
        } else {
            fs::write(&path, out).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    // The two sibling docs this generator feeds (`hooks-doc` and
    // `lua-types` read the model's enabled set): checked or written
    // through their own generators, so one command keeps all five fresh.
    super::hooks_doc::run(check)?;
    super::lua_types::run(check)?;
    Ok(())
}

/// `rustfmt` (the same passthrough `horizon-mod-api-gen` uses): the
/// emitted Rust arrives formatted, so `cargo fmt --all --check` agrees
/// with `lua-helpers --check` without a second touch-up run.
fn rustfmt(code: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = std::process::Command::new("rustfmt")
        .args(["--emit", "stdout", "--edition", "2024"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("rustfmt stdin")?
        .write_all(code.as_bytes())
        .map_err(|e| e.to_string())?;
    let formatted = child.wait_with_output().map_err(|e| e.to_string())?;
    if !formatted.status.success() {
        return Err("rustfmt failed".into());
    }
    String::from_utf8(formatted.stdout).map_err(|e| e.to_string())
}

/// The Lua names of the enabled helpers' halves, split by side:
/// `(server reads, server actions, client reads, client actions)`. A
/// helper in [`CLIENT_SIDE`] lands in the client halves, never the
/// server halves (the 88 enabled server helpers do not move); the read
/// and action arms themselves are shared (issue 50).
fn kinds_of(rows: &[HelperRow]) -> (Vec<&str>, Vec<&str>, Vec<&str>, Vec<&str>) {
    let mut reads = Vec::new();
    let mut actions = Vec::new();
    let mut client_reads = Vec::new();
    let mut client_actions = Vec::new();
    for row in rows {
        match &row.class {
            HelperClass::Generated(GenKind::Read) => {
                if is_client_side(&row.name) {
                    client_reads.push(row.name.as_str());
                } else {
                    reads.push(row.name.as_str());
                }
            }
            HelperClass::Generated(GenKind::Action) => {
                if is_client_side(&row.name) {
                    client_actions.push(row.name.as_str());
                } else {
                    actions.push(row.name.as_str());
                }
            }
            _ => {}
        }
    }
    (reads, actions, client_reads, client_actions)
}

/// `a{index}` local for the `index`-th argument: the native argument's
/// own name, suffixed on collision (`id`, `id_2`), so the emitted arm
/// reads like the hand-written ones.
fn arg_names(row: &HelperRow) -> Vec<String> {
    let mut used = std::collections::BTreeSet::new();
    row.args
        .iter()
        .map(|a| {
            let mut name = a.name.clone();
            let base = name.clone();
            let mut n = 2;
            while used.contains(&name) {
                name = format!("{base}_{n}");
                n += 1;
            }
            used.insert(name.clone());
            name
        })
        .collect()
}

/// The reads of one arm's arguments (the `index` is 1-based, the queue's
/// and `read`'s own numbering). A player reads as a `Value` for `uuid_of`;
/// an entity as a `Value` for `entity_id` (the id is then resolved through
/// `Host::entity`); a `LevelRef` as a `Value` for `dimension_of` (the
/// dimension id string, resolved through `Host::level`); a `Direction`
/// as a `Value` for `direction_of` (the six lowercase names, as the
/// hooks' converter errors); a position table as a `Table` read
/// component-wise (`req_finite` for `[f64; 3]`, `req_key` for `[i32; 3]`
/// — the same building blocks `LuaStruct` for those types uses, so an
/// `[i32; 3]` component errors on a fractional or out-of-range value
/// instead of saturating, as mlua's `i32` does); a tuple as a `Value`
/// (nil for `None`, the table for `Some`, read element-wise); a string
/// as a `String`; a bool as a `bool`; a float with the finite check
/// naming the argument (non-finite floats are a script error, and a
/// `"nan"` string that coerces is caught the same way); an int with the
/// saturating cast (never `as` wrap: -1 and 300 for a u8 or u32). A
/// malformed script argument IS an error naming the helper and the
/// argument (`hide_entity_from: allow: expected a boolean`).
/// `source: "rest"` is the read call's `MultiValue` (read through
/// `read_arg`); `source: "args"` is the queued action's `Table` (read
/// through `read_action_arg`, the same naming wrapper over a `Table`).
fn read_lines(row: &HelperRow, source: &str) -> Vec<String> {
    let names = arg_names(row);
    let mut out = Vec::new();
    // A bound `mod_id` is the runtime's, not the script's: it is not read,
    // and the script's arguments start at index 1 after it (see `call_args`).
    let skip = usize::from(is_bound_mod_id(&row.name));
    for (i, (arg, local)) in row.args.iter().zip(&names).enumerate().skip(skip) {
        let at = i + 1 - skip;
        let raw = if source == "args" {
            format!(
                "read_action_arg(args, {at}, {:?}, {:?})?",
                row.name, arg.name
            )
        } else {
            // A `MultiValue` has no typed `get`: the bridge's `read_arg`
            // reads the `index`-th value through `FromLua` (with `lua`
            // at hand), and a failure names the helper and the argument.
            format!(
                "read_arg(lua, &rest, {at}, {:?}, {:?})?",
                row.name, arg.name
            )
        };
        if arg.player {
            out.push(format!("let {local}: Value = {raw};"));
            continue;
        }
        if arg.entity {
            out.push(format!("let {local}_id: Value = {raw};"));
            out.push(format!(
                "let {local}_id: u32 = entity_id_of({:?}, {:?}, &{local}_id)?;",
                row.name, arg.name
            ));
            continue;
        }
        // A `Direction`: one of its six lowercase name strings,
        // through the shared `direction_of` (the hooks' converter's
        // names and errors, without needing a `Lua` in the
        // queued-action arm).
        if arg.direction {
            out.push(format!("let {local}_lua: Value = {raw};"));
            out.push(format!(
                "let {local}: Direction = direction_of({:?}, {:?}, &{local}_lua)?;",
                row.name, arg.name
            ));
            continue;
        }
        // A `LevelRef`: the level's dimension id string, through
        // `dimension_of` (a non-string or empty id is an error naming
        // the argument); the resolution below (`level_fallback`)
        // turns it into its handle.
        if arg.level {
            out.push(format!("let {local}_lua: Value = {raw};"));
            out.push(format!(
                "let {local}_dim: String = dimension_of({:?}, {:?}, &{local}_lua)?;",
                row.name, arg.name
            ));
            continue;
        }
        // An item table: the shared `stack_from_table` (a table of
        // `id`, `count`, `damage`, `extra`, `plain`). Both the read
        // and the queued-action arm pass `lua`, so the seeded `extra`
        // types come back.
        if arg.rust == "ItemStack" {
            let lua = "Some(lua)";
            let field = format!("{}.{}", row.name, arg.name);
            out.push(format!("let {local}_lua: Value = {raw};"));
            out.push(format!(
                "let {local} = crate::lua_mods::stack_from_table({lua}, {local}_lua.as_table().ok_or_else(|| mlua::Error::runtime(format!(\"{{}}: expected an item table, got {{}}\", {field:?}, {local}_lua.type_name())))?, {field:?})?;"
            ));
            continue;
        }
        // An NBT table: the existing `nbt::table_to_compound`
        // (`&Compound` borrows back for the host call, as `&str`
        // does). A non-table is a script error naming the argument.
        if arg.rust == "Compound" || arg.rust == "&Compound" {
            let field = format!("{}.{}", row.name, arg.name);
            out.push(format!("let {local}_lua: Value = {raw};"));
            out.push(format!(
                "let {local}_t = {local}_lua.as_table().ok_or_else(|| mlua::Error::runtime(format!(\"{{}}: expected a table, got {{}}\", {field:?}, {local}_lua.type_name())))?;"
            ));
            out.push(format!(
                "let {local} = crate::lua_mods::nbt::table_to_compound({local}_t, None).map_err(|e| mlua::Error::runtime(format!(\"{{}}: {{e}}\", {field:?})))?;"
            ));
            continue;
        }
        // An equipment-slot name: one of its eight lowercase name
        // strings, through the shared `equipment_slot_of` (a wrong
        // string or a non-string is a script error naming the
        // argument, as for every argument).
        if arg.rust == "EquipmentSlot" {
            out.push(format!("let {local}_lua: Value = {raw};"));
            out.push(format!(
                "let {local}: EquipmentSlot = equipment_slot_of({:?}, {:?}, &{local}_lua)?;",
                row.name, arg.name
            ));
            continue;
        }
        // An optional block-state name: nil for `None`, a name the
        // host resolves with `state_named` (an unknown name is a
        // script error naming the argument).
        if arg.rust == "Option<BlockStateId>" {
            let host_var = if source == "args" { "host" } else { "h" };
            out.push(format!("let {local}_lua: Value = {raw};"));
            out.push(format!("let {local} = match {local}_lua {{"));
            out.push("    Value::Nil => None,".to_string());
            out.push("    Value::String(s) => {".to_string());
            out.push("        let name = s.to_string_lossy().to_owned();".to_string());
            out.push(format!("        match {host_var}.state_named(&name) {{"));
            out.push("            Some(state) => Some(state),".to_string());
            out.push(format!(
                "            None => return Err(mlua::Error::runtime(format!(\"{{}}.{{}}: no block state {{name:?}}\", {:?}, {:?}))),",
                row.name, arg.name
            ));
            out.push("        }".to_string());
            out.push("    }".to_string());
            out.push(format!(
                "    other => return Err(mlua::Error::runtime(format!(\"{{}}.{{}}: expected a block state name or nil, got {{}}\", {:?}, {:?}, other.type_name()))),",
                row.name, arg.name
            ));
            out.push("};".to_string());
            continue;
        }
        // An optional plain two-tuple: nil for `None`, the table for
        // `Some`, read element-wise (a positional sequence read by
        // number, a named table by field name).
        if arg.rust.starts_with("Option<(") {
            out.extend(tuple_arg_lines(row, arg, local, raw));
            continue;
        }
        // A position table: read it, then its three finite `f64`s or
        // its three `i32`s (mlua's `i32` errors on a fractional or
        // out-of-range component, it does not saturate).
        if matches!(arg.rust.as_str(), "[f64; 3]" | "[i32; 3]") {
            let field = format!("{}.{}", row.name, arg.name);
            out.push(format!("let {local}: Table = {raw};"));
            for comp in ["x", "y", "z"] {
                if arg.rust == "[f64; 3]" {
                    out.push(format!(
                        "let {local}_{comp}: f64 = crate::generated_runtime::req_finite::<f64>(&{local}, {comp:?}, {field:?})?;"
                    ));
                } else {
                    out.push(format!(
                        "let {local}_{comp}: i32 = crate::generated_runtime::req_key::<i32>(&{local}, {comp:?}, {field:?})?;"
                    ));
                }
            }
            out.push(format!(
                "let {local}: {} = [{local}_x, {local}_y, {local}_z];",
                arg.rust
            ));
            continue;
        }
        match arg.rust.as_str() {
            "&str" | "String" => {
                // A dimension-id string (the native takes it in a
                // `dimension` argument, like `level`'s own) is
                // validated before the host sees it: a non-string or
                // an empty id is a script error naming the argument,
                // as `dimension_of` answers for a `LevelRef`.
                if arg.name == "dimension" {
                    out.push(format!("let {local}_lua: Value = {raw};"));
                    out.push(format!(
                        "let {local}: String = dimension_of({:?}, {:?}, &{local}_lua)?;",
                        row.name, arg.name
                    ));
                } else {
                    out.push(format!("let {local}: String = {raw};"));
                }
            }
            // `bool`'s `FromLua` is Lua truthiness (any non-nil value is
            // `true`, never an error), so a wrong type would coerce. Read
            // as `Value` and admit only an actual boolean, naming the
            // helper and the argument when it is not one.
            "bool" => out.push(format!(
                "let {local}: bool = match {raw} {{\n    Value::Boolean(b) => b,\n    _ => return Err(mlua::Error::runtime(format!(\"{{}}: {{}}: expected a boolean\", {:?}, {:?}))),\n}};",
                row.name, arg.name
            )),
            "f64" => {
                out.push(format!("let {local}: f64 = {raw};"));
                out.push(format!(
                    "crate::generated_runtime::check_finite({:?}, {local:?}, {local})?;",
                    row.name
                ));
            }
            "f32" => {
                out.push(format!("let {local}64: f64 = {raw};"));
                out.push(format!(
                    "crate::generated_runtime::check_finite({:?}, {local:?}, {local}64)?;",
                    row.name
                ));
                out.push(format!("let {local} = {local}64 as f32;"));
                out.push(format!(
                    "crate::generated_runtime::check_finite({:?}, {local:?}, f64::from({local}))?;",
                    row.name
                ));
            }
            // An `i64` is read as is: a `try_from` to the same type is a
            // no-op that clippy rejects.
            "i64" => out.push(format!("let {local}: i64 = {raw};")),
            ty => {
                out.push(format!("let n{at}: i64 = {raw};"));
                out.push(format!(
                    "let {local}: {ty} = {ty}::try_from(n{at}).unwrap_or(if n{at} < 0 {{ {ty}::MIN }} else {{ {ty}::MAX }});"
                ));
            }
        }
    }
    out
}

/// The reads of one optional plain two-tuple argument: nil for `None`,
/// the table for `Some` (each element read from its sequence position
/// or its field name, naming the argument on a malformed table). The
/// local is the owned normalized type (`String` for `&str`, which
/// `call_args` borrows back for the host call).
fn tuple_arg_lines(
    row: &HelperRow,
    arg: &super::lua_helper_model::Arg,
    local: &str,
    raw: String,
) -> Vec<String> {
    let inner = arg
        .rust
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(&arg.rust);
    let shape = super::lua_helper_model::tuple_shape(inner);
    let field = format!("{}.{}", row.name, arg.name);
    let owned = arg.rust.replace("&str", "String");
    let wrap = format!(
        "|e| mlua::Error::runtime(format!(\"{{}}.{{}}: {{e}}\", {:?}, {:?}))",
        row.name, arg.name
    );
    let mut out = vec![
        format!("let {local}_lua: Value = {raw};"),
        format!("let {local}: {owned} = match {local}_lua {{"),
        "    Value::Nil => None,".to_string(),
        format!("    Value::Table({local}_t) => {{"),
    ];
    for (i, elem) in shape.elems.iter().enumerate() {
        let key = if shape.named {
            format!(
                "{:?}",
                shape.fields.get(i).map(String::as_str).unwrap_or("")
            )
        } else {
            format!("{}i64", i + 1)
        };
        match elem.as_str() {
            "Option<String>" | "Option<&str>" => {
                out.push(format!(
                    "        let {local}_e{i}: Option<String> = {local}_t.get({key}).map_err({wrap})?;",
                ));
            }
            "[i32; 3]" => {
                out.push(format!(
                    "        let {local}_e{i}t: Table = {local}_t.get({key}).map_err({wrap})?;",
                ));
                for comp in ["x", "y", "z"] {
                    out.push(format!(
                        "        let {local}_e{i}_{comp}: i32 = crate::generated_runtime::req_key::<i32>(&{local}_e{i}t, {comp:?}, {field:?})?;",
                    ));
                }
                out.push(format!(
                    "        let {local}_e{i}: [i32; 3] = [{local}_e{i}_x, {local}_e{i}_y, {local}_e{i}_z];",
                ));
            }
            "[f64; 3]" => {
                out.push(format!(
                    "        let {local}_e{i}t: Table = {local}_t.get({key}).map_err({wrap})?;",
                ));
                for comp in ["x", "y", "z"] {
                    out.push(format!(
                        "        let {local}_e{i}_{comp}: f64 = crate::generated_runtime::req_finite::<f64>(&{local}_e{i}t, {comp:?}, {field:?})?;",
                    ));
                }
                out.push(format!(
                    "        let {local}_e{i}: [f64; 3] = [{local}_e{i}_x, {local}_e{i}_y, {local}_e{i}_z];",
                ));
            }
            "String" | "&str" => {
                out.push(format!(
                    "        let {local}_e{i}: String = {local}_t.get({key}).map_err({wrap})?;",
                ));
            }
            "f32" => {
                out.push(format!(
                    "        let {local}_e{i}64: f64 = {local}_t.get({key}).map_err({wrap})?;",
                ));
                out.push(format!(
                    "        crate::generated_runtime::check_finite({:?}, {:?}, {local}_e{i}64)?;",
                    row.name, arg.name
                ));
                out.push(format!(
                    "        let {local}_e{i}: f32 = {local}_e{i}64 as f32;",
                ));
                out.push(format!(
                    "        crate::generated_runtime::check_finite({:?}, {:?}, f64::from({local}_e{i}))?;",
                    row.name, arg.name
                ));
            }
            "f64" => {
                out.push(format!(
                    "        let {local}_e{i}: f64 = {local}_t.get({key}).map_err({wrap})?;",
                ));
                out.push(format!(
                    "        crate::generated_runtime::check_finite({:?}, {:?}, {local}_e{i})?;",
                    row.name, arg.name
                ));
            }
            _ => {
                out.push(format!(
                    "        let {local}_e{i}: {elem} = {local}_t.get({key}).map_err({wrap})?;",
                ));
            }
        }
    }
    out.push(format!("        Some(({local}_e0, {local}_e1))"));
    out.push("    }".to_owned());
    out.push(format!(
        "    _ => return Err(mlua::Error::runtime(format!(\"{{}}.{{}}: expected a table\", {:?}, {:?}))),",
        row.name, arg.name
    ));
    out.push("};".to_owned());
    out
}

/// The unresolvable-entity rule (the id resolved via `Host::entity`):
/// as the unresolvable player (a silent no-op, actions; nil, reads),
/// never calling the host method.
fn entity_fallback(row: &HelperRow, kind: GenKind) -> Vec<String> {
    let names = arg_names(row);
    let mut out = Vec::new();
    for (arg, local) in row.args.iter().zip(&names) {
        if arg.entity {
            match kind {
                GenKind::Action => {
                    out.push(format!(
                        "let Some({local}) = host.entity({local}_id) else {{",
                    ));
                    out.push(format!(
                        "    log::debug!(\"[lua] {}: no such entity\");",
                        row.name
                    ));
                    out.push("    return Ok(());".to_owned());
                    out.push("};".to_owned());
                }
                GenKind::Read => {
                    out.push(format!("let Some({local}) = h.entity({local}_id) else {{",));
                    out.push("    return Ok(Value::Nil);".to_owned());
                    out.push("};".to_owned());
                }
            }
        }
    }
    out
}

/// The unresolvable-level rule (the dimension resolved via
/// `Host::level`): as the unresolvable player (a silent no-op, actions;
/// nil, reads), never calling the host method.
fn level_fallback(row: &HelperRow, kind: GenKind) -> Vec<String> {
    let names = arg_names(row);
    let mut out = Vec::new();
    for (arg, local) in row.args.iter().zip(&names) {
        if arg.level {
            match kind {
                GenKind::Action => {
                    out.push(format!(
                        "let Some({local}) = host.level(&{local}_dim) else {{",
                    ));
                    out.push(format!(
                        "    log::debug!(\"[lua] {}: no such level\");",
                        row.name
                    ));
                    out.push("    return Ok(());".to_owned());
                    out.push("};".to_owned());
                }
                GenKind::Read => {
                    out.push(format!("let Some({local}) = h.level(&{local}_dim) else {{",));
                    out.push("    return Ok(Value::Nil);".to_owned());
                    out.push("};".to_owned());
                }
            }
        }
    }
    out
}

/// The unresolvable-player rule: as the hand-written actions (a registered
/// action, silent no-op with a debug log) or as `menu_contents` (nil).
fn player_fallback(row: &HelperRow, kind: GenKind) -> Vec<String> {
    let names = arg_names(row);
    let mut out = Vec::new();
    for (arg, local) in row.args.iter().zip(&names) {
        if arg.player {
            match kind {
                GenKind::Action => {
                    out.push(format!("let Some(uuid) = uuid_of(host, &{local}) else {{",));
                    out.push(format!(
                        "    log::debug!(\"[lua] {}: no such player\");",
                        row.name
                    ));
                    out.push("    return Ok(());".to_owned());
                    out.push("};".to_owned());
                }
                GenKind::Read => {
                    out.push(format!("let Some(uuid) = uuid_of(h, &{local}) else {{",));
                    // A non-optional string list is the empty table for an
                    // absent player (the host's `Vec` is empty for one who
                    // is not on); every other read answers nil.
                    if row.ret == "Vec<String>" {
                        out.push("    return Ok(Value::Table(lua.create_table()?));".to_owned());
                    } else {
                        out.push("    return Ok(Value::Nil);".to_owned());
                    }
                    out.push("};".to_owned());
                }
            }
        }
    }
    out
}

/// The host call's arguments, in order: a player as its uuid, a string
/// borrowed, a tuple borrowed back element-wise (`&str` as `as_str()`,
/// `Option<&str>` as `as_deref()`), the rest as their converted locals.
fn call_args(row: &HelperRow) -> String {
    let names = arg_names(row);
    let mut parts = Vec::new();
    let bound = is_bound_mod_id(&row.name);
    for (i, (arg, local)) in row.args.iter().zip(&names).enumerate() {
        if bound && i == 0 {
            // The calling mod's id: the bridge fn's own `mod_id` parameter.
            parts.push("mod_id".to_owned());
        } else if arg.player {
            parts.push("uuid".to_owned());
        } else if arg.rust == "&str" || arg.rust == "&Compound" {
            parts.push(format!("&{local}"));
        } else if arg.rust.starts_with("Option<(") {
            let inner = arg
                .rust
                .strip_prefix("Option<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or(&arg.rust);
            let shape = super::lua_helper_model::tuple_shape(inner);
            let from = shape
                .elems
                .iter()
                .enumerate()
                .map(|(i, e)| match e.as_str() {
                    "&str" => format!("e{i}.as_str()"),
                    "Option<&str>" => format!("e{i}.as_deref()"),
                    _ => format!("*e{i}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!(
                "match {local} {{ Some((ref e0, ref e1)) => Some(({from})), None => None }}"
            ));
        } else {
            parts.push(local.clone());
        }
    }
    parts.join(", ")
}

/// A two-tuple read back as a Lua table: the named table where a
/// sequence would lose a nil second element (an `Option` element is
/// set only when it is `Some`; otherwise it stays absent, which is
/// nil), the hook model's nine known shapes reuse their field names,
/// and a positional sequence everywhere else.
fn tuple_value_of(inner: &str, expr: &str) -> String {
    let shape = super::lua_helper_model::tuple_shape(inner);
    let mut out = String::from("{ let t = lua.create_table()?;");
    for (i, elem) in shape.elems.iter().enumerate() {
        let slot = if shape.named {
            format!(
                "{:?}",
                shape.fields.get(i).map(String::as_str).unwrap_or("")
            )
        } else {
            format!("{}", i + 1)
        };
        if let Some(inner_elem) = elem
            .strip_prefix("Option<")
            .and_then(|s| s.strip_suffix('>'))
        {
            out.push_str(&format!(
                " if let Some(e{i}) = {expr}.{i} {{ t.set({slot}, {val})?; }}",
                i = i,
                expr = expr,
                slot = slot,
                val = elem_value(inner_elem, &format!("e{i}")),
            ));
        } else {
            out.push_str(&format!(
                " t.set({slot}, {val})?;",
                slot = slot,
                val = elem_value(elem, &format!("{expr}.{i}")),
            ));
        }
    }
    out.push_str(" Value::Table(t) }");
    out
}

/// One tuple element as the [`mlua::IntoLua`] value `Table::set` takes
/// (see [`tuple_value_of`]): a bool or a string as they are, a `u128` as
/// its uuid text, a number as its number, a position as its table.
fn elem_value(elem: &str, expr: &str) -> String {
    match elem {
        "bool" | "String" | "f64" => expr.to_string(),
        "u128" => format!("crate::generated_runtime::uuid_text({expr})"),
        "f32" => format!("f64::from({expr})"),
        "[f64; 3]" => format!("crate::lua_mods::goals::vec3(lua, {expr})?"),
        "[i32; 3]" => format!("crate::generated_runtime::pos_table(lua, {expr})?"),
        _ => expr.to_string(),
    }
}

/// A scalar read back as a Lua value: the booleans and strings exact,
/// the integers in range (`Value::Integer` holds any i64), the floats as
/// numbers, a position as its `{x, y, z}` table, a level as its
/// dimension string, a `u128` as its uuid text, a two-tuple as its
/// table.
fn value_of(scalar: &str, expr: &str) -> String {
    if scalar.starts_with('(') {
        return tuple_value_of(scalar, expr);
    }
    match scalar {
        "bool" => format!("Value::Boolean({expr})"),
        "String" => format!("Value::String(lua.create_string({expr})?)"),
        "u128" => format!(
            "Value::String(lua.create_string(crate::generated_runtime::uuid_text({expr}))?)"
        ),
        "LevelRef" => format!("Value::String(lua.create_string({expr}.dimension())?)"),
        // `f64::from` only where the value is not already an `f64`
        // (an `f32` keeps the conversion); `i64` is already one.
        "f32" => format!("Value::Number(f64::from({expr}))"),
        "f64" => format!("Value::Number({expr})"),
        "i64" => format!("Value::Integer({expr})"),
        "[f64; 3]" => format!("Value::Table(crate::lua_mods::goals::vec3(lua, {expr})?)"),
        "[i32; 3]" => format!("Value::Table(crate::generated_runtime::pos_table(lua, {expr})?)"),
        // An item table: the `Item` table (`id`, `count`, `damage`,
        // `extra`, `plain`), as the hooks hand them out.
        "ItemStack" => format!("Value::Table(crate::lua_mods::item_table_of(lua, &{expr})?)"),
        // A block-state name: the `state_name` string.
        "BlockStateId" => format!("Value::String(lua.create_string(h.state_name({expr}))?)"),
        // A string list: a 1-based sequence table of its strings (an empty
        // list is an empty table), the `Option`'s nil being `read_arm`'s.
        "Vec<String>" => format!("Value::Table(lua.create_sequence_from({expr})?)"),
        _ => format!("Value::Integer(i64::from({expr}))"),
    }
}

/// Which host the bridge arms address: the real `Host`, or the
/// synthetic conformance module's local `SyntheticHost` (the same
/// emitter functions, a different trait name, read/action function
/// names, and double constructor).
struct Backend {
    host_ty: &'static str,
    read_fn: &'static str,
    action_fn: &'static str,
    host_new: &'static str,
    /// The recording double's struct name (`GenHost`/`SyntheticDouble`).
    double_ty: &'static str,
}

const REAL_BACKEND: Backend = Backend {
    host_ty: "Host",
    read_fn: "generated_read",
    action_fn: "run_generated_action",
    host_new: "gen_host",
    double_ty: "GenHost",
};

const SYNTHETIC_BACKEND: Backend = Backend {
    host_ty: "SyntheticHost",
    read_fn: "synthetic_read",
    action_fn: "run_synthetic_action",
    host_new: "synthetic_host",
    double_ty: "SyntheticDouble",
};

/// The bridge of one read arm: a closure so `?` (the argument reads,
/// the finite checks) widens to the `Some(...)` the caller wants. A
/// malformed script argument is an error naming it; a nil return is nil.
fn read_arm(row: &HelperRow, backend: &Backend) -> String {
    let mut out = format!(
        "            {:?} => Some((|h: &dyn {}| -> mlua::Result<Value> {{\n",
        row.name, backend.host_ty
    );
    for line in read_lines(row, "rest") {
        out.push_str(&format!("                {line}\n"));
    }
    for line in player_fallback(row, GenKind::Read) {
        out.push_str(&format!("                {line}\n"));
    }
    for line in entity_fallback(row, GenKind::Read) {
        out.push_str(&format!("                {line}\n"));
    }
    for line in level_fallback(row, GenKind::Read) {
        out.push_str(&format!("                {line}\n"));
    }
    let call = format!("h.{}({})", row.name, call_args(row));
    if let Some(inner) = row
        .ret
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
    {
        out.push_str(&format!(
            "                match {call} {{\n                    Some(v) => Ok({}),\n                    None => Ok(Value::Nil),\n                }}\n",
            value_of(inner, "v")
        ));
    } else {
        out.push_str(&format!(
            "                Ok({})\n",
            value_of(&row.ret, &call)
        ));
    }
    out.push_str("            })(h)),\n");
    out
}

/// The bridge of one action arm: the host call, the dropped return
/// (a native refusal, a `false`, never aborts the handler's remaining
/// queued actions), and the unresolvable-player/entity rules (a
/// silent no-op). The arm is a closure, so `?` (the argument reads,
/// the finite checks) widens to the `Some(...)` the caller wants;
/// `None` ("not mine") stays the match's own.
fn action_arm(row: &HelperRow, backend: &Backend) -> String {
    let mut out = format!(
        "            {:?} => Some((|host: &mut dyn {}| -> mlua::Result<()> {{\n",
        row.name, backend.host_ty
    );
    // A bound helper takes one argument fewer than its host method: a
    // script that still passes the old leading `mod_id` is refused here,
    // not shifted into the wrong parameters.
    if is_bound_mod_id(&row.name) {
        let script_args = row.args.len() - 1;
        out.push_str(&format!(
            "                let n = args.raw_len();\n                if n > {script_args} {{\n                    return Err(mlua::Error::runtime(format!(\"{}: takes {script_args} argument(s), got {{n}} (the mod id is the runtime's: do not pass it)\")));\n                }}\n",
            row.name
        ));
    }
    for line in read_lines(row, "args") {
        out.push_str(&format!("                {line}\n"));
    }
    for line in player_fallback(row, GenKind::Action) {
        out.push_str(&format!("                {line}\n"));
    }
    for line in entity_fallback(row, GenKind::Action) {
        out.push_str(&format!("                {line}\n"));
    }
    for line in level_fallback(row, GenKind::Action) {
        out.push_str(&format!("                {line}\n"));
    }
    let call = format!("host.{}({})", row.name, call_args(row));
    // A unit call is bare (a `let _ =` on a unit value trips
    // `let_unit_value`); a `bool` is dropped so later actions run.
    if row.ret == "unit" {
        out.push_str(&format!(
            "                // A native refusal is ordinary: later queued actions still run.\n                {call};\n                Ok(())\n",
        ));
    } else {
        out.push_str(&format!(
            "                // A native refusal is ordinary: drop the return\n                // value, so later queued actions still run.\n                let _ = {call};\n                Ok(())\n",
        ));
    }
    out.push_str("            })(host)),\n");
    out
}

/// The `direction_of` helper the bridge carries while a `Direction`
/// argument is emitted: the six lowercase names, with the hooks'
/// converter's errors (a wrong string or a non-string is a script error
/// named `helper.argument`).
fn direction_of_fn() -> &'static str {
    r#"
/// One `Direction` argument's Lua value: one of the six lowercase name
/// strings, as `LuaStruct for Direction` names it, else a script error
/// naming the helper and the argument. Shared by the read and
/// queued-action arms (the latter has no `Lua` at hand for the
/// converter itself).
fn direction_of(helper: &str, arg: &str, value: &Value) -> mlua::Result<Direction> {
    let field = format!("{helper}.{arg}");
    match value {
        Value::String(s) => match s.to_string_lossy().as_ref() {
            "down" => Ok(Direction::Down),
            "up" => Ok(Direction::Up),
            "north" => Ok(Direction::North),
            "south" => Ok(Direction::South),
            "west" => Ok(Direction::West),
            "east" => Ok(Direction::East),
            name => Err(mlua::Error::runtime(format!(
                "{field}: expected one of \"down\", \"up\", \"north\", \"south\", \"west\", \"east\", got {name:?}"
            ))),
        },
        other => Err(mlua::Error::runtime(format!(
            "{field}: expected a name, got {}",
            other.type_name()
        ))),
    }
}

"#
}

/// The `equipment_slot_of` helper the bridge carries while an
/// `EquipmentSlot` argument is emitted: one of its eight lowercase
/// name strings, as the hooks name them (a wrong string or a
/// non-string is a script error naming the helper and the argument).
fn equipment_slot_of_fn() -> &'static str {
    r#"
/// One `EquipmentSlot` argument's Lua value: one of its eight
/// lowercase name strings (`"mainhand"`, `"offhand"`, `"head"`,
/// `"chest"`, `"legs"`, `"feet"`, `"body"`, `"saddle"`), as the
/// hooks name them, else a script error naming the helper and the
/// argument. Shared by the read and queued-action arms (the latter
/// has no `Lua` at hand for the converter itself).
fn equipment_slot_of(helper: &str, arg: &str, value: &Value) -> mlua::Result<EquipmentSlot> {
    let field = format!("{helper}.{arg}");
    match value {
        Value::String(s) => match s.to_string_lossy().as_ref() {
            "mainhand" => Ok(EquipmentSlot::MainHand),
            "offhand" => Ok(EquipmentSlot::OffHand),
            "head" => Ok(EquipmentSlot::Head),
            "chest" => Ok(EquipmentSlot::Chest),
            "legs" => Ok(EquipmentSlot::Legs),
            "feet" => Ok(EquipmentSlot::Feet),
            "body" => Ok(EquipmentSlot::Body),
            "saddle" => Ok(EquipmentSlot::Saddle),
            name => Err(mlua::Error::runtime(format!(
                "{field}: expected one of \"mainhand\", \"offhand\", \"head\", \"chest\", \"legs\", \"feet\", \"body\", \"saddle\", got {name:?}"
            ))),
        },
        other => Err(mlua::Error::runtime(format!(
            "{field}: expected a name, got {}",
            other.type_name()
        ))),
    }
}

"#
}

/// Whether an argument is a dimension-id string: a plain string
/// (`&str`/`String`) the native names `dimension` (like `level`'s
/// own, which the host lookup then resolves). Its non-string or
/// empty value is a script error naming the argument, as
/// `dimension_of` answers.
fn is_dimension_arg(arg: &super::lua_helper_model::Arg) -> bool {
    arg.name == "dimension" && matches!(arg.rust.as_str(), "&str" | "String")
}

/// The `dimension_of` helper the bridge carries while a `LevelRef`
/// argument is emitted: the level's dimension id string, or an error
/// naming the helper and the argument (a non-string and an empty id
/// alike). Resolving the id through `Host::level` is the arm's work
/// (`level_fallback`), as `Host::entity` is for an entity.
fn dimension_of_fn() -> &'static str {
    r#"
/// One `LevelRef` argument's Lua value: its dimension id string
/// (`"minecraft:overworld"`), else a script error naming the helper
/// and the argument. Shared by the read and queued-action arms.
fn dimension_of(helper: &str, arg: &str, value: &Value) -> mlua::Result<String> {
    match value {
        Value::String(s) if !s.to_string_lossy().is_empty() => {
            Ok(s.to_string_lossy().to_owned())
        }
        _ => Err(mlua::Error::runtime(format!(
            "{helper}.{arg}: expected a dimension id"
        ))),
    }
}

"#
}

/// `crates/event-horizon-lua/src/generated_helpers.rs`, the whole bridge
/// against `backend`'s host (the real file against `Host`, the
/// synthetic conformance file against `SyntheticHost`).
fn bridge_of(rows: &[HelperRow], backend: &Backend) -> String {
    let (reads, actions, client_reads, client_actions) = kinds_of(rows);
    let mut out = String::new();
    out.push_str("// Generated by `cargo run -p xtask -- lua-helpers` from the\n");
    out.push_str("// `native_*` helper signatures (`lua_helper_model`): do not edit. Regenerate\n");
    out.push_str("// to update.\n//\n");
    out.push_str("// The Lua bridge of the host helpers the generator enables: `horizon.<name>`\n");
    out.push_str(
        "// wrappers it installs, the queued actions `run_generated_action` runs after a\n",
    );
    out.push_str("// handler, and the reads `generated_read` answers from the hook's host (both\n");
    out.push_str(
        "// fallthroughs from the hand-written `run_action` and `read` in `lua_mods.rs`).\n",
    );
    out.push_str(
        "// Server-state helpers: the server lists stay out of `CLIENT_API`/`DRAW_API`, so a client\n",
    );
    out.push_str(
        "// state raises the server-only error for them, like the hand-written server ones;\n",
    );
    out.push_str(
        "// client-state helpers (`GENERATED_CLIENT_*`, issue 50) are installed for real on a\n",
    );
    out.push_str(
        "// client state and stubbed on a server state with the client-only error, like\n",
    );
    out.push_str("// the world markers.\n");
    out.push_str(
        "// The hand-written helpers it mirrors: `kick` (a player and a string, dropped\n",
    );
    out.push_str(
        "// bool), `give_experience` (the clamp-then-`as` saturating cast; every integer\n",
    );
    out.push_str("// argument here saturates the same way, never `as` wrap), `teleport_player`\n");
    out.push_str("// (non-finite floats are a script error naming the argument). An\n");
    out.push_str("// entity argument is an id number (a Lua number, integral and\n");
    out.push_str("// non-negative) resolved through `Host::entity` to its handle; a\n");
    out.push_str("// position is a `{x, y, z}` table (`[i32; 3]` errors on a\n");
    out.push_str("// fractional or out-of-range component instead of saturating).\n");
    out.push_str("// A malformed script argument IS an error naming the argument; a\n");
    out.push_str("// host-side refusal (a `false`) is never a Lua error and never aborts\n");
    out.push_str("// the handler's remaining queued actions; an unresolvable player or\n");
    out.push_str("// an unresolvable entity is a silent no-op (actions) or nil (reads,\n");
    out.push_str("// as `menu_contents` and `has_permission` do).\n\n");
    // The player lookup and the `Host` import are real-bridge only:
    // the synthetic rows carry no player arguments and no `Host`
    // methods, so those imports would be unused there.
    if backend.host_ty == "Host" {
        out.push_str("use crate::lua_mods::uuid_of;\n");
        out.push_str("use horizon_client::mod_api::Host;\n");
    }
    if rows.iter().any(|row| row.args.iter().any(|a| a.direction)) {
        out.push_str("use horizon_client::mod_api::Direction;\n");
    }
    if rows
        .iter()
        .any(|row| row.args.iter().any(|a| a.rust == "EquipmentSlot"))
    {
        out.push_str("use horizon_client::mod_api::EquipmentSlot;\n");
    }
    // The block-state type is named by the synthetic trait's
    // `Option<BlockStateId>` argument only (the real bridge never
    // names it: it carries `state_name`/`state_named` through the
    // host), so the import rides on the argument.
    if rows
        .iter()
        .any(|row| row.args.iter().any(|a| a.rust == "Option<BlockStateId>"))
    {
        out.push_str("use horizon_client::mod_api::BlockStateId;\n");
    }
    out.push_str("use mlua::{Lua, MultiValue, Table, Value};\n\n");
    if rows.iter().any(|row| row.args.iter().any(|a| a.direction)) {
        out.push_str(direction_of_fn());
    }
    if rows
        .iter()
        .any(|row| row.args.iter().any(|a| a.rust == "EquipmentSlot"))
    {
        out.push_str(equipment_slot_of_fn());
    }
    if rows
        .iter()
        .any(|row| row.args.iter().any(|a| a.level || is_dimension_arg(a)))
    {
        out.push_str(dimension_of_fn());
    }
    out.push_str("/// The queued actions this generator emits, installed beside the\n");
    out.push_str("/// hand-written ones.\n");
    out.push_str("pub(crate) const GENERATED_ACTIONS: &[&str] = &[\n");
    for name in &actions {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("];\n\n");
    out.push_str("/// The reads this generator emits: `install_reads` sets one\n");
    out.push_str("/// `horizon.<name>` wrapper per entry, delegating to `read`.\n");
    out.push_str("pub(crate) const GENERATED_READS: &[&str] = &[\n");
    for name in &reads {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("];\n\n");
    out.push_str("/// The queued actions of the client state (issue 50): installed for real\n");
    out.push_str("/// on a client state, stubbed on a server state with the client-only\n");
    out.push_str("/// error, like the world markers.\n");
    out.push_str("pub(crate) const GENERATED_CLIENT_ACTIONS: &[&str] = &[\n");
    for name in &client_actions {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("];\n\n");
    out.push_str("/// The reads of the client state (issue 50): `install_reads` sets one\n");
    out.push_str("/// `horizon.<name>` wrapper per entry, as for the server reads; a\n");
    out.push_str("/// client state keeps them for real, a server state stubs them.\n");
    out.push_str("pub(crate) const GENERATED_CLIENT_READS: &[&str] = &[\n");
    for name in &client_reads {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("];\n\n");
    // The read-call argument reader is used only by reads with
    // arguments: the synthetic reads carry none, and an unused
    // helper would warn.
    if rows.iter().any(|row| {
        matches!(row.class, HelperClass::Generated(GenKind::Read)) && !row.args.is_empty()
    }) {
        out.push_str("/// Reads the `index`-th (1-based) argument of a read call as `T`:\n");
        out.push_str("/// the malformed argument is a script error naming the helper\n");
        out.push_str("/// and the argument (`<helper>: <argument>: <expected>`), as the\n");
        out.push_str("/// hand-written reads do when they convert a value.\n");
        out.push_str("fn read_arg<T: mlua::FromLua>(\n");
        out.push_str("    lua: &Lua,\n");
        out.push_str("    rest: &MultiValue,\n");
        out.push_str("    index: usize,\n");
        out.push_str("    helper: &str,\n");
        out.push_str("    arg: &str,\n");
        out.push_str(") -> mlua::Result<T> {\n");
        out.push_str("    let missing = || {\n");
        out.push_str("        mlua::Error::runtime(format!(\"{helper}: {arg} is missing\"))\n");
        out.push_str("    };\n");
        out.push_str("    let value = rest.get(index - 1).ok_or_else(missing)?;\n");
        out.push_str(
        "    T::from_lua(value.clone(), lua).map_err(|e| mlua::Error::runtime(format!(\"{helper}: {arg}: {e}\")))\n",
    );
        out.push_str("}\n\n");
    }
    // The entity-id reader is used only by entity arms: the
    // synthetic rows carry none, and an unused helper would warn.
    if rows.iter().any(|row| row.args.iter().any(|a| a.entity)) {
        out.push_str("/// An entity argument's id from its Lua value: a Lua number that is\n");
        out.push_str("/// integral, finite and non-negative (the entity's network id),\n");
        out.push_str("/// else a script error naming the helper and the argument. The\n");
        out.push_str("/// arm then resolves the id through `Host::entity`.\n");
        out.push_str(
            "fn entity_id_of(helper: &str, arg: &str, value: &Value) -> mlua::Result<u32> {\n",
        );
        out.push_str("    match value {\n");
        out.push_str(
            "        Value::Integer(n) if *n >= 0 && *n <= i64::from(u32::MAX) => Ok(*n as u32),\n",
        );
        out.push_str("        Value::Number(n) if n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= u32::MAX as f64 => Ok(*n as u32),\n");
        out.push_str("        _ => Err(mlua::Error::runtime(format!(\"{helper}.{arg}: expected an entity id\"))),\n");
        out.push_str("    }\n");
        out.push_str("}\n\n");
    }
    out.push_str("/// Reads the `index`-th (1-based) argument of a queued action as `T`:\n");
    out.push_str("/// the same naming wrapper over the action's `Table`, so a\n");
    out.push_str("/// malformed action argument names the helper and the argument.\n");
    out.push_str("fn read_action_arg<T: mlua::FromLua>(\n");
    out.push_str("    args: &Table,\n");
    out.push_str("    index: usize,\n");
    out.push_str("    helper: &str,\n");
    out.push_str("    arg: &str,\n");
    out.push_str(") -> mlua::Result<T> {\n");
    out.push_str("    args\n");
    out.push_str("        .get(index)\n");
    out.push_str("        .map_err(|e| mlua::Error::runtime(format!(\"{helper}: {arg}: {e}\")))\n");
    out.push_str("}\n\n");
    out.push_str("/// Runs one queued generated action after a handler (`None` means the\n");
    out.push_str("/// action is not this generator's, and the caller keeps its own `warn`).\n");
    out.push_str("/// The arms are closures (so `?` widens to the match's `Some`);\n");
    out.push_str("/// `mod_id` is the calling mod's own id, passed to the bound helpers\n");
    out.push_str("/// (`BOUND_MOD_ID`) in place of a script argument.\n");
    out.push_str("#[allow(clippy::redundant_closure_call)]\n");
    out.push_str("#[allow(unused_variables)]\n");
    out.push_str(&format!("pub(crate) fn {}(\n", backend.action_fn));
    out.push_str("    lua: &Lua,\n");
    out.push_str(&format!("    host: &mut dyn {},\n", backend.host_ty));
    out.push_str("    mod_id: &str,\n");
    out.push_str("    action: &str,\n");
    out.push_str("    args: &Table,\n");
    out.push_str(") -> Option<mlua::Result<()>> {\n");
    out.push_str("    match action {\n");
    for row in rows {
        if matches!(row.class, HelperClass::Generated(GenKind::Action)) {
            out.push_str(&action_arm(row, backend));
        }
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out.push_str("/// Answers one generated read from the hook's `&self` host (`None`\n");
    out.push_str("/// means the read is not this generator's, and the caller keeps its own\n");
    out.push_str("/// `no such read` error).\n");
    out.push_str("/// The arms are closures (so `?` widens to the match's `Some`);\n");
    out.push_str("#[allow(clippy::redundant_closure_call)]\n");
    out.push_str("#[allow(unused_variables)]\n");
    out.push_str(&format!("pub(crate) fn {}(\n", backend.read_fn));
    out.push_str("    lua: &Lua,\n");
    out.push_str(&format!("    h: &dyn {},\n", backend.host_ty));
    out.push_str("    what: &str,\n");
    out.push_str("    rest: MultiValue,\n");
    out.push_str(") -> Option<mlua::Result<Value>> {\n");
    out.push_str("    match what {\n");
    for row in rows {
        if matches!(row.class, HelperClass::Generated(GenKind::Read)) {
            out.push_str(&read_arm(row, backend));
        }
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    out
}

/// The `GenHost` double: ONE double for every emitted helper (the `Host`
/// trait carries default bodies for everything it does not override).
/// Each emitted helper records (name, args) and answers a canned value,
/// so a test sees the arguments the bridge handed the host, exactly.
fn gen_host(rows: &[HelperRow]) -> String {
    let mut out = String::new();
    out.push_str("struct GenHost {\n");
    out.push_str("    lines: Mutex<Vec<String>>,\n");
    out.push_str("    none: bool,\n");
    out.push_str("    refuse: bool,\n");
    out.push_str("}\n\n");
    out.push_str("impl GenHost {\n");
    out.push_str("    fn record(&self, name: &str, args: &[String]) {\n");
    out.push_str("        let line = if args.is_empty() {\n");
    out.push_str("            name.to_owned()\n");
    out.push_str("        } else {\n");
    out.push_str("            format!(\"{} {}\", name, args.join(\" \"))\n");
    out.push_str("        };\n");
    out.push_str("        self.lines\n");
    out.push_str("            .lock()\n");
    out.push_str("            .unwrap()\n");
    out.push_str("            .push(line);\n");
    out.push_str("    }\n");
    out.push_str("    fn logged(&self) -> Vec<String> {\n");
    out.push_str("        self.lines.lock().unwrap().clone()\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out.push_str("impl Host for GenHost {\n");
    out.push_str("    fn dimension(&self) -> &'static str {\n");
    out.push_str("        \"minecraft:overworld\"\n");
    out.push_str("    }\n");
    out.push_str(
        "    fn level_players(&self) -> Vec<horizon_client::mod_api::NativeServerPlayer> {\n",
    );
    out.push_str("        vec![horizon_client::test_events::native_server_player(\n");
    out.push_str("            STEVE_UUID,\n");
    out.push_str("            \"steve\".into(),\n");
    out.push_str("            0,\n");
    out.push_str("            \"minecraft:overworld\".into(),\n");
    out.push_str("            [1.0, 64.0, 1.0],\n");
    out.push_str("            0,\n");
    out.push_str("            20.0,\n");
    out.push_str("            0.0,\n");
    out.push_str("            0.0,\n");
    out.push_str("            0,\n");
    out.push_str("            String::new(),\n");
    out.push_str("        )]\n");
    out.push_str("    }\n");
    out.push_str("    fn broadcast(&self, line: horizon_client::mod_api::Text) {\n");
    out.push_str("        self.record(\"broadcast\", &[line.plain()]);\n");
    out.push_str("    }\n");
    // The entity lookup the bridge uses: id 999 resolves to nothing,
    // any other id resolves to its handle (as the `entity` helper
    // would), so a test can drive both an unresolved entity (id 999)
    // and a resolved one (its normal id). `EntityRef::new` and
    // `NativeWorldEpoch::new` are `#[doc(hidden)] pub` constructors,
    // so the double can build the handle.
    out.push_str("    fn entity(&self, id: u32) -> Option<horizon_client::mod_api::EntityRef> {\n");
    out.push_str("        if id == 999 {\n");
    out.push_str("            None\n");
    out.push_str("        } else {\n");
    out.push_str("            Some(horizon_client::mod_api::EntityRef::new(\n");
    out.push_str("                horizon_client::mod_api::NativeWorldEpoch::new(1),\n");
    out.push_str("                id,\n");
    out.push_str("            ))\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    // The level lookup and the `level` helper share their name and
    // signature (`Host::level` IS `native_level`): one method records
    // the call and answers a handle for `"minecraft:overworld"` only,
    // so a test can drive both an unresolved dimension and a resolved
    // one. `LevelRef::new` and `NativeWorldEpoch::new` are
    // `#[doc(hidden)] pub` constructors, so the double can build the
    // handle.
    out.push_str(
        "    fn level(&self, dimension: &str) -> Option<horizon_client::mod_api::LevelRef> {\n",
    );
    out.push_str("        self.record(\"level\", &[dimension.to_string()]);\n");
    out.push_str("        if self.none || dimension != \"minecraft:overworld\" {\n");
    out.push_str("            None\n");
    out.push_str("        } else {\n");
    out.push_str("            Some(horizon_client::mod_api::LevelRef::new(\n");
    out.push_str("                horizon_client::mod_api::NativeWorldEpoch::new(1),\n");
    out.push_str("                \"minecraft:overworld\",\n");
    out.push_str("            ))\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    // The block-state lookups the bridge for `Option<BlockStateId>`
    // uses: `state_named` answers the canned name only (anything
    // else is unresolved, as an unknown state is), and `state_name`
    // answers its canned name back.
    out.push_str("    fn state_named(&self, name: &str) -> Option<horizon_client::mod_api::BlockStateId> {\n");
    out.push_str(
        "        (name == \"ruby:gen\").then_some(horizon_client::mod_api::BlockStateId(7))\n",
    );
    out.push_str("    }\n");
    out.push_str(
        "    fn state_name(&self, state: horizon_client::mod_api::BlockStateId) -> String {\n",
    );
    out.push_str("        let _ = state;\n");
    out.push_str("        \"ruby:gen\".to_owned()\n");
    out.push_str("    }\n");
    for row in rows {
        // `level`'s method is the lookup above, not a canned helper.
        if row.name != "level" {
            out.push_str(&gen_host_method(row));
        }
    }
    out.push_str("}\n");
    out
}

/// The synthetic conformance module's host: a local `SyntheticHost`
/// trait (the synthetic rows have no native `Host` methods) and the
/// `SyntheticDouble` recording double that implements it, with the
/// same canned values as `GenHost` (the same `gen_host_method` and
/// `canned` emitters). The bridge arms against it come from the same
/// `bridge_of`/`read_arm`/`action_arm` emitters as the real output.
fn synthetic_host(rows: &[HelperRow]) -> String {
    let mut out = String::new();
    out.push_str("/// The synthetic conformance host: one method per synthetic\n");
    out.push_str("/// row, so every emitter branch the enabled helpers alone\n");
    out.push_str("/// never reach is compiled and executed in the normal build.\n");
    out.push_str("pub(crate) trait SyntheticHost {\n");
    for row in rows {
        out.push_str(&format!("{};\n", host_method_sig(row)));
    }
    // The block-state lookups the bridge for `Option<BlockStateId>`
    // uses (the same pair, on the synthetic trait, as on `Host`):
    // `state_name` rides on a synthetic read returning one.
    out.push_str("    fn state_named(&self, name: &str) -> Option<BlockStateId>;\n");
    if rows.iter().any(|row| row.ret == "Option<BlockStateId>") {
        out.push_str("    fn state_name(&self, state: BlockStateId) -> String;\n");
    }
    out.push_str("}\n\n");
    out.push_str("struct SyntheticDouble {\n");
    out.push_str("    lines: Mutex<Vec<String>>,\n");
    out.push_str("    none: bool,\n");
    out.push_str("    refuse: bool,\n");
    out.push_str("}\n\n");
    out.push_str("impl SyntheticDouble {\n");
    out.push_str("    fn record(&self, name: &str, args: &[String]) {\n");
    out.push_str("        let line = if args.is_empty() {\n");
    out.push_str("            name.to_owned()\n");
    out.push_str("        } else {\n");
    out.push_str("            format!(\"{} {}\", name, args.join(\" \"))\n");
    out.push_str("        };\n");
    out.push_str("        self.lines\n");
    out.push_str("            .lock()\n");
    out.push_str("            .unwrap()\n");
    out.push_str("            .push(line);\n");
    out.push_str("    }\n");
    out.push_str("    fn logged(&self) -> Vec<String> {\n");
    out.push_str("        self.lines.lock().unwrap().clone()\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out.push_str("impl SyntheticHost for SyntheticDouble {\n");
    for row in rows {
        out.push_str(&gen_host_method(row));
    }
    out.push_str("    fn state_named(&self, name: &str) -> Option<BlockStateId> {\n");
    out.push_str("        (name == \"ruby:gen\").then_some(BlockStateId(7))\n");
    out.push_str("    }\n");
    if rows.iter().any(|row| row.ret == "Option<BlockStateId>") {
        out.push_str("    fn state_name(&self, state: BlockStateId) -> String {\n");
        out.push_str("        let _ = state;\n");
        out.push_str("        \"ruby:gen\".to_owned()\n");
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

/// The fixed list of synthetic rows: between them they exercise every
/// emitter branch no enabled helper reaches (the `Direction`
/// argument, tuple arguments and tuple returns with `f32`, `f64`,
/// `i32`, `[f64; 3]`, `[i32; 3]`, `String` and `Option<&str>`
/// elements, a unit action, a `u128` read). The kinds are enumerated
/// by `arg_kind`/`ret_kind`/`tuple_arg_elem_kind`/
/// `tuple_ret_elem_kind`; the coverage test fails when a kind is
/// covered by neither an enabled helper nor one of these rows.
fn synthetic_rows() -> Vec<HelperRow> {
    fn arg(name: &str, rust: &str, direction: bool) -> super::lua_helper_model::Arg {
        super::lua_helper_model::Arg {
            name: name.to_owned(),
            rust: rust.to_owned(),
            player: false,
            entity: false,
            direction,
            level: false,
        }
    }
    fn row(
        name: &str,
        kind: GenKind,
        args: Vec<super::lua_helper_model::Arg>,
        ret: &str,
        receiver_mut: bool,
    ) -> HelperRow {
        HelperRow {
            name: name.to_owned(),
            target: format!("horizon_client::game::native_{name}"),
            class: HelperClass::Generated(kind),
            args,
            ret: ret.to_owned(),
            doc: String::new(),
            api: None,
            receiver: None,
            receiver_mut,
            ret_borrowed: false,
            ret_static: false,
        }
    }
    vec![
        row(
            "synthetic_direction_probe",
            GenKind::Action,
            vec![arg("face", "Direction", true)],
            "bool",
            true,
        ),
        row(
            "synthetic_float_pair",
            GenKind::Action,
            vec![arg("pair", "Option<(f32, f64)>", false)],
            "bool",
            true,
        ),
        row(
            "synthetic_int_pair",
            GenKind::Action,
            vec![arg("pair", "Option<(i32, i32)>", false)],
            "bool",
            true,
        ),
        row(
            "synthetic_vec_pair",
            GenKind::Action,
            vec![arg("pair", "Option<([f64; 3], [i32; 3])>", false)],
            "bool",
            true,
        ),
        row(
            "synthetic_text_pair",
            GenKind::Action,
            vec![arg("pair", "Option<(&str, Option<&str>)>", false)],
            "bool",
            true,
        ),
        row(
            "synthetic_unit_action",
            GenKind::Action,
            Vec::new(),
            "unit",
            true,
        ),
        row(
            "synthetic_state_probe",
            GenKind::Action,
            vec![arg("state", "Option<BlockStateId>", false)],
            "bool",
            true,
        ),
        row(
            "synthetic_mixed_read",
            GenKind::Read,
            Vec::new(),
            "Option<(bool, f64)>",
            false,
        ),
        row(
            "synthetic_vec_read",
            GenKind::Read,
            Vec::new(),
            "Option<([f64; 3], [i32; 3])>",
            false,
        ),
        row(
            "synthetic_uuid_read",
            GenKind::Read,
            Vec::new(),
            "Option<u128>",
            false,
        ),
    ]
}

/// One `GenHost` method: it records the arguments it was handed (a
/// player as its uuid number, a string as its text, an int as the
/// saturated value, a float as its digits, a bool as `true`/`false`,
/// an entity as its id, a position as its `{x, y, z}` triple) and
/// returns a canned value (a read's canned value, or `None` when the
/// host is in its `none` mode).
/// One host method's signature line (without the body or the
/// trailing `;`/`{`), shared by the `GenHost` method and the
/// synthetic trait's declaration.
fn host_method_sig(row: &HelperRow) -> String {
    let mut params = Vec::new();
    for arg in &row.args {
        let a = match arg.rust.as_str() {
            "u128" => format!("{}: u128", arg.name),
            "&str" => format!("{}: &str", arg.name),
            "String" => format!("{}: String", arg.name),
            "bool" => format!("{}: bool", arg.name),
            ty => format!("{}: {ty}", arg.name),
        };
        params.push(a);
    }
    let sig_params = params.join(", ");
    // A unit return is no return type at all (`-> ()` would trip the
    // `unused_unit` lint).
    let ret = if row.ret == "unit" {
        String::new()
    } else if row.ret_borrowed {
        // `&'static str` borrows spell the same lifetime: the trait
        // builder copies the native signature's `'static`, and the
        // `impl` must match it (else the signature check rejects).
        if row.ret_static {
            match row.ret.as_str() {
                "String" => "&'static str".to_owned(),
                "Option<String>" => "Option<&'static str>".to_owned(),
                other => other.to_owned(),
            }
        } else {
            match row.ret.as_str() {
                "String" => "&str".to_owned(),
                "Option<String>" => "Option<&str>".to_owned(),
                other => other.to_owned(),
            }
        }
    } else {
        row.ret.clone()
    };
    let arrow = if ret.is_empty() {
        String::new()
    } else {
        format!(" -> {ret}")
    };
    format!(
        "    fn {}(&{}, {}){}",
        row.name,
        if row.receiver_mut { "mut self" } else { "self" },
        sig_params,
        arrow
    )
}

/// The canned payloads every test host answers, in one place: the
/// host methods render them (`gen_host_method`, `canned`) and the
/// emitted read tests assert them exactly (`value_asserts`), so a
/// bridge that returns a fixed value of the right Lua type fails a
/// test instead of passing it.
fn canned_string() -> &'static str {
    "ruby:gen"
}

/// The canned `Vec<String>` a string-list read answers (two distinct
/// names, in this order: the Lua array must keep it).
const STRING_LIST: [&str; 2] = ["alpha", "beta"];

fn canned_float() -> f64 {
    1.5
}

fn canned_f64_xyz() -> [f64; 3] {
    [1.5, 2.5, 3.5]
}

fn canned_i32_xyz() -> [i64; 3] {
    [3, 4, 5]
}

/// One canned item stack's fields, every one distinct (`ruby:gen`, 7,
/// 3), so a swapped field fails a test on either side.
fn canned_stack() -> (&'static str, u32, u32) {
    ("ruby:gen", 7, 3)
}

fn gen_host_method(row: &HelperRow) -> String {
    let out = format!("{} {{\n", host_method_sig(row));
    let mut out = out
        + &format!(
            "        self.record({:?}, &[{}]);\n",
            row.name,
            row.args
                .iter()
                .map(|a| match a.rust.as_str() {
                    "EntityRef" => format!("{}.id().to_string()", a.name),
                    "LevelRef" => format!("{}.dimension().to_string()", a.name),
                    // `Direction` has no `Display` (its text is its
                    // Debug variant name, `North`); recorded as Debug,
                    // like a position or a tuple.
                    "Direction" => format!("format!(\"{{:?}}\", {})", a.name),
                    _ if a.rust.starts_with("Option<(") => {
                        format!("format!(\"{{:?}}\", {})", a.name)
                    }
                    "[f64; 3]" | "[i32; 3]" => format!("format!(\"{{:?}}\", {})", a.name),
                    // Item, NBT and block-state values have no `Display`:
                    // recorded as Debug, like a position or a tuple.
                    "ItemStack"
                    | "Compound"
                    | "&Compound"
                    | "EquipmentSlot"
                    | "Option<BlockStateId>" => format!("format!(\"{{:?}}\", {})", a.name),
                    _ => format!("{}.to_string()", a.name),
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
    out.push_str(&match (&row.class, row.ret.as_str()) {
        // A unit method is empty after recording (a trailing `()`
        // would trip `unused_unit`).
        (_, "unit") => String::new(),
        (HelperClass::Generated(GenKind::Action), "bool") => "        !self.refuse\n".to_owned(),
        (_, "bool") => "        true\n".to_owned(),
        (_, "[f64; 3]") => format!("        {:?}\n", canned_f64_xyz()),
        (_, "[i32; 3]") => format!("        {:?}\n", canned_i32_xyz()),
        (_, "String") if row.ret_borrowed => format!("        {:?}\n", canned_string()),
        (_, "String") => format!("        {:?}.to_owned()\n", canned_string()),
        (_, "f64" | "f32") => format!("        {:?}\n", canned_float()),
        (_, inner) if inner.starts_with("Option<") => {
            let scalar = inner
                .strip_prefix("Option<")
                .unwrap()
                .strip_suffix('>')
                .unwrap();
            let canned_value = if scalar == "String" && row.ret_borrowed {
                format!("{:?}", canned_string())
            } else {
                canned(scalar)
            };
            return format!(
                "{out}        if self.none {{ None }} else {{ Some({}) }}\n    }}\n",
                canned_value
            );
        }
        (_, scalar) => return format!("{out}        {}\n    }}\n", canned(scalar)),
    });
    out.push_str("    }\n");
    out
}

/// One tuple element's canned value, per position `i`: the two
/// elements take DISTINCT values (1.5/2.5, 3/4, per-position
/// strings), so a swapped pair cannot pass a tuple-read test
/// (`tuple_field_asserts` asserts these exact values per field).
/// An `Option` is `Some`; a `u128` is the `STEVE_UUID` double.
fn canned_elem(elem: &str, i: usize) -> String {
    match elem {
        "Option<String>" | "Option<&str>" => format!("Some({:?}.to_owned())", tuple_str(1, i)),
        "String" | "&str" => format!("{:?}.to_owned()", tuple_str(1, i)),
        "f32" | "f64" => format!("{}", tuple_float(i)),
        "bool" => {
            if i == 0 {
                "true".to_owned()
            } else {
                "false".to_owned()
            }
        }
        "u128" => "STEVE_UUID".to_owned(),
        "[f64; 3]" => {
            let (x, y, z) = tuple_float_xyz(0);
            format!("[{x}, {y}, {z}]")
        }
        "[i32; 3]" => {
            let (x, y, z) = tuple_int_xyz(0);
            format!("[{x}, {y}, {z}]")
        }
        _ => format!("{}", tuple_int(i)),
    }
}

/// A canned value of a scalar, in the host double's terms.
fn canned(scalar: &str) -> String {
    match scalar {
        "bool" => "true".to_owned(),
        "String" => format!("{:?}.to_owned()", canned_string()),
        "u128" => "STEVE_UUID".to_owned(),
        "f32" | "f64" => format!("{:?}", canned_float()),
        "[f64; 3]" => format!("{:?}", canned_f64_xyz()),
        "[i32; 3]" => format!("{:?}", canned_i32_xyz()),
        "LevelRef" => "horizon_client::mod_api::LevelRef::new(horizon_client::mod_api::NativeWorldEpoch::new(1), \"minecraft:overworld\")".to_owned(),
        // One canned stack, every field distinct (`ruby:gen`, 7, 3), so
        // a swapped field fails a return test.
        "ItemStack" => {
            let (id, count, damage) = canned_stack();
            format!("ItemStack {{ item: {id:?}.into(), count: {count}, damage: {damage}, extra: None }}")
        }
        // One canned block state.
        "BlockStateId" => "BlockStateId(7)".to_owned(),
        // Two distinct strings, so a reordered list fails a return test.
        "Vec<String>" => format!(
            "vec![{:?}.to_owned(), {:?}.to_owned()]",
            STRING_LIST[0], STRING_LIST[1]
        ),
        _ if scalar.starts_with('(') => {
            let elems = crate::lua_helper_model::tuple_elems(scalar);
            let values: Vec<String> = elems
                .iter()
                .enumerate()
                .map(|(i, e)| canned_elem(e, i))
                .collect();
            format!("({})", values.join(", "))
        }
        _ => "3".to_owned(),
    }
}

/// `crates/event-horizon-lua/src/generated_helpers.rs` tests: one test
/// per emitted helper (the arguments reach the host exactly, a read
/// returns its exact canned value, and nil for None; an
/// unresolvable player or entity is a silent no-op/nil without calling
/// the host; a malformed script argument is an error naming it) plus
/// the shared client/draw raises and the drain test.
fn tests_of(rows: &[HelperRow], backend: &Backend) -> String {
    let real = backend.host_ty == "Host";
    let mut out = String::new();
    out.push_str("// Generated by `cargo run -p xtask -- lua-helpers`: do not edit.\n");
    out.push_str("// One test per emitted helper, over the `GenHost` recording double: the\n");
    out.push_str("// arguments reach the host exactly, a read returns its exact canned\n");
    out.push_str("// value (and nil for None), an unresolvable player or\n");
    out.push_str("// entity is a silent no-op/nil without calling the host, a malformed\n");
    out.push_str("// script argument is an error naming it, and the names raise in a client\n");
    out.push_str("// state and a draw hook. The drain test drives the real queue through a\n");
    out.push_str("// hook dispatch.\n\n");
    // The synthetic file is one module carrying its own bridge, so it
    // takes no `generated_helpers` import and no `Host`/`LuaMod`
    // imports (its double implements its local trait).
    if real {
        out.push_str("use crate::generated_helpers::{\n");
        out.push_str(
            "    GENERATED_ACTIONS, GENERATED_CLIENT_ACTIONS, GENERATED_CLIENT_READS, GENERATED_READS,\n    generated_read, run_generated_action,\n",
        );
        out.push_str("};\n");
        out.push_str("use crate::lua_mods::{LuaMod, LuaSide};\n");
        out.push_str("use horizon_client::mod_api::{EntityRef, Host, LevelRef};\n");
    }
    if real && rows.iter().any(|row| row.args.iter().any(|a| a.direction)) {
        out.push_str("use horizon_client::mod_api::Direction;\n");
    }
    // The synthetic file already carries the bridge's `mlua` import.
    if real {
        out.push_str("use mlua::{Lua, Table};\n");
    }
    // The `GenHost` methods spell the converted types in their
    // signatures (and their canned returns name them), so import each
    // one a row uses (like `Direction` above).
    if real {
        let uses = |ty: &str| {
            rows.iter()
                .any(|row| row.ret.contains(ty) || row.args.iter().any(|a| a.rust.contains(ty)))
        };
        if uses("ItemStack") {
            out.push_str("use horizon_client::mod_api::ItemStack;\n");
        }
        if uses("Compound") {
            out.push_str("use horizon_client::mod_api::Compound;\n");
        }
        if uses("EquipmentSlot") {
            out.push_str("use horizon_client::mod_api::EquipmentSlot;\n");
        }
        if uses("BlockStateId") {
            out.push_str("use horizon_client::mod_api::BlockStateId;\n");
        }
    }
    out.push_str("use std::sync::Mutex;\n\n");
    out.push_str("const STEVE_UUID: u128 = 0x123e4567_e89b_12d3_a456_426614174000;\n\n");
    if real {
        out.push_str(&gen_host(rows));
    }
    out.push_str("\nfn args(lua: &Lua, values: &[mlua::Value]) -> Table {\n");
    out.push_str("    let t = lua.create_table().unwrap();\n");
    out.push_str("    for (i, v) in values.iter().enumerate() {\n");
    out.push_str("        t.set(i + 1, v.clone()).unwrap();\n");
    out.push_str("    }\n");
    out.push_str("    t\n");
    out.push_str("}\n");
    out.push_str("\nfn rest(_lua: &Lua, values: &[mlua::Value]) -> mlua::MultiValue {\n");
    out.push_str("    values.iter().cloned().collect()\n");
    out.push_str("}\n");
    out.push_str("\nfn lua() -> Lua {\n");
    out.push_str("    Lua::new()\n");
    out.push_str("}\n");
    out.push_str(&format!(
        "\nfn {}(none: bool, refuse: bool) -> {} {{\n",
        backend.host_new, backend.double_ty
    ));
    out.push_str(&format!("    {} {{\n", backend.double_ty));
    out.push_str("        lines: Mutex::new(Vec::new()),\n");
    out.push_str("        none,\n");
    out.push_str("        refuse,\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    if real {
        out.push_str(hmod_helpers());
    }
    for row in rows {
        // A bound helper's Lua call takes no mod id, so its generated test
        // (built from the script arguments) cannot be written here; its
        // tests are hand-written in `lua_mods.rs` (issue 61).
        if is_bound_mod_id(&row.name) {
            continue;
        }
        out.push_str(&helper_test(row, backend));
    }
    if real {
        out.push_str(&shared_tests(rows));
    } else {
        out.push_str(&synthetic_shared_tests(rows));
    }
    out
}

/// Whether an argument is an integer the saturation edge pins.
fn is_int(arg: &super::lua_helper_model::Arg) -> bool {
    matches!(
        arg.rust.as_str(),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
    )
}

/// The one normal call of every helper: the player as its uuid text
/// (which `uuid_of` reads back), the entity as its id (7, 8, ...),
/// every other argument a distinct value within the helper (the
/// first bool `true`, the second `false`, the third `true`, ...;
/// strings `ruby:gen1`, `ruby:gen2`, ...; the first int 3, the next
/// 4, ...; the first float 1.5, the next 2.5, ...; three array
/// components walking the same float/int counters, so a swap of two
/// same-type reads changes the recorded line) — and the line the
/// host records for it (the uuid as its number, an entity as its id,
/// a position as its debug triple). The resolved level (`minecraft:overworld`,
/// plus the `level <dimension>` line its lookup records first) and the
/// resolved direction short names it as a string.
fn normal_case(row: &HelperRow) -> (Vec<String>, Vec<String>) {
    let mut literals = Vec::new();
    let mut recorded = vec![row.name.clone()];
    let mut lookups = Vec::new();
    let mut strings = 0;
    let mut bools = 0;
    let mut ints = 0;
    let mut floats = 0;
    let mut entities = 0;
    let mut directions = 0;
    let mut tuples = 0;
    for arg in &row.args {
        match arg.rust.as_str() {
            "Direction" => {
                directions += 1;
                let (text, variant) = [
                    ("north", "North"),
                    ("south", "South"),
                    ("west", "West"),
                    ("east", "East"),
                    ("up", "Up"),
                    ("down", "Down"),
                ][(directions - 1) % 6];
                literals.push(format!(
                    "mlua::Value::String(lua.create_string({text:?}).unwrap())"
                ));
                recorded.push(variant.to_owned());
            }
            "LevelRef" => {
                literals.push(
                    "mlua::Value::String(lua.create_string(\"minecraft:overworld\").unwrap())"
                        .to_owned(),
                );
                lookups.push("level minecraft:overworld".to_owned());
                recorded.push("minecraft:overworld".to_owned());
            }
            _ if arg.rust.starts_with("Option<(") => {
                tuples += 1;
                literal_tuple(&mut literals, &arg.rust, tuples);
                recorded.push(recorded_tuple(&arg.rust, tuples));
            }
            "u128" => {
                literals.push("mlua::Value::String(lua.create_string(\"123e4567-e89b-12d3-a456-426614174000\").unwrap())".to_owned());
                recorded.push("24249434048109030647017182301789831168".to_owned());
            }
            "EntityRef" => {
                entities += 1;
                let value = 6 + entities as i64;
                literals.push(format!("mlua::Value::Integer({value})"));
                recorded.push(value.to_string());
            }
            "[f64; 3]" => {
                floats += 1;
                let x = 1.5 + (floats - 1) as f64;
                floats += 1;
                let y = 1.5 + (floats - 1) as f64;
                floats += 1;
                let z = 1.5 + (floats - 1) as f64;
                literals.push(format!(
                    "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", {x}).unwrap();\n        t.set(\"y\", {y}).unwrap();\n        t.set(\"z\", {z}).unwrap();\n        mlua::Value::Table(t)\n    }}",
                ));
                recorded.push(format!("[{x}, {y}, {z}]"));
            }
            "ItemStack" => {
                let (id, count, damage) = canned_stack();
                literals.push(format!(
                    "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"id\", {id:?}).unwrap();\n        t.set(\"count\", {count}).unwrap();\n        t.set(\"damage\", {damage}).unwrap();\n        mlua::Value::Table(t)\n    }}",
                ));
                recorded.push(format!(
                    "ItemStack {{ item: {id:?}, count: {count}, damage: {damage}, extra: None }}"
                ));
            }
            "Compound" | "&Compound" => {
                literals.push(
                    "{\n        let t = lua.create_table().unwrap();\n        t.set(\"a\", 1).unwrap();\n        t.set(\"b\", 2).unwrap();\n        mlua::Value::Table(t)\n    }"
                    .to_owned(),
                );
                recorded.push("{JavaString([97]): Int(1), JavaString([98]): Int(2)}".to_owned());
            }
            "EquipmentSlot" => {
                literals
                    .push("mlua::Value::String(lua.create_string(\"head\").unwrap())".to_owned());
                recorded.push("Head".to_owned());
            }
            "Option<BlockStateId>" => {
                literals.push(
                    "mlua::Value::String(lua.create_string(\"ruby:gen\").unwrap())".to_owned(),
                );
                recorded.push("Some(BlockStateId(7))".to_owned());
            }
            "[i32; 3]" => {
                ints += 1;
                let x: i64 = 2 + ints as i64;
                ints += 1;
                let y: i64 = 2 + ints as i64;
                ints += 1;
                let z: i64 = 2 + ints as i64;
                literals.push(format!(
                    "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", {x}).unwrap();\n        t.set(\"y\", {y}).unwrap();\n        t.set(\"z\", {z}).unwrap();\n        mlua::Value::Table(t)\n    }}",
                ));
                recorded.push(format!("[{x}, {y}, {z}]"));
            }
            "&str" | "String" => {
                strings += 1;
                // The `level` helper names the level it resolves: its
                // dimension is the mock's one resolvable dimension.
                let text = if row.name == "level" && arg.name == "dimension" {
                    "minecraft:overworld".to_owned()
                } else {
                    format!("ruby:gen{strings}")
                };
                literals.push(format!(
                    "mlua::Value::String(lua.create_string({text:?}).unwrap())"
                ));
                recorded.push(text);
            }
            "bool" => {
                bools += 1;
                let value = bools % 2 == 1;
                literals.push(format!("mlua::Value::Boolean({value})"));
                recorded.push(value.to_string());
            }
            "f32" | "f64" => {
                floats += 1;
                let value = 1.5 + (floats - 1) as f64;
                literals.push(format!("mlua::Value::Number({value})"));
                recorded.push(format!("{value}"));
            }
            _ => {
                ints += 1;
                let value: i64 = 2 + ints as i64;
                literals.push(format!("mlua::Value::Integer({value})"));
                recorded.push(value.to_string());
            }
        }
    }
    lookups.extend([recorded.join(" ")]);
    (literals, lookups)
}

/// The two tuple elements of one test take DISTINCT values, per
/// position `i` (1.5/2.5, 3/4, per-position strings, offset
/// position triples), so a swapped pair cannot pass a tuple-read
/// test; `canned_elem` answers the same for tuple returns.
fn tuple_float(i: usize) -> f64 {
    1.5 + i as f64
}

fn tuple_int(i: usize) -> i64 {
    3 + i as i64
}

fn tuple_float_xyz(i: usize) -> (f64, f64, f64) {
    let b = 10.0 * i as f64;
    (1.5 + b, 2.5 + b, 3.5 + b)
}

fn tuple_int_xyz(i: usize) -> (i64, i64, i64) {
    let b = 10 * i as i64;
    (3 + b, 4 + b, 5 + b)
}

fn tuple_str(nth: usize, i: usize) -> String {
    format!("ruby:tup{nth}_{i}")
}

fn tuple_opt_str(nth: usize, i: usize) -> String {
    format!("ruby:tup{nth}_{i}b")
}

/// The Lua table literal one optional tuple argument takes (its `Some`
/// shape, see `tuple_shape`), pushing it onto `literals` and the
/// Debug text the host records onto the recorded call.
fn literal_tuple(literals: &mut Vec<String>, normalized: &str, nth: usize) {
    let inner = normalized
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(normalized);
    let shape = super::lua_helper_model::tuple_shape(inner);
    let mut build = String::from("{\n        let t = lua.create_table().unwrap();\n");
    for (i, elem) in shape.elems.iter().enumerate() {
        let set = |expr: String| -> String {
            if shape.named {
                format!(
                    "t.set({:?}, {expr}).unwrap();\n",
                    shape.fields.get(i).map(String::as_str).unwrap_or("")
                )
            } else {
                format!("t.set({}, {expr}).unwrap();\n", i + 1)
            }
        };
        match elem.as_str() {
            "String" | "&str" => {
                let text = tuple_str(nth, i);
                build.push_str(&set(format!("{text:?}")));
            }
            "Option<String>" | "Option<&str>" => {
                let text = tuple_opt_str(nth, i);
                build.push_str(&set(format!("{text:?}")));
            }
            "[i32; 3]" => {
                let (x, y, z) = tuple_int_xyz(i);
                let p = format!(
                    "{{ let p = lua.create_table().unwrap(); p.set(\"x\", {x}).unwrap(); p.set(\"y\", {y}).unwrap(); p.set(\"z\", {z}).unwrap(); p }}"
                );
                build.push_str(&set(p));
            }
            "[f64; 3]" => {
                let (x, y, z) = tuple_float_xyz(i);
                let p = format!(
                    "{{ let p = lua.create_table().unwrap(); p.set(\"x\", {x}).unwrap(); p.set(\"y\", {y}).unwrap(); p.set(\"z\", {z}).unwrap(); p }}"
                );
                build.push_str(&set(p));
            }
            "f32" | "f64" => {
                build.push_str(&set(format!("{}", tuple_float(i))));
            }
            _ => {
                build.push_str(&set(format!("{}", tuple_int(i))));
            }
        }
    }
    build.push_str("        mlua::Value::Table(t)\n    }");
    literals.push(build);
}

/// The Debug text the host records for one optional tuple argument this
/// call: `Some(("ruby:tup1_0", [3, 4, 5]))` and friends (the same
/// distinct values `literal_tuple` sends, formatted as the host's
/// Debug of the value is).
fn recorded_tuple(normalized: &str, nth: usize) -> String {
    let inner = normalized
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(normalized);
    let shape = super::lua_helper_model::tuple_shape(inner);
    let parts: Vec<String> = shape
        .elems
        .iter()
        .enumerate()
        .map(|(i, elem)| match elem.as_str() {
            "String" | "&str" => {
                let text = tuple_str(nth, i);
                format!("{text:?}")
            }
            "Option<String>" | "Option<&str>" => {
                let text = tuple_opt_str(nth, i);
                format!("Some({text:?})")
            }
            "[i32; 3]" => {
                let (x, y, z) = tuple_int_xyz(i);
                format!("[{x}, {y}, {z}]")
            }
            "[f64; 3]" => {
                let (x, y, z) = tuple_float_xyz(i);
                format!("[{x}, {y}, {z}]")
            }
            "f32" | "f64" => format!("{}", tuple_float(i)),
            _ => format!("{}", tuple_int(i)),
        })
        .collect();
    format!("Some(({}))", parts.join(", "))
}

/// The saturation edges of a helper's integer arguments: -1 and 300,
/// each pushed through the bridge and recorded as the saturated value
/// (`300` for a `u32`, `255` for a `u8`, `-1` for a signed type, and so
/// on — as the saturating cast hands it to the host). Entity ids and
/// position tables do not saturate: they ride along at their normal
/// values (an entity id's bad values error, and an `[i32; 3]`
/// component's error; those are pinned in the per-helper error blocks).
fn edge_cases(row: &HelperRow) -> Vec<(Vec<String>, Vec<String>)> {
    let mut out = Vec::new();
    for (value, _) in [(-1i64, ""), (300i64, "")] {
        let mut literals = Vec::new();
        let mut recorded = vec![row.name.clone()];
        let mut lookups = Vec::new();
        let mut tuples = 0;
        for arg in &row.args {
            match arg.rust.as_str() {
                "Direction" => {
                    literals.push(
                        "mlua::Value::String(lua.create_string(\"north\").unwrap())".to_owned(),
                    );
                    recorded.push("North".to_owned());
                }
                "LevelRef" => {
                    literals.push(
                        "mlua::Value::String(lua.create_string(\"minecraft:overworld\").unwrap())"
                            .to_owned(),
                    );
                    lookups.push("level minecraft:overworld".to_owned());
                    recorded.push("minecraft:overworld".to_owned());
                }
                _ if arg.rust.starts_with("Option<(") => {
                    tuples += 1;
                    literal_tuple(&mut literals, &arg.rust, tuples);
                    recorded.push(recorded_tuple(&arg.rust, tuples));
                }
                "u128" => {
                    literals.push("mlua::Value::String(lua.create_string(\"123e4567-e89b-12d3-a456-426614174000\").unwrap())".to_owned());
                    recorded.push("24249434048109030647017182301789831168".to_owned());
                }
                "EntityRef" => {
                    literals.push("mlua::Value::Integer(7)".to_owned());
                    recorded.push("7".to_owned());
                }
                "[f64; 3]" => {
                    literals.push(format!("{}\n", float_table(1.5, 2.5, 3.5)));
                    recorded.push("[1.5, 2.5, 3.5]".to_owned());
                }
                "[i32; 3]" => {
                    literals.push(format!("{}\n", int_table(3, 4, 5)));
                    recorded.push("[3, 4, 5]".to_owned());
                }
                "&str" | "String" => {
                    literals.push(
                        "mlua::Value::String(lua.create_string(\"ruby:gen\").unwrap())".to_owned(),
                    );
                    recorded.push("ruby:gen".to_owned());
                }
                "bool" => {
                    literals.push("mlua::Value::Boolean(true)".to_owned());
                    recorded.push("true".to_owned());
                }
                "f32" | "f64" => {
                    literals.push("mlua::Value::Number(1.5)".to_owned());
                    recorded.push("1.5".to_owned());
                }
                "ItemStack" => {
                    let (id, count, damage) = canned_stack();
                    literals.push(format!(
                        "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"id\", {id:?}).unwrap();\n        t.set(\"count\", {count}).unwrap();\n        t.set(\"damage\", {damage}).unwrap();\n        mlua::Value::Table(t)\n    }}",
                    ));
                    recorded.push(format!(
                        "ItemStack {{ item: {id:?}, count: {count}, damage: {damage}, extra: None }}"
                    ));
                }
                "Compound" | "&Compound" => {
                    literals.push(
                        "{\n        let t = lua.create_table().unwrap();\n        t.set(\"a\", 1).unwrap();\n        t.set(\"b\", 2).unwrap();\n        mlua::Value::Table(t)\n    }"
                            .to_owned(),
                    );
                    recorded
                        .push("{JavaString([97]): Int(1), JavaString([98]): Int(2)}".to_owned());
                }
                "EquipmentSlot" => {
                    literals.push(
                        "mlua::Value::String(lua.create_string(\"head\").unwrap())".to_owned(),
                    );
                    recorded.push("Head".to_owned());
                }
                "Option<BlockStateId>" => {
                    literals.push(
                        "mlua::Value::String(lua.create_string(\"ruby:gen\").unwrap())".to_owned(),
                    );
                    recorded.push("Some(BlockStateId(7))".to_owned());
                }
                ty => {
                    literals.push(format!("mlua::Value::Integer({value})"));
                    recorded.push(saturate(value, ty).to_string());
                }
            }
        }
        lookups.extend([recorded.join(" ")]);
        out.push((literals, lookups));
    }
    out
}

/// A `{x, y, z}` table-building block (a Lua value expression) for the
/// test literals, as `normal_case` builds them.
fn float_table(x: f64, y: f64, z: f64) -> String {
    format!(
        "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", {x}).unwrap();\n        t.set(\"y\", {y}).unwrap();\n        t.set(\"z\", {z}).unwrap();\n        mlua::Value::Table(t)\n    }}",
    )
}

/// An `{x, y, z}` table-building block for integer components.
fn int_table(x: i64, y: i64, z: i64) -> String {
    format!(
        "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", {x}).unwrap();\n        t.set(\"y\", {y}).unwrap();\n        t.set(\"z\", {z}).unwrap();\n        mlua::Value::Table(t)\n    }}",
    )
}

/// What the bridge's saturating cast hands the host for a Lua number
/// `value` into a Rust integer `ty`: the value clamped into range.
/// Computed here, never copied from the emitted code.
fn saturate(value: i64, ty: &str) -> i64 {
    let (min, max): (i128, i128) = match ty {
        "i8" => (i8::MIN as i128, i8::MAX as i128),
        "i16" => (i16::MIN as i128, i16::MAX as i128),
        "i32" => (i32::MIN as i128, i32::MAX as i128),
        "i64" => (i64::MIN as i128, i64::MAX as i128),
        "i128" => (i128::MIN, i128::MAX),
        "isize" => (isize::MIN as i128, isize::MAX as i128),
        "u8" => (u8::MIN as i128, u8::MAX as i128),
        "u16" => (u16::MIN as i128, u16::MAX as i128),
        "u32" => (u32::MIN as i128, u32::MAX as i128),
        "u64" => (u64::MIN as i128, u64::MAX as i128),
        "usize" => (usize::MIN as i128, usize::MAX as i128),
        _ => return value,
    };
    let value = value as i128;
    let clamped = value.clamp(min, max);
    // The recorded string is the host's decimal text; for a wide type
    // the value still fits the i64 the test's expected line names.
    clamped as i64
}

/// The per-helper error blocks for entity and position arguments: an
/// unresolvable entity (id 999) calls nothing (a read answers nil), a
/// negative, fractional or non-numeric entity id is a script error
/// naming the argument, a NaN or missing component (or a fractional
/// or out-of-range `[i32; 3]` component) likewise.
fn residual_blocks(
    row: &HelperRow,
    literals: &[String],
    kind: GenKind,
    backend: &Backend,
) -> String {
    let host_new = backend.host_new;
    let read_fn = backend.read_fn;
    let action_fn = backend.action_fn;
    let mut out = String::new();
    let call = |lits: &Vec<String>| match kind {
        GenKind::Action => format!(
            "        let t = args(&lua, &[{}]);\n        let err = {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap_err();",
            lits.join(", "),
            row.name
        ),
        GenKind::Read => format!(
            "        let err = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap_err();",
            row.name,
            lits.join(", ")
        ),
    };
    let ok_call = |lits: &Vec<String>| match kind {
        GenKind::Action => format!(
            "        let t = args(&lua, &[{}]);\n        {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap();",
            lits.join(", "),
            row.name
        ),
        GenKind::Read => format!(
            "        let v = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();",
            row.name,
            lits.join(", ")
        ),
    };
    // An ok call whose answer nobody reads (the variant matrices
    // below assert on the host log, not on `v`), so it binds `_v`.
    let discard_call = |lits: &Vec<String>| match kind {
        GenKind::Action => ok_call(lits),
        GenKind::Read => format!(
            "        let _v = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();",
            row.name,
            lits.join(", ")
        ),
    };
    let host_decl = |b: &mut String| match kind {
        GenKind::Action => b.push_str(&format!(
            "        let mut host = {host_new}(false, false);\n"
        )),
        GenKind::Read => b.push_str(&format!("        let h = {host_new}(false, false);\n")),
    };
    let err_block = |lits: Vec<String>, arg: &str| {
        let mut s = String::from("    {\n");
        host_decl(&mut s);
        s.push_str(&call(&lits));
        s.push_str(&format!(
            "\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
            format!("{}.{}", row.name, arg)
        ));
        s
    };
    // An unresolvable entity calls nothing.
    if let Some(pos) = row.args.iter().position(|a| a.entity) {
        let mut ghost = literals.to_vec();
        ghost[pos] = "mlua::Value::Integer(999)".to_owned();
        let mut s = String::from("    {\n");
        host_decl(&mut s);
        s.push_str(&ok_call(&ghost));
        s.push_str(match kind {
            GenKind::Action => {
                "\n        assert!(host.logged().is_empty(), \"an unresolvable entity calls nothing\");\n    }\n"
            }
            GenKind::Read => {
                "\n        assert!(matches!(v, mlua::Value::Nil));\n        assert!(h.logged().is_empty());\n    }\n"
            }
        });
        out.push_str(&s);
    }
    // A negative, fractional or non-numeric entity id is an error.
    if let Some(pos) = row.args.iter().position(|a| a.entity) {
        let arg = row.args[pos].name.clone();
        let mut bad = literals.to_vec();
        for value in [
            "mlua::Value::Integer(-1)",
            "mlua::Value::Number(1.5)",
            "mlua::Value::Table(lua.create_table().unwrap())",
        ] {
            bad[pos] = value.to_owned();
            out.push_str(&err_block(bad.clone(), &arg));
        }
    }
    // A NaN or missing `[f64; 3]` component is an error.
    if let Some(pos) = row.args.iter().position(|a| a.rust == "[f64; 3]") {
        let arg = row.args[pos].name.clone();
        for table in [
            "{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", f64::NAN).unwrap();\n        t.set(\"y\", 2.5).unwrap();\n        t.set(\"z\", 3.5).unwrap();\n        mlua::Value::Table(t)\n    }",
            "{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", 1.5).unwrap();\n        t.set(\"y\", 2.5).unwrap();\n        mlua::Value::Table(t)\n    }",
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = table.to_owned();
            out.push_str(&err_block(bad, &arg));
        }
    }
    // A fractional or out-of-range `[i32; 3]` component is an error
    // (an `[i32; 3]` argument errors rather than saturating).
    if let Some(pos) = row.args.iter().position(|a| a.rust == "[i32; 3]") {
        let arg = row.args[pos].name.clone();
        for table in [
            "{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", 1.5).unwrap();\n        t.set(\"y\", 4).unwrap();\n        t.set(\"z\", 5).unwrap();\n        mlua::Value::Table(t)\n    }",
            "{\n        let t = lua.create_table().unwrap();\n        t.set(\"x\", 3000000000i64).unwrap();\n        t.set(\"y\", 4).unwrap();\n        t.set(\"z\", 5).unwrap();\n        mlua::Value::Table(t)\n    }",
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = table.to_owned();
            out.push_str(&err_block(bad, &arg));
        }
    }
    // An unresolved dimension calls nothing past the lookup (a read
    // answers nil, an action is a silent no-op).
    if let Some(pos) = row.args.iter().position(|a| a.level) {
        let mut ghost = literals.to_vec();
        ghost[pos] =
            "mlua::Value::String(lua.create_string(\"minecraft:nowhere\").unwrap())".to_owned();
        let mut s = String::from("    {\n");
        host_decl(&mut s);
        s.push_str(&ok_call(&ghost));
        s.push_str(match kind {
            GenKind::Action => {
                "\n        assert_eq!(host.logged(), [\"level minecraft:nowhere\"], \"an unresolved level calls nothing past the lookup\");\n    }\n"
            }
            GenKind::Read => {
                "\n        assert!(matches!(v, mlua::Value::Nil));\n        assert_eq!(h.logged(), [\"level minecraft:nowhere\"]);\n    }\n"
            }
        });
        out.push_str(&s);
    }
    // A dimension-id string (`dimension: &str`): an empty or
    // non-string value is an error naming the argument, and the
    // host is never called.
    if let Some(pos) = row.args.iter().position(is_dimension_arg) {
        let arg = row.args[pos].name.clone();
        for value in [
            "mlua::Value::String(lua.create_string(\"\").unwrap())",
            "mlua::Value::Integer(3)",
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = value.to_owned();
            let mut s = String::from("    {\n");
            host_decl(&mut s);
            s.push_str(&call(&bad));
            s.push_str(&format!(
                "\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n",
                format!("{}.{}", row.name, arg)
            ));
            s.push_str(match kind {
                GenKind::Action => {
                    "        assert!(host.logged().is_empty(), \"an empty dimension calls nothing\");\n    }\n"
                }
                GenKind::Read => {
                    "        assert!(h.logged().is_empty(), \"an empty dimension calls nothing\");\n    }\n"
                }
            });
            out.push_str(&s);
        }
    }
    // A non-string or empty dimension is an error naming the argument.
    if let Some(pos) = row.args.iter().position(|a| a.level) {
        let arg = row.args[pos].name.clone();
        for value in [
            "mlua::Value::Integer(3)",
            "mlua::Value::String(lua.create_string(\"\").unwrap())",
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = value.to_owned();
            out.push_str(&err_block(bad, &arg));
        }
    }
    // The six direction names reach the host as the right variant; a
    // misspelled name or a non-string is an error naming the argument.
    if let Some(pos) = row.args.iter().position(|a| a.direction) {
        let arg = row.args[pos].name.clone();
        for (text, variant) in [
            ("down", "Down"),
            ("up", "Up"),
            ("north", "North"),
            ("south", "South"),
            ("west", "West"),
            ("east", "East"),
        ] {
            let mut lit = literals.to_vec();
            lit[pos] = format!("mlua::Value::String(lua.create_string({text:?}).unwrap())");
            let mut s = String::from("    {\n");
            host_decl(&mut s);
            s.push_str(&ok_call(&lit));
            s.push_str(&format!(
                "\n        let last = {log}.logged().last().cloned().unwrap_or_default();\n        assert!(last.contains({variant:?}), \"{{last}}\");\n    }}\n",
                log = match kind {
                    GenKind::Action => "host",
                    GenKind::Read => "h",
                },
                variant = variant,
            ));
            out.push_str(&s);
        }
        for value in [
            "mlua::Value::String(lua.create_string(\"upwards\").unwrap())".to_string(),
            "mlua::Value::Integer(3)".to_owned(),
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = value;
            out.push_str(&err_block(bad, &arg));
        }
    }
    // A malformed tuple argument (an empty table) is an error naming
    // the argument.
    if let Some(pos) = row.args.iter().position(|a| a.rust.starts_with("Option<(")) {
        let arg = row.args[pos].name.clone();
        let mut bad = literals.to_vec();
        bad[pos] = "mlua::Value::Table(lua.create_table().unwrap())".to_owned();
        out.push_str(&err_block(bad, &arg));
    }
    // The converted types' bad shapes: an error naming the argument,
    // and the host is never called (like the dimension block above).
    let bad_block = |lits: Vec<String>, arg: &str| {
        let mut s = String::from("    {\n");
        host_decl(&mut s);
        s.push_str(&call(&lits));
        s.push_str(&format!(
            "\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n",
            format!("{}.{}", row.name, arg)
        ));
        s.push_str(match kind {
            GenKind::Action => {
                "        assert!(host.logged().is_empty(), \"a bad argument calls nothing\");\n    }\n"
            }
            GenKind::Read => {
                "        assert!(h.logged().is_empty(), \"a bad argument calls nothing\");\n    }\n"
            }
        });
        s
    };
    // An item table with a fractional or out-of-range count, a
    // negative damage, a non-string id or any other key is an error
    // naming the argument, and the host is never called.
    if let Some(pos) = row.args.iter().position(|a| a.rust == "ItemStack") {
        let arg = row.args[pos].name.clone();
        let table = |id: &str, count: &str, damage: &str, extra: &str| {
            format!(
                "{{\n        let t = lua.create_table().unwrap();\n        t.set(\"id\", {id}).unwrap();\n        t.set(\"count\", {count}).unwrap();\n        t.set(\"damage\", {damage}).unwrap();{extra}\n        mlua::Value::Table(t)\n    }}"
            )
        };
        for bad in [
            table("\"ruby:gen\"", "2.5", "3", ""),
            table("\"ruby:gen\"", "0", "3", ""),
            table("\"ruby:gen\"", "100", "3", ""),
            table("\"ruby:gen\"", "7", "-1", ""),
            table(
                "\"ruby:gen\"",
                "7",
                "3",
                "\n        t.set(\"bogus\", 1).unwrap();",
            ),
            table("123", "7", "3", ""),
        ] {
            let mut lit = literals.to_vec();
            lit[pos] = bad;
            out.push_str(&bad_block(lit, &arg));
        }
    }
    // A non-table NBT argument is an error naming the argument, and
    // the host is never called.
    if let Some(pos) = row
        .args
        .iter()
        .position(|a| matches!(a.rust.as_str(), "Compound" | "&Compound"))
    {
        let arg = row.args[pos].name.clone();
        for value in [
            "mlua::Value::Integer(3)",
            "mlua::Value::String(lua.create_string(\"ruby:gen\").unwrap())",
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = value.to_owned();
            out.push_str(&bad_block(bad, &arg));
        }
    }
    // The eight slot names reach the host as the right variant; a
    // misspelled name or a non-string is an error naming the
    // argument, and the host is never called.
    if let Some(pos) = row.args.iter().position(|a| a.rust == "EquipmentSlot") {
        let arg = row.args[pos].name.clone();
        for (text, variant) in [
            ("mainhand", "MainHand"),
            ("offhand", "OffHand"),
            ("head", "Head"),
            ("chest", "Chest"),
            ("legs", "Legs"),
            ("feet", "Feet"),
            ("body", "Body"),
            ("saddle", "Saddle"),
        ] {
            let mut lit = literals.to_vec();
            lit[pos] = format!("mlua::Value::String(lua.create_string({text:?}).unwrap())");
            let mut s = String::from("    {\n");
            host_decl(&mut s);
            s.push_str(&discard_call(&lit));
            s.push_str(&format!(
                "\n        let last = {log}.logged().last().cloned().unwrap_or_default();\n        assert!(last.contains({variant:?}), \"{{last}}\");\n    }}\n",
                log = match kind {
                    GenKind::Action => "host",
                    GenKind::Read => "h",
                },
                variant = variant,
            ));
            out.push_str(&s);
        }
        for value in [
            "mlua::Value::String(lua.create_string(\"heead\").unwrap())".to_string(),
            "mlua::Value::Integer(3)".to_owned(),
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = value;
            out.push_str(&bad_block(bad, &arg));
        }
    }
    // Nil is `None`; a bad block-state name or a non-string is an
    // error naming the argument, and the host is never called.
    if let Some(pos) = row
        .args
        .iter()
        .position(|a| a.rust == "Option<BlockStateId>")
    {
        let arg = row.args[pos].name.clone();
        let mut lit = literals.to_vec();
        lit[pos] = "mlua::Value::Nil".to_owned();
        let mut s = String::from("    {\n");
        host_decl(&mut s);
        s.push_str(&discard_call(&lit));
        s.push_str(&format!(
            "\n        let last = {log}.logged().last().cloned().unwrap_or_default();\n        assert!(last.contains(\"None\"), \"{{last}}\");\n    }}\n",
            log = match kind {
                GenKind::Action => "host",
                GenKind::Read => "h",
            }
        ));
        out.push_str(&s);
        for value in [
            "mlua::Value::String(lua.create_string(\"ruby:gibberish\").unwrap())",
            "mlua::Value::Integer(3)",
        ] {
            let mut bad = literals.to_vec();
            bad[pos] = value.to_owned();
            out.push_str(&bad_block(bad, &arg));
        }
    }
    out
}

/// One helper's own test: the arguments reach the host exactly (strings,
/// saturated integers, a NaN on a float, an entity id resolved to its
/// handle, position tables), an unresolvable player or entity calls
/// nothing (a read answers nil), a bad id/component/parse is an error
/// naming the argument, and a read returns its exact canned value
/// (and nil for None).
fn helper_test(row: &HelperRow, backend: &Backend) -> String {
    let host_new = backend.host_new;
    let read_fn = backend.read_fn;
    let action_fn = backend.action_fn;
    let mut out = format!("\n#[test]\nfn {}_is_generated_and_exact() {{\n", row.name);
    let (literals, recorded) = normal_case(row);
    match &row.class {
        HelperClass::Generated(GenKind::Action) => {
            out.push_str(&format!(
                "    let lua = lua();\n    let mut host = {host_new}(false, false);\n    let t = args(&lua, &[{}]);\n    {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap();\n    assert_eq!(host.logged(), [{}]);\n",
                literals.join(", "),
                row.name,
                recorded
                    .iter()
                    .map(|l| format!("{:?}", l))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if row.args.iter().any(is_int) {
                for (literals, recorded) in edge_cases(row) {
                    out.push_str(&format!(
                        "    {{\n        let mut host = {host_new}(false, false);\n        let t = args(&lua, &[{}]);\n        {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap();\n        assert_eq!(host.logged(), [{}]);\n    }}\n",
                        literals.join(", "),
                        row.name,
                        recorded
                            .iter()
                            .map(|l| format!("{:?}", l))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            if let Some(pos) = row.args.iter().position(|a| a.player) {
                let mut ghost = literals.clone();
                ghost[pos] =
                    "mlua::Value::String(lua.create_string(\"ghost\").unwrap())".to_owned();
                out.push_str(&format!(
                    "    {{\n        let mut host = {host_new}(false, false);\n        let t = args(&lua, &[{}]);\n        {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap();\n        assert!(host.logged().is_empty(), \"an unresolvable player calls nothing\");\n    }}\n",
                    ghost.join(", "),
                    row.name
                ));
            }
            out.push_str(&residual_blocks(row, &literals, GenKind::Action, backend));
            if let Some(arg) = row
                .args
                .iter()
                .find(|a| matches!(a.rust.as_str(), "f32" | "f64"))
            {
                let mut nan_lua = literals.clone();
                let pos = row.args.iter().position(|a| std::ptr::eq(a, arg)).unwrap();
                nan_lua[pos] = "mlua::Value::Number(f64::NAN)".to_owned();
                out.push_str(&format!(
                    "    {{\n        let mut host = {host_new}(false, false);\n        let t = args(&lua, &[{}]);\n        let err = {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    nan_lua.join(", "),
                    row.name,
                    format!("{}.{}", row.name, arg.name)
                ));
            }
            if let Some(pos) = row.args.iter().position(|a| !a.player && a.rust != "u128") {
                let arg = &row.args[pos];
                let mut wrong = literals.clone();
                wrong[pos] = match arg.rust.as_str() {
                    // A position argument expects a table: a number
                    // errors for it instead.
                    "[f64; 3]" | "[i32; 3]" => "mlua::Value::Integer(3)",
                    _ => "mlua::Value::Table(lua.create_table().unwrap())",
                }
                .to_owned();
                out.push_str(&format!(
                    "    {{\n        let mut host = {host_new}(false, false);\n        let t = args(&lua, &[{}]);\n        let err = {action_fn}(&lua, &mut host, \"a\", {:?}, &t).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    wrong.join(", "),
                    row.name,
                    row.name,
                    arg.name
                ));
            }
        }
        HelperClass::Generated(GenKind::Read) => {
            out.push_str(&format!(
                "    let lua = lua();\n    let h = {host_new}(false, false);\n    let v = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n    assert!(matches!(v, {}), \"{{v:?}}\");\n",
                row.name,
                literals.join(", "),
                lua_type_of(row),
            ));
            let field_asserts = tuple_field_asserts(row);
            if !field_asserts.is_empty() {
                out.push_str("    if let mlua::Value::Table(t) = &v {\n");
                for line in &field_asserts {
                    out.push_str(&format!("{line}\n"));
                }
                out.push_str("    }\n");
            }
            out.push_str(&value_asserts(row));
            out.push_str(&format!(
                "    assert_eq!(h.logged(), [{}]);\n",
                recorded
                    .iter()
                    .map(|l| format!("{:?}", l))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if row.ret.starts_with("Option<") {
                out.push_str(&format!(
                    "    {{\n        let h = {host_new}(true, false);\n        let v = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n        assert!(matches!(v, mlua::Value::Nil), \"{{v:?}}\");\n    }}\n",
                    row.name,
                    literals.join(", ")
                ));
            }
            if row.args.iter().any(is_int) {
                for (literals, recorded) in edge_cases(row) {
                    out.push_str(&format!(
                        "    {{\n        let h = {host_new}(false, false);\n        let v = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n        let _ = v;\n        assert_eq!(h.logged(), [{}]);\n    }}\n",
                        row.name,
                        literals.join(", "),
                        recorded
                            .iter()
                            .map(|l| format!("{:?}", l))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            if let Some(pos) = row.args.iter().position(|a| a.player) {
                let mut ghost = literals.clone();
                ghost[pos] =
                    "mlua::Value::String(lua.create_string(\"ghost\").unwrap())".to_owned();
                // A string list is the empty table for a player who is not
                // on (the native doc's `Empty`), never nil; any other read
                // answers nil.
                let unresolved = if row.ret == "Vec<String>" {
                    "assert!(matches!(&v, mlua::Value::Table(t) if t.raw_len() == 0), \"{v:?}\");"
                } else {
                    "assert!(matches!(v, mlua::Value::Nil));"
                };
                out.push_str(&format!(
                    "    {{\n        let h = {host_new}(false, false);\n        let v = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n        {unresolved}\n        assert!(h.logged().is_empty());\n    }}\n",
                    row.name,
                    ghost.join(", ")
                ));
            }
            out.push_str(&residual_blocks(row, &literals, GenKind::Read, backend));
            if let Some(arg) = row
                .args
                .iter()
                .find(|a| matches!(a.rust.as_str(), "f32" | "f64"))
            {
                let mut nan_lua = literals.clone();
                let pos = row.args.iter().position(|a| std::ptr::eq(a, arg)).unwrap();
                nan_lua[pos] = "mlua::Value::Number(f64::NAN)".to_owned();
                out.push_str(&format!(
                    "    {{\n        let h = {host_new}(false, false);\n        let err = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    row.name,
                    nan_lua.join(", "),
                    format!("{}.{}", row.name, arg.name)
                ));
            }
            if let Some(pos) = row.args.iter().position(|a| !a.player && a.rust != "u128") {
                let arg = &row.args[pos];
                let mut wrong = literals.clone();
                wrong[pos] = match arg.rust.as_str() {
                    // A position argument expects a table: a number
                    // errors for it instead.
                    "[f64; 3]" | "[i32; 3]" => "mlua::Value::Integer(3)",
                    _ => "mlua::Value::Table(lua.create_table().unwrap())",
                }
                .to_owned();
                out.push_str(&format!(
                    "    {{\n        let h = {host_new}(false, false);\n        let err = {read_fn}(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    row.name,
                    wrong.join(", "),
                    row.name,
                    arg.name
                ));
            }
        }
        _ => {}
    }
    out.push_str("}\n");
    out
}

/// The field asserts of one tuple-return read (the `v` it already
/// matched as a `Table`): each element's exact canned value, at its
/// field name for a named shape (`min`/`max`, `value`/`signature`)
/// or its 1-based sequence position otherwise, so a swapped pair
/// fails (positional `[1]` first, then `[2]`; a `u128` first as its
/// uuid text, then the name). The canned values are
/// `canned_elem`'s, per position.
fn tuple_field_asserts(row: &HelperRow) -> Vec<String> {
    let scalar = row
        .ret
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(&row.ret);
    if !scalar.starts_with('(') {
        return Vec::new();
    }
    let shape = super::lua_helper_model::tuple_shape(scalar);
    let mut out = Vec::new();
    for (i, elem) in shape.elems.iter().enumerate() {
        let key = if shape.named {
            format!(
                "{:?}",
                shape.fields.get(i).map(String::as_str).unwrap_or("")
            )
        } else {
            format!("{}", i + 1)
        };
        match elem.as_str() {
            "String" | "&str" => out.push(format!(
                "        assert_eq!(t.get::<String>({key}).unwrap(), {:?});",
                tuple_str(1, i)
            )),
            "Option<String>" | "Option<&str>" => out.push(format!(
                "        assert_eq!(t.get::<Option<String>>({key}).unwrap().as_deref(), Some({:?}));",
                tuple_str(1, i)
            )),
            "f32" | "f64" => out.push(format!(
                "        assert_eq!(t.get::<f64>({key}).unwrap(), {});",
                tuple_float(i)
            )),
            "bool" => out.push(if i == 0 {
                format!("        assert!(t.get::<bool>({key}).unwrap());")
            } else {
                format!("        assert!(!t.get::<bool>({key}).unwrap());")
            }),
            "u128" => out.push(format!(
                "        assert_eq!(t.get::<String>({key}).unwrap(), \"123e4567-e89b-12d3-a456-426614174000\");",
            )),
            "[f64; 3]" => {
                let (x, y, z) = tuple_float_xyz(0);
                out.push(format!(
                    "        assert_eq!(t.get::<mlua::Table>({key}).unwrap().get::<f64>(\"x\").unwrap(), {x});",
                ));
                out.push(format!(
                    "        assert_eq!(t.get::<mlua::Table>({key}).unwrap().get::<f64>(\"y\").unwrap(), {y});",
                ));
                out.push(format!(
                    "        assert_eq!(t.get::<mlua::Table>({key}).unwrap().get::<f64>(\"z\").unwrap(), {z});",
                ));
            }
            "[i32; 3]" => {
                let (x, y, z) = tuple_int_xyz(0);
                out.push(format!(
                    "        assert_eq!(t.get::<mlua::Table>({key}).unwrap().get::<i64>(\"x\").unwrap(), {x});",
                ));
                out.push(format!(
                    "        assert_eq!(t.get::<mlua::Table>({key}).unwrap().get::<i64>(\"y\").unwrap(), {y});",
                ));
                out.push(format!(
                    "        assert_eq!(t.get::<mlua::Table>({key}).unwrap().get::<i64>(\"z\").unwrap(), {z});",
                ));
            }
            _ => out.push(format!(
                "        assert_eq!(t.get::<i64>({key}).unwrap(), {});",
                tuple_int(i)
            )),
        }
    }
    out
}

/// The exact canned-value asserts of one scalar read test (the `v` it
/// already matched as its Lua type): a string read asserts the exact
/// canned string, a float read the exact canned number, an `[i32; 3]`
/// read its `x`/`y`/`z` components — the canned payloads from
/// `canned_string`/`canned_float`/`canned_i32_xyz` (the same values
/// `gen_host_method`/`canned` hand the host double), so a bridge that
/// returns a fixed value of the right Lua type fails the test. Other
/// scalar returns need none here: `bool` already matches
/// `Boolean(true)` exactly, and tuples are `tuple_field_asserts`'s.
fn value_asserts(row: &HelperRow) -> String {
    let scalar = row
        .ret
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(&row.ret);
    match scalar {
        "String" | "&str" | "&'static str" => format!(
            "    if let mlua::Value::String(s) = &v {{\n        assert_eq!(s.to_string_lossy(), {:?});\n    }}\n",
            canned_string()
        ),
        "f32" | "f64" => format!(
            "    if let mlua::Value::Number(n) = &v {{\n        assert_eq!(*n, {:?});\n    }}\n",
            canned_float()
        ),
        "[i32; 3]" => {
            let xyz = canned_i32_xyz();
            format!(
                "    if let mlua::Value::Table(t) = &v {{\n        assert_eq!(t.get::<i64>(\"x\").unwrap(), {:?});\n        assert_eq!(t.get::<i64>(\"y\").unwrap(), {:?});\n        assert_eq!(t.get::<i64>(\"z\").unwrap(), {:?});\n    }}\n",
                xyz[0], xyz[1], xyz[2]
            )
        }
        "ItemStack" => {
            let (id, count, damage) = canned_stack();
            format!(
                "    if let mlua::Value::Table(t) = &v {{\n        assert_eq!(t.get::<String>(\"id\").unwrap(), {:?});\n        assert_eq!(t.get::<u32>(\"count\").unwrap(), {count});\n        assert_eq!(t.get::<u32>(\"damage\").unwrap(), {damage});\n        assert!(matches!(t.get::<mlua::Value>(\"extra\").unwrap(), mlua::Value::Nil));\n        assert!(t.get::<bool>(\"plain\").unwrap());\n    }}\n",
                id
            )
        }
        "BlockStateId" => format!(
            "    if let mlua::Value::String(s) = &v {{\n        assert_eq!(s.to_string_lossy(), {:?});\n    }}\n",
            canned_string()
        ),
        "Vec<String>" => format!(
            "    if let mlua::Value::Table(t) = &v {{\n        assert_eq!(t.raw_len(), 2);\n        assert_eq!(t.get::<String>(1).unwrap(), {:?});\n        assert_eq!(t.get::<String>(2).unwrap(), {:?});\n    }}\n",
            STRING_LIST[0], STRING_LIST[1]
        ),
        _ => String::new(),
    }
}

/// The Lua value type a canned read answers.
fn lua_type_of(row: &HelperRow) -> String {
    let scalar = row
        .ret
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(&row.ret);
    match scalar {
        "bool" => "mlua::Value::Boolean(true)".to_owned(),
        "String" | "u128" | "LevelRef" | "BlockStateId" => "mlua::Value::String(_)".to_owned(),
        "f32" | "f64" => "mlua::Value::Number(_)".to_owned(),
        "[f64; 3]" | "[i32; 3]" | "ItemStack" | "Vec<String>" => "mlua::Value::Table(_)".to_owned(),
        _ if scalar.starts_with('(') => "mlua::Value::Table(_)".to_owned(),
        _ => "mlua::Value::Integer(_)".to_owned(),
    }
}

/// The shared tests: the name lists are exactly the enabled set
/// (Generated minus the denylist); the server names raise in a client
/// state with the server-only error, the client names raise in a
/// server state with the client-only error, and a draw hook refuses a
/// client action (the same stub behaviour as the hand-written helpers);
/// reads plus an action run from hooks with the real drain (client
/// helpers through a `tick` dispatch on a client state); and the drain
/// test — a helper whose host answers `false` does not abort the
/// handler's remaining queued `broadcast` (the real queue, driven
/// through a hook dispatch as the net-send tests drive it).
fn shared_tests(rows: &[HelperRow]) -> String {
    let (reads, actions, client_reads, client_actions) = kinds_of(rows);
    let actions_lit = actions
        .iter()
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    let reads_lit = reads
        .iter()
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    let client_actions_lit = client_actions
        .iter()
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    let client_reads_lit = client_reads
        .iter()
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    let mut out = String::new();
    out.push_str(&format!(
        "#[test]\nfn the_generated_lists_are_the_enabled_set_and_nothing_else() {{\n    assert_eq!(\n        GENERATED_ACTIONS,\n        [\n{actions_lit}        ]\n    );\n    assert_eq!(\n        GENERATED_READS,\n        [\n{reads_lit}        ]\n    );\n    assert_eq!(\n        GENERATED_CLIENT_ACTIONS,\n        [\n{client_actions_lit}        ]\n    );\n    assert_eq!(\n        GENERATED_CLIENT_READS,\n        [\n{client_reads_lit}        ]\n    );\n}}\n\n"
    ));
    // Every generated name on `horizon`: a server state carries each as a
    // function, a client state raises the server-only error for each, and
    // reads plus an action run from a hook with the real drain.
    let all_lit = actions
        .iter()
        .chain(reads.iter())
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    out.push_str(
        r##"
#[test]
fn every_generated_name_is_a_function_on_horizon() {
    let (dir, path) = hmod(
        "gen_all_functions",
        &[(
            "scripts/server/init.lua",
            br#"
            local names = {
"##,
    );
    out.push_str(&all_lit);
    out.push_str(
        r##"            }
            local missing = {}
            for _, name in ipairs(names) do
              if type(horizon[name]) ~= "function" then
                missing[#missing + 1] = name
              end
            end
            assert(#missing == 0, "not installed: " .. table.concat(missing, ", "))
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
    assert_eq!(m.side(), LuaSide::Server);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn generated_server_names_raise_in_a_client_state() {
    let (dir, path) = hmod_as(
        "gen_client_stubs",
        &[(
            "scripts/client/init.lua",
            br#"
            local names = {
"##,
    );
    out.push_str(&all_lit);
    out.push_str(
        r##"            }
            for _, name in ipairs(names) do
              local f = horizon[name]
              assert(type(f) == "function", name .. " is not installed on horizon")
              local ok, err = pcall(f)
              assert(not ok, name .. " should fail in a client state")
              assert(
                string.find(tostring(err), "only available in scripts/server/", 1, true),
                name .. ": " .. tostring(err)
              )
            end
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
    assert_eq!(m.side(), LuaSide::Client);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn generated_helpers_run_from_a_hook() {
    // Reads run at once from the hook's host and an action waits for the
    // drain: `entity_position` (a resolved id's values and an unresolved
    // id's nil), `entity_velocity`, a `[i32; 3]`-argument action, the plain
    // `bool` read `sneaking`, the number read `world_seed`, and the string
    // read `entity_name`.
    let (dir, path) = hmod(
        "gen_hook_calls",
        &[(
            "scripts/server/init.lua",
            br#"
            horizon.on("net_from_client", function(event)
              local p = horizon.entity_position(7)
              horizon.broadcast("pos=" .. p.x .. "," .. p.y .. "," .. p.z)
              horizon.broadcast("pos999=" .. tostring(horizon.entity_position(999)))
              local v = horizon.entity_velocity(7)
              horizon.broadcast("vel=" .. v.x .. "," .. v.y .. "," .. v.z)
              horizon.broadcast("sneaking=" .. tostring(horizon.sneaking()))
              horizon.broadcast("seed=" .. tostring(horizon.world_seed()))
              horizon.broadcast("name=" .. tostring(horizon.entity_name(7)))
              horizon.broadcast("lv=" .. tostring(horizon.level_block("minecraft:overworld", {x = 1, y = 2, z = 3})))
              horizon.broadcast("lv999=" .. tostring(horizon.level_block("minecraft:nowhere", {x = 1, y = 2, z = 3})))
              horizon.block_entity_sync({x = 1, y = 2, z = 3})
            end)
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
    let mods: Vec<Box<dyn horizon_client::mod_api::Hooks>> = vec![Box::new(m)];
    let mut host = gen_host(false, false);
    horizon_client::mod_api::dispatch_net_from_client(
        &mut host,
        &mods,
        STEVE_UUID,
        "ruby:from",
        &[],
    );
    assert_eq!(
        host.logged(),
        [
            "entity_position 7",
            "entity_velocity 7",
            "sneaking",
            "world_seed",
            "entity_name 7",
            "level minecraft:overworld",
            "level_block minecraft:overworld [1, 2, 3]",
            "level minecraft:nowhere",
            "broadcast pos=1.5,2.5,3.5",
            "broadcast pos999=nil",
            "broadcast vel=1.5,2.5,3.5",
            "broadcast sneaking=true",
            "broadcast seed=3",
            "broadcast name=ruby:gen",
            "broadcast lv=ruby:gen",
            "broadcast lv999=nil",
            "block_entity_sync [1, 2, 3]",
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
"##,
    );
    let client_all_lit = client_actions
        .iter()
        .chain(client_reads.iter())
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    out.push_str(
        r##"
#[test]
fn every_generated_client_name_is_a_function_on_horizon() {
    for side in [LuaSide::Server, LuaSide::Client] {
        let (dir, path) = hmod_as(
            &format!("gen_client_functions_{side:?}"),
            &[(
                match side {
                    LuaSide::Server => "scripts/server/init.lua",
                    LuaSide::Client => "scripts/client/init.lua",
                },
                br#"
                local names = {
"##,
    );
    out.push_str(&client_all_lit);
    out.push_str(
        r##"                }
                local missing = {}
                for _, name in ipairs(names) do
                  local f = horizon[name]
                  if type(f) ~= "function" then
                    missing[#missing + 1] = name
                  end
                end
                assert(#missing == 0, "not installed: " .. table.concat(missing, ", "))
                "#,
            )],
        );
        let m = LuaMod::load(&path, side, &[]).unwrap();
        assert_eq!(m.side(), side);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
fn generated_client_names_raise_in_a_server_state() {
    let (dir, path) = hmod_as(
        "gen_server_stubs",
        &[(
            "scripts/server/init.lua",
            br#"
            local names = {
"##,
    );
    out.push_str(&client_all_lit);
    out.push_str(
        r##"            }
            for _, name in ipairs(names) do
              local f = horizon[name]
              assert(type(f) == "function", name .. " is not installed on horizon")
              local ok, err = pcall(f)
              assert(not ok, name .. " should fail in a server state")
              assert(
                string.find(tostring(err), "only available in scripts/client/", 1, true),
                name .. ": " .. tostring(err)
              )
            end
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
    assert_eq!(m.side(), LuaSide::Server);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn generated_client_helpers_run_from_a_client_hook() {
    // Client-side reads answer from the hook's host at once, and a
    // client action queues and runs after the handler against the
    // hook's `&mut` host: `has_shift_down` (a plain `bool` read),
    // `text_width` (a `f32` read of its argument), `playing` (a no-arg
    // `bool` read), and `flame_particle` (a typed position + a bool).
    // Every host line names its arguments, so a swapped argument or
    // the wrong side fails.
    let (dir, path) = hmod(
        "gen_client_hook_calls",
        &[(
            "scripts/client/init.lua",
            br#"
            horizon.on("tick", function(event)
              local pair = {horizon.has_shift_down(), horizon.playing()}
              horizon.text_width("ruby:gen1")
              horizon.flame_particle({x = 1.5, y = 2.5, z = 3.5}, true)
            end)
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
    let mods: Vec<Box<dyn horizon_client::mod_api::Hooks>> = vec![Box::new(m)];
    let mut host = gen_host(false, false);
    horizon_client::mod_api::dispatch_tick(&mut host, &mods);
    assert_eq!(
        host.logged(),
        [
            "has_shift_down",
            "playing",
            "text_width ruby:gen1",
            "flame_particle [1.5, 2.5, 3.5] true",
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn generated_client_reads_work_in_a_draw_hook_but_actions_do_not() {
    // A `&self` client read answers through the shared host in a draw
    // hook (`hud`); a client action raises the draw-hook error, like a
    // `marker_*` call does. (Every generated read is `&self` by
    // construction — a `&mut self` method returning data is `Cannot` —
    // so the read arm does not diverge.)
    let (dir, path) = hmod(
        "gen_client_draw",
        &[(
            "scripts/client/init.lua",
            br#"
            horizon.on("hud", function(event)
              __shift = horizon.has_shift_down()
              local ok, err = pcall(function()
                horizon.flame_particle({x = 1.5, y = 2.5, z = 3.5}, true)
              end)
              __draw_ok = ok
              __draw_err = tostring(err)
            end)
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
    let state = m.lua.clone();
    let mods: Vec<Box<dyn horizon_client::mod_api::Hooks>> = vec![Box::new(m)];
    let mut host = gen_host(false, false);
    let mut batch = horizon_client::mod_api::HudBatch::new([800, 600]);
    let mut corners = horizon_client::mod_api::Corners::new(800.0, 600.0, 0.0);
    horizon_client::mod_api::dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
    assert!(state.globals().get::<bool>("__shift").unwrap());
    assert!(!state.globals().get::<bool>("__draw_ok").unwrap());
    assert!(
        state
            .globals()
            .get::<String>("__draw_err")
            .unwrap()
            .contains("not available in draw hooks (hud, gui_layer_post, gui_layer_pre, world_primitives)"),
        "{:?}",
        state.globals().get::<String>("__draw_err")
    );
    assert_eq!(host.logged(), ["has_shift_down"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn generated_names_raise_in_a_draw_hook() {
    let (dir, path) = hmod(
        "gen_draw_stubs",
        &[(
            "scripts/client/init.lua",
            br#"
            horizon.on("hud", function(event)
              local function expect_server_only(name, f)
                local ok, err = pcall(f)
                assert(not ok, name .. " should fail in a draw hook")
                assert(
                  string.find(tostring(err), "is only available in scripts/server/", 1, true),
                  name .. ": " .. tostring(err)
                )
              end
              expect_server_only("remove_effect", function() horizon.remove_effect("steve", "minecraft:poison") end)
              expect_server_only("profile_by_uuid", function() return horizon.profile_by_uuid("steve") end)
            end)
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
    let mods: Vec<Box<dyn horizon_client::mod_api::Hooks>> = vec![Box::new(m)];
    let mut host = gen_host(false, false);
    let mut batch = horizon_client::mod_api::HudBatch::new([800, 600]);
    let mut corners = horizon_client::mod_api::Corners::new(800.0, 600.0, 0.0);
    horizon_client::mod_api::dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_false_generated_action_does_not_abort_later_queued_actions() {
    // The drain test: one `net_from_client` handler queues a generated
    // action whose host answers `false` (a native refusal, ordinary) and
    // then a `broadcast`, and the `broadcast` still runs. (Driving the
    // real `call_kind` drain, not `run_action` on its own.)
    let (dir, path) = hmod(
        "gen_drain",
        &[(
            "scripts/server/init.lua",
            br#"
            horizon.on("net_from_client", function(event)
              horizon.remove_effect(event.player, "minecraft:poison")
              horizon.broadcast("after")
            end)
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Server, &[]).unwrap();
    let mods: Vec<Box<dyn horizon_client::mod_api::Hooks>> = vec![Box::new(m)];
    let mut host = gen_host(false, true);
    horizon_client::mod_api::dispatch_net_from_client(
        &mut host,
        &mods,
        STEVE_UUID,
        "ruby:from",
        &[],
    );
    assert_eq!(
        host.logged(),
        [
            "remove_effect 24249434048109030647017182301789831168 minecraft:poison",
            "broadcast after"
        ]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
"##,
    );
    out
}

/// The `.hmod` helpers every generated test file carries (the manifest
/// every LuaMod test loads with).
fn hmod_helpers() -> &'static str {
    r##"
const MANIFEST_TEXT: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["scripts"]
experimental = ["scripts"]
"#;

fn hmod(name: &str, files: &[(&str, &[u8])]) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("horizon-lua-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ruby.hmod");
    std::fs::write(
        &path,
        horizon_mods::Package::write_files(MANIFEST_TEXT, None, files).unwrap(),
    )
    .unwrap();
    (dir, path)
}

fn hmod_as(id: &str, files: &[(&str, &[u8])]) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("horizon-lua-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ruby.hmod");
    std::fs::write(
        &path,
        horizon_mods::Package::write_files(MANIFEST_TEXT, None, files).unwrap(),
    )
    .unwrap();
    (dir, path)
}
"##
}

/// `docs/api/helper-exposure.md`: every one of the catalog helpers with its
/// class and reason, grouped; the Adapter rows list their blocking
/// types, the excluded rows their reason. The header carries the
/// counts, the adapter ranking with the conversion each type needs,
/// how many helpers the top few adapters would unblock, and what the
/// generator could not express.
fn exposure_of(rows: &[HelperRow], enabled: &[HelperRow]) -> String {
    let (generated, already, adapter, handle, cannot) = class_counts(rows);
    let enabled_count = enabled.len();
    let excluded_count = generated - enabled_count;
    let mut out = String::new();
    out.push_str("# Helper exposure\n\n");
    out.push_str("<!-- GENERATED by `cargo run -p xtask -- lua-helpers`; do not edit. `--check` fails when this file is not what the classifier says. -->\n\n");
    out.push_str(&"Every native host helper (`native-hooks.json` plus the pending `tools/native-hook-fragments/`, @TOTAL@ in all) lands in exactly one class: `generated` (plain args and a supported return: the generator can emit the `horizon.*` bridge), `already-lua` (a hand-written Lua helper already stands behind it), `adapter` (a hand-written converter would unblock it: the table names the blocking types), `handle` (an opaque draw/mod handle, needing its own design), or `cannot` (no bridge, with the reason). `docs/api/hook-exposure.md` is the same idea for hook points. This page is `lua-helpers`'s human-readable record; `crates/event-horizon-lua/src/generated_helpers.rs` is its bridge for the enabled helpers. Enabled is every `generated` helper except the [`EXCLUDED`] denylist below.\n\n".replace("@TOTAL@", &rows.len().to_string()));
    out.push_str(&format!(
        "{generated} generated ({enabled_count} enabled, {excluded_count} excluded), {already} already-lua, {adapter} adapter, {handle} handle, {cannot} cannot ({} total). Every generated helper not on the denylist reaches Lua; there is no other not-enabled bucket.\n\n",
        rows.len(),
    ));
    out.push_str("## What the numbers say\n\n");
    let ranking = adapter_ranking(rows);
    let distinct = ranking.len();
    let top_names = |n: usize| {
        ranking
            .iter()
            .take(n)
            .map(|(ty, _)| ty.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.push_str(&format!(
        "{distinct} distinct blocking types, ranked by how many helpers they block (one conversion each).\n\n| Type | Helpers blocked | Conversion |\n| --- | --- | --- |\n",
    ));
    for (ty, count) in &ranking {
        out.push_str(&format!("| `{ty}` | {count} | {} |\n", adapter_sketch(ty)));
    }
    out.push_str(&format!(
        "\nConverting the top 1 (`{}`) would unblock {} Adapter helpers; the top 3 (`{}`) would unblock {}; the top 5 (`{}`) would unblock {}; the top 8 would unblock {}; the top 10 would unblock {}; the top 15 would unblock {}; the top 20 would unblock {}; converting all {distinct} together would unblock {} (every Adapter helper, as each lists only those types).\n\n",
        top_names(1),
        unblocked_by_top(rows, 1),
        top_names(3),
        unblocked_by_top(rows, 3),
        top_names(5),
        unblocked_by_top(rows, 5),
        unblocked_by_top(rows, 8),
        unblocked_by_top(rows, 10),
        unblocked_by_top(rows, 15),
        unblocked_by_top(rows, 20),
        unblocked_by_top(rows, distinct),
    ));
    out.push_str("What the generator could not express: a `&mut self` helper returning data (a queued action has no way back; it needs the run_command_as callback design); a `Result` return (no error design yet); a callback/closure parameter (needs that same design); an opaque `&mut` draw/mod handle; and every non-plain type above. Native-name types in the adapter list are all small read conversions of the same kinds.\n\n");
    out.push_str("Generator size: this generator plus its model are hand-written; the emitted bridge and its tests are the output. Counted separately in the report.\n\n");
    out.push_str("Generated but excluded (the denylist): these all have plain args and a supported return (the generator can emit them) but are not emitted: their Lua cell in `docs/api/hook-points.md` stays empty, each for the reason named.\n\n");
    group_of(
        &mut out,
        "Generated, excluded",
        rows,
        |row| matches!(row.class, HelperClass::Generated(_)) && exclusion_of(&row.name).is_some(),
        |row| exclusion_of(&row.name).unwrap().to_owned(),
    );
    out.push_str(&format!("\n## Generated, enabled ({})\n\n", enabled.len()));
    out.push_str(
        "| Helper | Side | Receiver | Kind | Lua signature |\n| --- | --- | --- | --- | --- |\n",
    );
    for row in enabled {
        let kind = match &row.class {
            HelperClass::Generated(GenKind::Read) => "read",
            HelperClass::Generated(GenKind::Action) => "action",
            _ => unreachable!(),
        };
        let (_, sig) = luau_function(row);
        let receiver = row.receiver.as_deref().unwrap_or("-");
        let side = if is_client_side(&row.name) {
            "client"
        } else {
            "server"
        };
        out.push_str(&format!(
            "| `{}` | {side} | {receiver} | {kind} | `{sig}` |\n",
            row.name
        ));
    }
    // Every plain two-tuple shape this round enabled, why: nine hook
    // shapes reuse `tuple_struct` (only `level_height`'s `(i32, i32)` is
    // among them), a tuple with an `Option` element is named (its field
    // names come from the helper's doc comment: `value` and `signature`
    // for `player_textures` and `set_profile_property`, the neutral
    // `first`/`second` fallback for any other), and every other plain
    // pair is a positional sequence; excluded tuples never reach Lua.
    // A `LevelRef` is its dimension id string (`"minecraft:overworld"`),
    // resolved through the host's `level` lookup; a `Direction` is one
    // of its six lowercase names.
    let mut shapes: Vec<(String, String)> = Vec::new();
    for row in rows {
        if !matches!(row.class, HelperClass::Generated(_)) {
            continue;
        }
        let tuple_here = row.args.iter().any(|a| a.rust.starts_with("Option<("))
            || row.ret.starts_with("Option<(");
        if !tuple_here {
            continue;
        }
        let (_, sig) = luau_function(row);
        let mark = if exclusion_of(&row.name).is_some() {
            " (excluded; no Lua cell)"
        } else {
            ""
        };
        shapes.push((row.name.clone(), format!("{sig}{mark}")));
    }
    out.push_str(&format!("\n## Plain tuple shapes ({})\n\n", shapes.len()));
    for (name, sig) in &shapes {
        out.push_str(&format!("- `{name}` -> `{sig}`\n"));
    }
    group_of(
        &mut out,
        "Already Lua",
        rows,
        |row| matches!(row.class, HelperClass::AlreadyLua),
        |_row| "already a hand-written Lua helper".to_owned(),
    );
    group_of(
        &mut out,
        "Adapter: a converter would unblock it",
        rows,
        |row| matches!(row.class, HelperClass::Adapter(_)),
        |row| match &row.class {
            HelperClass::Adapter(types) => {
                let types: Vec<String> = types.iter().map(|ty| format!("`{ty}`")).collect();
                format!("blocks on {}", types.join(", "))
            }
            _ => unreachable!(),
        },
    );
    group_of(
        &mut out,
        "Handle: an opaque draw/mod handle",
        rows,
        |row| matches!(row.class, HelperClass::Handle(_)),
        |row| match &row.class {
            HelperClass::Handle(types) => {
                let types: Vec<String> = types.iter().map(|ty| format!("`{ty}`")).collect();
                format!("a {} handle needs its own design", types.join(", "))
            }
            _ => unreachable!(),
        },
    );
    group_of(
        &mut out,
        "Cannot: no bridge",
        rows,
        |row| matches!(row.class, HelperClass::Cannot(_)),
        |row| match &row.class {
            HelperClass::Cannot(why) => why.clone(),
            _ => unreachable!(),
        },
    );
    out
}

/// One exposure group's section: its table of `| Helper | Reason |`,
/// with the native doc beside the reason.
fn group_of(
    out: &mut String,
    title: &str,
    rows: &[HelperRow],
    select: impl Fn(&HelperRow) -> bool,
    reason: impl Fn(&HelperRow) -> String,
) {
    let members: Vec<&HelperRow> = rows.iter().filter(|row| select(row)).collect();
    out.push_str(&format!("\n## {title} ({})\n\n", members.len()));
    out.push_str("| Helper | Reason |\n| --- | --- |\n");
    for row in members {
        let why = reason(row);
        out.push_str(&format!("| `{}` | {why} |\n", row.name));
    }
}

/// One line on the conversion a blocking type needs, per the settled
/// rule in `docs/lua-bindings.md`: three numbers read as `{x, y, z}`, two
/// ints as `{x, z}`, two floats as `{x, y}`, other arrays as a sequence,
/// and `[u8; N]` as a Lua string of N bytes.
fn adapter_sketch(ty: &str) -> String {
    match ty {
        "EntityRef" => "an entity id number, resolved to its view".to_owned(),
        "LevelRef" => "the hook's level (a dimension name or the acting level)".to_owned(),
        "Direction" => "a string naming the direction".to_owned(),
        "Text" => "a string, or a component table (the text rule)".to_owned(),
        "ItemStack" => "an item table (`id`, `count`, …)".to_owned(),
        "Vec3" => "a `{x, y, z}` table".to_owned(),
        "[u8]" => "a string of bytes (a Lua string)".to_owned(),
        _ if ty.starts_with("[") && ty.contains("; ") => {
            let inner = ty.strip_prefix('[').unwrap_or(ty);
            let inner = inner.strip_suffix(']').unwrap_or(inner);
            let (elem, len) = inner.split_once(';').unwrap_or((inner, ""));
            let elem = elem.trim().to_owned();
            let len = len.trim().to_owned();
            if elem == "u8" {
                return format!("a Lua string of {len} bytes");
            }
            let len: usize = len.parse().unwrap_or(0);
            if len == 2 {
                if elem == "f32" || elem == "f64" {
                    return "an `{x, y}` table".to_owned();
                }
                if matches!(
                    elem.as_str(),
                    "i8" | "i16"
                        | "i32"
                        | "i64"
                        | "i128"
                        | "isize"
                        | "u16"
                        | "u32"
                        | "u64"
                        | "usize"
                ) {
                    return "an `{x, z}` table".to_owned();
                }
            }
            if len == 3 {
                return "a `{x, y, z}` table".to_owned();
            }
            format!("a sequence of {len} numbers")
        }
        _ if ty.starts_with("[") => "an array table, element by element".to_owned(),
        _ if ty.starts_with("Vec<") => "an array table, element by element".to_owned(),
        _ if ty.starts_with("Option<") => "an optional Lua argument (nil when absent)".to_owned(),
        _ if ty == "u64" || ty == "u128" || ty == "i128" => {
            "an id as a string of hex/digits (a Lua number cannot hold it exactly)".to_owned()
        }
        _ => "a table of its fields, with the hand-written readers".to_owned(),
    }
}

/// The test-only synthetic conformance file: one module carrying
/// the synthetic bridge (from the same emitters), the synthetic
/// host, and their tests.
fn synthetic_of() -> String {
    let rows = synthetic_rows();
    let mut out = String::new();
    out.push_str("// Generated by `cargo run -p xtask -- lua-helpers`: do not edit.\n");
    out.push_str("// The synthetic conformance module: bridge arms and tests for the\n");
    out.push_str("// rows `synthetic_rows` fixes in the generator, over a local\n");
    out.push_str("// `SyntheticHost` trait, so every emitter branch no enabled helper\n");
    out.push_str("// reaches is compiled and executed. `#[cfg(test)]` only.\n\n");
    out.push_str(&bridge_of(&rows, &SYNTHETIC_BACKEND));
    out.push('\n');
    out.push_str(&synthetic_host(&rows));
    out.push_str(&tests_of(&rows, &SYNTHETIC_BACKEND));
    out
}

/// The synthetic file's shared test: its name lists are exactly the
/// synthetic rows.
fn synthetic_shared_tests(rows: &[HelperRow]) -> String {
    let (reads, actions, client_reads, client_actions) = kinds_of(rows);
    debug_assert!(client_reads.is_empty() && client_actions.is_empty());
    let actions_lit = actions
        .iter()
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    let reads_lit = reads
        .iter()
        .map(|n| format!("            {n:?},\n"))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"
#[test]
fn the_synthetic_lists_are_the_synthetic_set_and_nothing_else() {{
    assert_eq!(
        GENERATED_ACTIONS,
        [
{actions_lit}        ]
    );
    assert_eq!(
        GENERATED_READS,
        [
{reads_lit}        ]
    );
    assert!(GENERATED_CLIENT_ACTIONS.is_empty());
    assert!(GENERATED_CLIENT_READS.is_empty());
}}
"#
    )
}

/// The argument-conversion kinds the bridge emitter branches on, in
/// the same order as `read_lines`'s own branches.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg(test)]
enum ArgKind {
    Player,
    Entity,
    Direction,
    Level,
    OptTuple,
    PosFloat,
    PosInt,
    ItemStack,
    Compound,
    EquipmentSlot,
    OptBlockStateId,
    Str,
    Bool,
    Float64,
    Float32,
    Int,
}

#[cfg(test)]
const ALL_ARG_KINDS: &[ArgKind] = &[
    ArgKind::Player,
    ArgKind::Entity,
    ArgKind::Direction,
    ArgKind::Level,
    ArgKind::OptTuple,
    ArgKind::PosFloat,
    ArgKind::PosInt,
    ArgKind::ItemStack,
    ArgKind::Compound,
    ArgKind::EquipmentSlot,
    ArgKind::OptBlockStateId,
    ArgKind::Str,
    ArgKind::Bool,
    ArgKind::Float64,
    ArgKind::Float32,
    ArgKind::Int,
];

#[cfg(test)]
fn arg_kind(arg: &super::lua_helper_model::Arg) -> ArgKind {
    if arg.player {
        ArgKind::Player
    } else if arg.entity {
        ArgKind::Entity
    } else if arg.direction {
        ArgKind::Direction
    } else if arg.level {
        ArgKind::Level
    } else if arg.rust.starts_with("Option<(") {
        ArgKind::OptTuple
    } else if arg.rust == "[f64; 3]" {
        ArgKind::PosFloat
    } else if arg.rust == "[i32; 3]" {
        ArgKind::PosInt
    } else {
        match arg.rust.as_str() {
            "ItemStack" => ArgKind::ItemStack,
            "Compound" | "&Compound" => ArgKind::Compound,
            "EquipmentSlot" => ArgKind::EquipmentSlot,
            "Option<BlockStateId>" => ArgKind::OptBlockStateId,
            "&str" | "String" => ArgKind::Str,
            "bool" => ArgKind::Bool,
            "f64" => ArgKind::Float64,
            "f32" => ArgKind::Float32,
            _ => ArgKind::Int,
        }
    }
}

/// The return-conversion kinds the read emitter branches on (in
/// `value_of`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg(test)]
enum RetKind {
    Unit,
    Bool,
    Str,
    U128,
    LevelRef,
    F32,
    Float64,
    Int,
    PosFloat,
    PosInt,
    ItemStack,
    BlockStateId,
    Tuple,
    StringList,
}

#[cfg(test)]
const ALL_RET_KINDS: &[RetKind] = &[
    RetKind::Unit,
    RetKind::Bool,
    RetKind::Str,
    RetKind::U128,
    RetKind::LevelRef,
    RetKind::F32,
    RetKind::Float64,
    RetKind::Int,
    RetKind::PosFloat,
    RetKind::PosInt,
    RetKind::ItemStack,
    RetKind::BlockStateId,
    RetKind::Tuple,
    RetKind::StringList,
];

#[cfg(test)]
fn ret_kind(row: &HelperRow) -> RetKind {
    let scalar = row
        .ret
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(&row.ret);
    match scalar {
        "unit" => RetKind::Unit,
        "bool" => RetKind::Bool,
        "String" => RetKind::Str,
        "u128" => RetKind::U128,
        "LevelRef" => RetKind::LevelRef,
        "f32" => RetKind::F32,
        "f64" => RetKind::Float64,
        "[f64; 3]" => RetKind::PosFloat,
        "[i32; 3]" => RetKind::PosInt,
        "ItemStack" => RetKind::ItemStack,
        "BlockStateId" => RetKind::BlockStateId,
        "Vec<String>" => RetKind::StringList,
        _ if scalar.starts_with('(') => RetKind::Tuple,
        _ => RetKind::Int,
    }
}

/// The tuple-argument element kinds the emitter branches on (in
/// `tuple_arg_lines`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg(test)]
enum TupleArgElemKind {
    OptStr,
    PosInt,
    PosFloat,
    Str,
    Float32,
    Float64,
    Int,
}

#[cfg(test)]
const ALL_TUPLE_ARG_ELEM_KINDS: &[TupleArgElemKind] = &[
    TupleArgElemKind::OptStr,
    TupleArgElemKind::PosInt,
    TupleArgElemKind::PosFloat,
    TupleArgElemKind::Str,
    TupleArgElemKind::Float32,
    TupleArgElemKind::Float64,
    TupleArgElemKind::Int,
];

#[cfg(test)]
fn tuple_arg_elem_kind(elem: &str) -> TupleArgElemKind {
    match elem {
        "Option<String>" | "Option<&str>" => TupleArgElemKind::OptStr,
        "[i32; 3]" => TupleArgElemKind::PosInt,
        "[f64; 3]" => TupleArgElemKind::PosFloat,
        "String" | "&str" => TupleArgElemKind::Str,
        "f32" => TupleArgElemKind::Float32,
        "f64" => TupleArgElemKind::Float64,
        _ => TupleArgElemKind::Int,
    }
}

/// The tuple-return element kinds the emitter branches on (in
/// `elem_value`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg(test)]
enum TupleRetElemKind {
    Bool,
    Str,
    F64,
    U128,
    F32,
    PosFloat,
    PosInt,
    Int,
}

#[cfg(test)]
const ALL_TUPLE_RET_ELEM_KINDS: &[TupleRetElemKind] = &[
    TupleRetElemKind::Bool,
    TupleRetElemKind::Str,
    TupleRetElemKind::F64,
    TupleRetElemKind::U128,
    TupleRetElemKind::F32,
    TupleRetElemKind::PosFloat,
    TupleRetElemKind::PosInt,
    TupleRetElemKind::Int,
];

#[cfg(test)]
fn tuple_ret_elem_kind(elem: &str) -> TupleRetElemKind {
    match elem {
        "bool" => TupleRetElemKind::Bool,
        "String" | "&str" => TupleRetElemKind::Str,
        "f64" => TupleRetElemKind::F64,
        "u128" => TupleRetElemKind::U128,
        "f32" => TupleRetElemKind::F32,
        "[f64; 3]" => TupleRetElemKind::PosFloat,
        "[i32; 3]" => TupleRetElemKind::PosInt,
        _ => TupleRetElemKind::Int,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    /// The `GenHost` double carries the native return: a unit action's
    /// method has no `->`, a `bool` action's returns `bool` (so enabling
    /// a unit helper compiles and the `unused_unit` lint stays quiet).
    #[test]
    fn gen_host_double_matches_the_native_return() {
        let rows = classify_helpers(&root()).unwrap();
        let unit = rows.iter().find(|r| r.name == "mod_screen_close").unwrap();
        let method = gen_host_method(unit);
        assert!(method.contains("fn mod_screen_close"), "{method}");
        assert!(!method.contains("-> ()"), "{method}");
        let boolean = rows.iter().find(|r| r.name == "set_flight").unwrap();
        let method = gen_host_method(boolean);
        assert!(method.contains("fn set_flight"), "{method}");
        assert!(method.contains("-> bool"), "{method}");
    }

    /// Every [`EXCLUDED`] entry names a real helper that classifies as
    /// Generated and carries a non-empty reason, so a typo or a stale
    /// entry fails instead of silently keeping it.
    #[test]
    fn excluded_names_real_generated_helpers() {
        let rows = classify_helpers(&root()).unwrap();
        for (name, why) in EXCLUDED {
            assert!(!why.is_empty(), "{name} has an empty exclusion reason");
            let row = rows
                .iter()
                .find(|r| r.name == *name)
                .unwrap_or_else(|| panic!("{name} is excluded but is not a helper"));
            assert!(
                matches!(row.class, HelperClass::Generated(_)),
                "{name} is excluded but classifies as {:?}",
                row.class
            );
        }
    }

    /// A [`BOUND_MOD_ID`] helper is enabled, and its Lua signature (the
    /// `.d.lua` type and the exposure doc's column) leaves the `mod_id`
    /// out: a script that passes it would shift the arguments (issue 61).
    #[test]
    fn bound_mod_id_helpers_take_no_mod_id_from_the_script() {
        let rows = classify_helpers(&root()).unwrap();
        let enabled = enabled_rows(&rows);
        for name in BOUND_MOD_ID {
            let row = rows
                .iter()
                .find(|r| r.name == *name)
                .unwrap_or_else(|| panic!("{name} is bound but is not a helper"));
            assert!(
                enabled.iter().any(|r| r.name == *name),
                "{name} is bound but not enabled"
            );
            let (_, sig) = luau_function(row);
            assert!(!sig.contains("mod_id"), "{name}: {sig}");
        }
    }

    /// The enabling policy: every helper the model classifies as
    /// Generated is either enabled or explicitly excluded, and is
    /// impossible for one to be neither. A missing Generated helper
    /// would silently lower the yield, so every Generated helper is
    /// accounted for here.
    #[test]
    fn every_generated_helper_is_enabled_or_excluded() {
        let rows = classify_helpers(&root()).unwrap();
        let enabled = enabled_rows(&rows);
        let enabled_names: std::collections::BTreeSet<&str> =
            enabled.iter().map(|r| r.name.as_str()).collect();
        for row in &rows {
            if matches!(row.class, HelperClass::Generated(_)) {
                assert!(
                    enabled_names.contains(row.name.as_str()) || exclusion_of(&row.name).is_some(),
                    "{} is Generated but neither enabled nor excluded",
                    row.name
                );
            }
        }
    }

    /// The exposure doc names every helper exactly once: the generated
    /// enabled table, the excluded denylist, and the adapter, handle and
    /// cannot groups cover each of the helpers a single time.
    #[test]
    fn the_exposure_doc_names_every_helper_exactly_once() {
        let rows = classify_helpers(&root()).unwrap();
        let enabled = enabled_rows(&rows);
        let doc = exposure_of(&rows, &enabled);
        for row in &rows {
            let needle = format!("| `{}` |", row.name);
            let count = doc.matches(needle.as_str()).count();
            assert_eq!(count, 1, "{} is named {} times", row.name, count);
        }
    }

    /// Every emitter conversion kind (argument, return, tuple-argument
    /// element and tuple-return element) is covered by an enabled
    /// helper or a synthetic row: a new converter cannot ship
    /// uncovered.
    #[test]
    fn every_emitter_conversion_kind_is_covered_by_enabled_or_synthetic() {
        let rows = classify_helpers(&root()).unwrap();
        let enabled = enabled_rows(&rows);
        let synthetic = synthetic_rows();
        let mut args = std::collections::BTreeSet::new();
        let mut rets = std::collections::BTreeSet::new();
        let mut tup_args = std::collections::BTreeSet::new();
        let mut tup_rets = std::collections::BTreeSet::new();
        for row in enabled.iter().chain(synthetic.iter()) {
            rets.insert(ret_kind(row));
            for arg in &row.args {
                args.insert(arg_kind(arg));
                if arg.rust.starts_with("Option<(") {
                    let inner = arg
                        .rust
                        .strip_prefix("Option<")
                        .and_then(|s| s.strip_suffix('>'))
                        .unwrap();
                    for elem in crate::lua_helper_model::tuple_elems(inner) {
                        tup_args.insert(tuple_arg_elem_kind(&elem));
                    }
                }
            }
            let scalar = row
                .ret
                .strip_prefix("Option<")
                .and_then(|r| r.strip_suffix('>'))
                .unwrap_or(&row.ret);
            if scalar.starts_with('(') {
                for elem in crate::lua_helper_model::tuple_elems(scalar) {
                    tup_rets.insert(tuple_ret_elem_kind(&elem));
                }
            }
        }
        let missing_args: Vec<_> = ALL_ARG_KINDS.iter().filter(|k| !args.contains(k)).collect();
        let missing_rets: Vec<_> = ALL_RET_KINDS.iter().filter(|k| !rets.contains(k)).collect();
        let missing_tup_args: Vec<_> = ALL_TUPLE_ARG_ELEM_KINDS
            .iter()
            .filter(|k| !tup_args.contains(k))
            .collect();
        let missing_tup_rets: Vec<_> = ALL_TUPLE_RET_ELEM_KINDS
            .iter()
            .filter(|k| !tup_rets.contains(k))
            .collect();
        assert!(
            missing_args.is_empty()
                && missing_rets.is_empty()
                && missing_tup_args.is_empty()
                && missing_tup_rets.is_empty(),
            "uncovered kinds: arg {missing_args:?}, ret {missing_rets:?}, tuple-arg {missing_tup_args:?}, tuple-ret {missing_tup_rets:?}"
        );
    }
}
