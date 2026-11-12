//! `cargo run -p xtask -- lua-helpers [--check]`: the Lua bridge of every
//! native host helper `lua_helper_model` classifies as Generated, from
//! the `native_*` signatures (never hand-written), plus its tests
//! (`crates/event-horizon-lua/src/generated_helpers.rs` and
//! `generated_helpers_tests.rs`) and the exposure doc
//! (`docs/api/helper-exposure.md`). `--check` fails when any of its five
//! outputs is not what the model says.
//!
//! The spike enables [`SPIKE_SET`]: five reads and ten actions. Every
//! other Generated helper is classified but neither emitted nor
//! documented as reaching Lua; the exposure doc lists it as "eligible,
//! not enabled yet", and `run` fails if an enabled helper stops
//! classifying as Generated ("dropped from the output") or overlaps a
//! hand-written Lua name.
use super::lua_helper_model::{
    GenKind, HelperClass, HelperRow, adapter_ranking, class_counts, classify_helpers,
    unblocked_by_top,
};
use std::{collections::BTreeSet, fs, path::Path};

/// The helpers this spike enables: five reads (`&self` helpers whose
/// value travels back) and ten queued actions (`&mut self` helpers of
/// unit/bool return, dropped as every hand-written action drops it).
/// Enable order is the doc and emission order.
pub const SPIKE_SET: &[&str] = &[
    "remove_effect",
    "set_attribute_base",
    "set_flight",
    "respawn_player",
    "resend_commands",
    "profile_by_uuid",
    "player_view_distance",
    "player_sneaking",
    "player_stat",
    "hide_entity_from",
    "sleep_ignored",
    "mob_stop",
    "mob_navigating",
    "max_stack_size",
    "refresh_player",
];

pub fn run(check: bool) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let rows = classify_helpers(&root)?;
    let (generated, already, adapter, handle, cannot) = class_counts(&rows);
    println!(
        "lua-helpers: {generated} generated, {already} already-lua, {adapter} adapter, \
         {handle} handle, {cannot} cannot ({} total; {} enabled)",
        rows.len(),
        SPIKE_SET.len()
    );
    let enabled = enabled_rows(&rows)?;
    // The hand-written and generated names stay disjoint.
    let lua_names: BTreeSet<&str> = super::hooks_doc::lua_helper_names().into_iter().collect();
    for row in &enabled {
        if lua_names.contains(row.name.as_str()) {
            return Err(format!(
                "{} is both a hand-written Lua helper and enabled by SPIKE_SET",
                row.name
            ));
        }
    }
    let bridge = rustfmt(&bridge_of(&enabled))?;
    let tests = rustfmt(&tests_of(&enabled))?;
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

/// The spike-set rows, in [`SPIKE_SET`] order, each verified to classify
/// as Generated. One that stops classifying as Generated is "dropped
/// from the output" and fails the run, as an enabled helper missing
/// from the generated bridge would.
fn enabled_rows(rows: &[HelperRow]) -> Result<Vec<HelperRow>, String> {
    let mut out = Vec::new();
    for name in SPIKE_SET {
        let row = rows
            .iter()
            .find(|row| row.name == *name)
            .ok_or_else(|| format!("{name} is in SPIKE_SET but is not a helper"))?;
        match &row.class {
            HelperClass::Generated(_) => {}
            class => {
                return Err(format!(
                    "{name} is enabled by SPIKE_SET but classifies as {class:?}"
                ));
            }
        }
        out.push(HelperRow {
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
        });
    }
    Ok(out)
}

/// The Lua names of the spike set's read and action halves.
fn kinds_of(rows: &[HelperRow]) -> (Vec<&str>, Vec<&str>) {
    let mut reads = Vec::new();
    let mut actions = Vec::new();
    for row in rows {
        match &row.class {
            HelperClass::Generated(GenKind::Read) => reads.push(row.name.as_str()),
            HelperClass::Generated(GenKind::Action) => actions.push(row.name.as_str()),
            _ => {}
        }
    }
    (reads, actions)
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
/// a string as a `String`; a bool as a `bool`; a float with the finite
/// check naming the argument (non-finite floats are a script error, and
/// a `"nan"` string that coerces is caught the same way); an int with the
/// saturating cast (never `as` wrap: -1 and 300 for a u8 or u32). A
/// malformed script argument IS an error naming the helper and the
/// argument (`hide_entity_from: allow: expected a boolean`).
/// `source: "rest"` is the read call's `MultiValue` (read through
/// `read_arg`); `source: "args"` is the queued action's `Table` (read
/// through `read_action_arg`, the same naming wrapper over a `Table`).
fn read_lines(row: &HelperRow, source: &str) -> Vec<String> {
    let names = arg_names(row);
    let mut out = Vec::new();
    for (i, (arg, local)) in row.args.iter().zip(&names).enumerate() {
        let at = i + 1;
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
        match arg.rust.as_str() {
            "&str" | "String" => out.push(format!("let {local}: String = {raw};")),
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
                    out.push("    return Ok(Value::Nil);".to_owned());
                    out.push("};".to_owned());
                }
            }
        }
    }
    out
}

/// The host call's arguments, in order: a player as its uuid, a string
/// borrowed, the rest as their converted locals.
fn call_args(row: &HelperRow) -> String {
    let names = arg_names(row);
    let mut parts = Vec::new();
    for (arg, local) in row.args.iter().zip(&names) {
        if arg.player {
            parts.push("uuid".to_owned());
        } else if arg.rust == "&str" {
            parts.push(format!("&{local}"));
        } else {
            parts.push(local.clone());
        }
    }
    parts.join(", ")
}

/// A scalar read back as a Lua value: the booleans and strings exact,
/// the integers in range (`Value::Integer` holds any i64), the floats as
/// numbers.
fn value_of(scalar: &str, expr: &str) -> String {
    match scalar {
        "bool" => format!("Value::Boolean({expr})"),
        "String" => format!("Value::String(lua.create_string({expr})?)"),
        "f32" | "f64" => format!("Value::Number(f64::from({expr}))"),
        _ => format!("Value::Integer(i64::from({expr}))"),
    }
}

/// The bridge of one read arm: a closure so `?` (the argument reads,
/// the finite checks) widens to the `Some(...)` the caller wants. A
/// malformed script argument is an error naming it; a nil return is nil.
fn read_arm(row: &HelperRow) -> String {
    let mut out = format!(
        "            {:?} => Some((|h: &dyn Host| -> mlua::Result<Value> {{\n",
        row.name
    );
    for line in read_lines(row, "rest") {
        out.push_str(&format!("                {line}\n"));
    }
    for line in player_fallback(row, GenKind::Read) {
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
/// queued actions), and the unresolvable-player rule (a silent no-op).
/// The arm is a closure, so `?` (the argument reads, the finite
/// checks) widens to the `Some(...)` the caller wants; `None` ("not
/// mine") stays the match's own.
fn action_arm(row: &HelperRow) -> String {
    let mut out = format!(
        "            {:?} => Some((|host: &mut dyn Host| -> mlua::Result<()> {{\n",
        row.name
    );
    for line in read_lines(row, "args") {
        out.push_str(&format!("                {line}\n"));
    }
    for line in player_fallback(row, GenKind::Action) {
        out.push_str(&format!("                {line}\n"));
    }
    let call = format!("host.{}({})", row.name, call_args(row));
    out.push_str(&format!(
        "                // A native refusal is ordinary: drop the return\n                // value, so later queued actions still run.\n                let _ = {call};\n                Ok(())\n",
    ));
    out.push_str("            })(host)),\n");
    out
}

/// `crates/event-horizon-lua/src/generated_helpers.rs`, the whole bridge.
fn bridge_of(rows: &[HelperRow]) -> String {
    let (reads, actions) = kinds_of(rows);
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
        "// Server-state only: the names stay out of `CLIENT_API`/`DRAW_API`, so a client\n",
    );
    out.push_str(
        "// state and a draw hook raise the server-only error, like the hand-written ones.\n",
    );
    out.push_str(
        "// The hand-written helpers it mirrors: `kick` (a player and a string, dropped\n",
    );
    out.push_str(
        "// bool), `give_experience` (the clamp-then-`as` saturating cast; every integer\n",
    );
    out.push_str("// argument here saturates the same way, never `as` wrap), `teleport_player`\n");
    out.push_str("// (non-finite floats are a script error naming the argument). A malformed\n");
    out.push_str("// script argument IS an error naming the argument; a host-side refusal (a\n");
    out.push_str("// `false`) is never a Lua error and never aborts the handler's remaining\n");
    out.push_str("// queued actions; an unresolvable player is a silent no-op (actions) or nil\n");
    out.push_str("// (reads, as `menu_contents` and `has_permission` do).\n\n");
    out.push_str("use crate::lua_mods::uuid_of;\n");
    out.push_str("use horizon_client::mod_api::Host;\n");
    out.push_str("use mlua::{Lua, MultiValue, Table, Value};\n\n");
    out.push_str("/// The queued actions this generator emits, installed beside the\n");
    out.push_str("/// hand-written ones.\n");
    out.push_str("pub(crate) const GENERATED_ACTIONS: &[&str] = &[\n");
    for name in &actions {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("];\n\n");
    out.push_str("/// The reads this generator emits, installed beside the\n");
    out.push_str("/// hand-written ones. `#[allow(dead_code)]`: the wrappers are the\n");
    out.push_str("/// Lua table's `READS_CODE` lines; this list is the doc's and the tests'\n");
    out.push_str("/// record of them.\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("pub(crate) const GENERATED_READS: &[&str] = &[\n");
    for name in &reads {
        out.push_str(&format!("    {name:?},\n"));
    }
    out.push_str("];\n\n");
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
    out.push_str("pub(crate) fn run_generated_action(\n");
    out.push_str("    host: &mut dyn Host,\n");
    out.push_str("    action: &str,\n");
    out.push_str("    args: &Table,\n");
    out.push_str(") -> Option<mlua::Result<()>> {\n");
    out.push_str("    match action {\n");
    for row in rows {
        if matches!(row.class, HelperClass::Generated(GenKind::Action)) {
            out.push_str(&action_arm(row));
        }
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n");
    out.push_str("}\n\n");
    out.push_str("/// Answers one generated read from the hook's `&self` host (`None`\n");
    out.push_str("/// means the read is not this generator's, and the caller keeps its own\n");
    out.push_str("/// `no such read` error).\n");
    out.push_str("pub(crate) fn generated_read(\n");
    out.push_str("    lua: &Lua,\n");
    out.push_str("    h: &dyn Host,\n");
    out.push_str("    what: &str,\n");
    out.push_str("    rest: MultiValue,\n");
    out.push_str(") -> Option<mlua::Result<Value>> {\n");
    out.push_str("    match what {\n");
    for row in rows {
        if matches!(row.class, HelperClass::Generated(GenKind::Read)) {
            out.push_str(&read_arm(row));
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
    for row in rows {
        out.push_str(&gen_host_method(row));
    }
    out.push_str("}\n");
    out
}

/// One `GenHost` method: it records the arguments it was handed (a
/// player as its uuid number, a string as its text, an int as the
/// saturated value, a float as its digits, a bool as `true`/`false`) and
/// returns a canned value (a read's canned value, or `None` when the
/// host is in its `none` mode).
fn gen_host_method(row: &HelperRow) -> String {
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
    let ret = if row.ret == "unit" {
        "()".to_owned()
    } else if row.ret_borrowed {
        match row.ret.as_str() {
            "String" => "&str".to_owned(),
            "Option<String>" => "Option<&str>".to_owned(),
            other => other.to_owned(),
        }
    } else {
        row.ret.clone()
    };
    let out = format!(
        "    fn {}(&{}, {}) -> {} {{\n",
        row.name,
        if row.receiver_mut { "mut self" } else { "self" },
        sig_params,
        ret
    );
    let mut out = out
        + &format!(
            "        self.record({:?}, &[{}]);\n",
            row.name,
            row.args
                .iter()
                .map(|a| format!("{}.to_string()", a.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
    out += match (&row.class, row.ret.as_str()) {
        (_, "unit") => "        ()\n",
        (HelperClass::Generated(GenKind::Action), "bool") => "        !self.refuse\n",
        (_, "bool") => "        true\n",
        (_, "String") if row.ret_borrowed => "        \"ruby:gen\"\n",
        (_, "String") => "        \"ruby:gen\".to_owned()\n",
        (_, "f64" | "f32") => "        1.5\n",
        (_, inner) if inner.starts_with("Option<") => {
            let scalar = inner
                .strip_prefix("Option<")
                .unwrap()
                .strip_suffix('>')
                .unwrap();
            let canned_value = if scalar == "String" && row.ret_borrowed {
                "\"ruby:gen\"".to_owned()
            } else {
                canned(scalar)
            };
            return format!(
                "{out}        if self.none {{ None }} else {{ Some({}) }}\n    }}\n",
                canned_value
            );
        }
        (_, scalar) => return format!("{out}        {}\n    }}\n", canned(scalar)),
    };
    out.push_str("    }\n");
    out
}

/// A canned value of a scalar, in the host double's terms.
fn canned(scalar: &str) -> String {
    match scalar {
        "bool" => "true".to_owned(),
        "String" => "\"ruby:gen\".to_owned()".to_owned(),
        "f32" | "f64" => "1.5".to_owned(),
        _ => "3".to_owned(),
    }
}

/// `crates/event-horizon-lua/src/generated_helpers_tests.rs`: one test
/// per emitted helper (the arguments reach the host exactly, a read
/// returns its canned value as the right Lua type, and nil for None; an
/// unresolvable player is a silent no-op/nil without calling the host;
/// a malformed script argument is an error naming it) plus the shared
/// client/draw raises and the drain test.
fn tests_of(rows: &[HelperRow]) -> String {
    let mut out = String::new();
    out.push_str("// Generated by `cargo run -p xtask -- lua-helpers`: do not edit.\n");
    out.push_str("// One test per emitted helper, over the `GenHost` recording double: the\n");
    out.push_str("// arguments reach the host exactly, a read returns its canned value as\n");
    out.push_str("// the right Lua type (and nil for None), an unresolvable player is a\n");
    out.push_str("// silent no-op/nil without calling the host, a malformed script argument\n");
    out.push_str("// is an error naming it, and the names raise in a client state and a draw\n");
    out.push_str("// hook. The drain test drives the real queue through a hook dispatch.\n\n");
    out.push_str("use crate::generated_helpers::{\n");
    out.push_str("    GENERATED_ACTIONS, GENERATED_READS, generated_read, run_generated_action,\n");
    out.push_str("};\n");
    out.push_str("use crate::lua_mods::{LuaMod, LuaSide};\n");
    out.push_str("use horizon_client::mod_api::Host;\n");
    out.push_str("use mlua::{Lua, Table};\n");
    out.push_str("use std::sync::Mutex;\n\n");
    out.push_str("const STEVE_UUID: u128 = 0x123e4567_e89b_12d3_a456_426614174000;\n\n");
    out.push_str(&gen_host(rows));
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
    out.push_str("\nfn gen_host(none: bool, refuse: bool) -> GenHost {\n");
    out.push_str("    GenHost {\n");
    out.push_str("        lines: Mutex::new(Vec::new()),\n");
    out.push_str("        none,\n");
    out.push_str("        refuse,\n");
    out.push_str("    }\n");
    out.push_str("}\n");
    out.push_str(hmod_helpers());
    for row in rows {
        out.push_str(&helper_test(row));
    }
    out.push_str(&shared_tests());
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
/// (which `uuid_of` reads back), and every other argument a distinct
/// value within the helper (the first bool `true`, the second `false`,
/// the third `true`, ...; strings `ruby:gen1`, `ruby:gen2`, ...; the
/// first int 3, the next 4, ...; the first float 1.5, the next 2.5, ...)
/// so a swap of two same-type reads changes the recorded line — and the
/// line the host records for it (the uuid as its number).
fn normal_case(row: &HelperRow) -> (Vec<String>, String) {
    let mut literals = Vec::new();
    let mut recorded = vec![row.name.clone()];
    let mut strings = 0;
    let mut bools = 0;
    let mut ints = 0;
    let mut floats = 0;
    for arg in &row.args {
        match arg.rust.as_str() {
            "u128" => {
                literals.push("mlua::Value::String(lua.create_string(\"123e4567-e89b-12d3-a456-426614174000\").unwrap())".to_owned());
                recorded.push("24249434048109030647017182301789831168".to_owned());
            }
            "&str" | "String" => {
                strings += 1;
                let text = format!("ruby:gen{strings}");
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
    (literals, recorded.join(" "))
}

/// The saturation edges of a helper's integer arguments: -1 and 300,
/// each pushed through the bridge and recorded as the saturated value
/// (`300` for a `u32`, `255` for a `u8`, `-1` for a signed type, and so
/// on — as the saturating cast hands it to the host).
fn edge_cases(row: &HelperRow) -> Vec<(Vec<String>, String)> {
    let mut out = Vec::new();
    for (value, _) in [(-1i64, ""), (300i64, "")] {
        let mut literals = Vec::new();
        let mut recorded = vec![row.name.clone()];
        for arg in &row.args {
            match arg.rust.as_str() {
                "u128" => {
                    literals.push("mlua::Value::String(lua.create_string(\"123e4567-e89b-12d3-a456-426614174000\").unwrap())".to_owned());
                    recorded.push("24249434048109030647017182301789831168".to_owned());
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
                ty => {
                    literals.push(format!("mlua::Value::Integer({value})"));
                    recorded.push(saturate(value, ty).to_string());
                }
            }
        }
        out.push((literals, recorded.join(" ")));
    }
    out
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

/// One helper's own test: the arguments reach the host exactly (strings,
/// saturated integers, a NaN on a float), an unresolvable player calls
/// nothing, and a read returns its canned value as the right Lua type
/// (and nil for None).
fn helper_test(row: &HelperRow) -> String {
    let mut out = format!("\n#[test]\nfn {}_is_generated_and_exact() {{\n", row.name);
    let (literals, recorded) = normal_case(row);
    match &row.class {
        HelperClass::Generated(GenKind::Action) => {
            out.push_str(&format!(
                "    let lua = lua();\n    let mut host = gen_host(false, false);\n    let t = args(&lua, &[{}]);\n    run_generated_action(&mut host, {:?}, &t).unwrap().unwrap();\n    assert_eq!(host.logged(), [{:?}]);\n",
                literals.join(", "),
                row.name,
                recorded
            ));
            if row.args.iter().any(is_int) {
                for (literals, recorded) in edge_cases(row) {
                    out.push_str(&format!(
                        "    {{\n        let mut host = gen_host(false, false);\n        let t = args(&lua, &[{}]);\n        run_generated_action(&mut host, {:?}, &t).unwrap().unwrap();\n        assert_eq!(host.logged(), [{:?}]);\n    }}\n",
                        literals.join(", "),
                        row.name,
                        recorded
                    ));
                }
            }
            if row.args.iter().any(|a| a.player) {
                let mut ghost = literals.clone();
                ghost[0] = "mlua::Value::String(lua.create_string(\"ghost\").unwrap())".to_owned();
                out.push_str(&format!(
                    "    {{\n        let mut host = gen_host(false, false);\n        let t = args(&lua, &[{}]);\n        run_generated_action(&mut host, {:?}, &t).unwrap().unwrap();\n        assert!(host.logged().is_empty(), \"an unresolvable player calls nothing\");\n    }}\n",
                    ghost.join(", "),
                    row.name
                ));
            }
            if let Some(arg) = row
                .args
                .iter()
                .find(|a| matches!(a.rust.as_str(), "f32" | "f64"))
            {
                let mut nan_lua = literals.clone();
                let pos = row.args.iter().position(|a| std::ptr::eq(a, arg)).unwrap();
                nan_lua[pos] = "mlua::Value::Number(f64::NAN)".to_owned();
                out.push_str(&format!(
                    "    {{\n        let mut host = gen_host(false, false);\n        let t = args(&lua, &[{}]);\n        let err = run_generated_action(&mut host, {:?}, &t).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    nan_lua.join(", "),
                    row.name,
                    format!("{}.{}", row.name, arg.name)
                ));
            }
            if let Some(pos) = row.args.iter().position(|a| !a.player && a.rust != "u128") {
                let arg = &row.args[pos];
                let mut wrong = literals.clone();
                wrong[pos] = "mlua::Value::Table(lua.create_table().unwrap())".to_owned();
                out.push_str(&format!(
                    "    {{\n        let mut host = gen_host(false, false);\n        let t = args(&lua, &[{}]);\n        let err = run_generated_action(&mut host, {:?}, &t).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    wrong.join(", "),
                    row.name,
                    row.name,
                    arg.name
                ));
            }
        }
        HelperClass::Generated(GenKind::Read) => {
            out.push_str(&format!(
                "    let lua = lua();\n    let h = gen_host(false, false);\n    let v = generated_read(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n    assert!(matches!(v, {}), \"{{v:?}}\");\n    assert_eq!(h.logged(), [{:?}]);\n",
                row.name,
                literals.join(", "),
                lua_type_of(row),
                recorded
            ));
            if row.ret.starts_with("Option<") {
                out.push_str(&format!(
                    "    {{\n        let h = gen_host(true, false);\n        let v = generated_read(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n        assert!(matches!(v, mlua::Value::Nil), \"{{v:?}}\");\n    }}\n",
                    row.name,
                    literals.join(", ")
                ));
            }
            if row.args.iter().any(is_int) {
                for (literals, recorded) in edge_cases(row) {
                    out.push_str(&format!(
                        "    {{\n        let h = gen_host(false, false);\n        let v = generated_read(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n        let _ = v;\n        assert_eq!(h.logged(), [{:?}]);\n    }}\n",
                        row.name,
                        literals.join(", "),
                        recorded
                    ));
                }
            }
            if row.args.iter().any(|a| a.player) {
                let mut ghost = literals.clone();
                ghost[0] = "mlua::Value::String(lua.create_string(\"ghost\").unwrap())".to_owned();
                out.push_str(&format!(
                    "    {{\n        let h = gen_host(false, false);\n        let v = generated_read(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap();\n        assert!(matches!(v, mlua::Value::Nil));\n        assert!(h.logged().is_empty());\n    }}\n",
                    row.name,
                    ghost.join(", ")
                ));
            }
            if let Some(arg) = row
                .args
                .iter()
                .find(|a| matches!(a.rust.as_str(), "f32" | "f64"))
            {
                let mut nan_lua = literals.clone();
                let pos = row.args.iter().position(|a| std::ptr::eq(a, arg)).unwrap();
                nan_lua[pos] = "mlua::Value::Number(f64::NAN)".to_owned();
                out.push_str(&format!(
                    "    {{\n        let h = gen_host(false, false);\n        let err = generated_read(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
                    row.name,
                    nan_lua.join(", "),
                    format!("{}.{}", row.name, arg.name)
                ));
            }
            if let Some(pos) = row.args.iter().position(|a| !a.player && a.rust != "u128") {
                let arg = &row.args[pos];
                let mut wrong = literals.clone();
                wrong[pos] = "mlua::Value::Table(lua.create_table().unwrap())".to_owned();
                out.push_str(&format!(
                    "    {{\n        let h = gen_host(false, false);\n        let err = generated_read(&lua, &h, {:?}, rest(&lua, &[{}])).unwrap().unwrap_err();\n        let text = err.to_string();\n        assert!(text.contains({:?}), \"{{text}}\");\n        assert!(text.contains({:?}), \"{{text}}\");\n    }}\n",
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

/// The Lua value type a canned read answers.
fn lua_type_of(row: &HelperRow) -> String {
    let scalar = row
        .ret
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(&row.ret);
    match scalar {
        "bool" => "mlua::Value::Boolean(true)".to_owned(),
        "String" => "mlua::Value::String(_)".to_owned(),
        "f32" | "f64" => "mlua::Value::Number(_)".to_owned(),
        _ => "mlua::Value::Integer(_)".to_owned(),
    }
}

/// The shared tests: the name lists are the spike set; the names raise
/// in a client state and a draw hook (the same stub behaviour as the
/// hand-written helpers); and the drain test — a helper whose host
/// answers `false` does not abort the handler's remaining queued
/// `broadcast` (the real queue, driven through a hook dispatch as the
/// net-send tests drive it).
fn shared_tests() -> String {
    r##"
#[test]
fn the_generated_lists_are_the_spike_set_and_nothing_else() {
    assert_eq!(
        GENERATED_ACTIONS,
        [
            "remove_effect",
            "set_attribute_base",
            "set_flight",
            "respawn_player",
            "resend_commands",
            "hide_entity_from",
            "sleep_ignored",
            "mob_stop",
            "mob_navigating",
            "refresh_player"
        ]
    );
    assert_eq!(
        GENERATED_READS,
        [
            "profile_by_uuid",
            "player_view_distance",
            "player_sneaking",
            "player_stat",
            "max_stack_size"
        ]
    );
}

#[test]
fn generated_names_raise_in_a_client_state() {
    let (dir, path) = hmod_as(
        "gen_client_stubs",
        &[(
            "scripts/client/init.lua",
            br#"
            local function expect_server_only(name, f)
              local ok, err = pcall(f)
              assert(not ok, name .. " should fail in a client state")
              local text = tostring(err)
              assert(
                string.find(text, "horizon." .. name .. " is only available in scripts/server/", 1, true),
                name .. ": " .. text
              )
            end
            expect_server_only("remove_effect", function() horizon.remove_effect("steve", "minecraft:poison") end)
            expect_server_only("profile_by_uuid", function() return horizon.profile_by_uuid("steve") end)
            expect_server_only("mob_stop", function() horizon.mob_stop("minecraft:overworld", 7) end)
            expect_server_only("max_stack_size", function() return horizon.max_stack_size("minecraft:stick") end)
            "#,
        )],
    );
    let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
    assert_eq!(m.side(), LuaSide::Client);
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
"##
        .to_owned()
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

/// `docs/api/helper-exposure.md`: every one of the 403 helpers with its
/// class and reason, grouped; the Adapter rows list their blocking
/// types. The header carries the spike's report: the per-class counts,
/// the adapter ranking with the conversion each type needs, how many
/// helpers the top few adapters would unblock, and what the generator
/// could not express.
fn exposure_of(rows: &[HelperRow], enabled: &[HelperRow]) -> String {
    let (generated, already, adapter, handle, cannot) = class_counts(rows);
    let enabled_count = enabled.len();
    let mut out = String::new();
    out.push_str("# Helper exposure\n\n");
    out.push_str("<!-- GENERATED by `cargo run -p xtask -- lua-helpers`; do not edit. `--check` fails when this file is not what the classifier says. -->\n\n");
    out.push_str("Every native host helper (`native-hooks.json` plus the pending `tools/native-hook-fragments/`, 403 in all) lands in exactly one class: `generated` (plain args and a supported return: the generator can emit the `horizon.*` bridge), `already-lua` (a hand-written Lua helper already stands behind it), `adapter` (a hand-written converter would unblock it: the table names the blocking types), `handle` (an opaque draw/mod handle, needing its own design), or `cannot` (no bridge, with the reason). `docs/api/hook-exposure.md` is the same idea for hook points. This page is `lua-helpers`'s human-readable record; `crates/event-horizon-lua/src/generated_helpers.rs` is its bridge for the spike set.\n\n");
    out.push_str(&format!(
        "{generated} generated ({enabled_count} enabled), {already} already-lua, {adapter} adapter, {handle} handle, {cannot} cannot ({} total). {} generated helpers are **not enabled**: the spike enables its allowlist ({}) and classifies the rest as eligible, not enabled yet.\n\n",
        rows.len(),
        generated - enabled_count,
        SPIKE_SET.join(", ")
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
    out.push_str("## Generated, not enabled yet\n\n");
    out.push_str("These all have plain args and a supported return (the generator can emit them) but are not in the spike's allowlist: their Lua cell in `docs/api/hook-points.md` stays empty, and they reach no one yet.\n\n");
    for row in rows {
        if matches!(row.class, HelperClass::Generated(_))
            && SPIKE_SET.iter().all(|name| *name != row.name)
        {
            out.push_str(&format!("| `{}` | {} |\n", row.name, row.doc));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    /// The `GenHost` double carries the native return: a unit action's
    /// method returns `()`, a `bool` action's returns `bool` (so enabling
    /// a unit helper compiles).
    #[test]
    fn gen_host_double_matches_the_native_return() {
        let rows = classify_helpers(&root()).unwrap();
        let unit = rows.iter().find(|r| r.name == "mod_screen_close").unwrap();
        let method = gen_host_method(unit);
        assert!(method.contains("fn mod_screen_close"), "{method}");
        assert!(method.contains("-> ()"), "{method}");
        let boolean = rows.iter().find(|r| r.name == "set_flight").unwrap();
        let method = gen_host_method(boolean);
        assert!(method.contains("fn set_flight"), "{method}");
        assert!(method.contains("-> bool"), "{method}");
    }
}
