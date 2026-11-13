//! The HUD pilot: `hud`, `gui_layer_post` and `gui_layer_pre` for scripted mods,
//! and the world-primitives builder of `world_primitives` (`primitives_table`).
//!
//! The first native hooks whose events carry borrowed render handles (the
//! `HudBatch` the handler draws into, the `Corners` it stacks boxes in) to
//! Lua, and the pattern the other handle-passing hooks (`screen_draw`,
//! `screen_render_post`, `tooltip_component_draw`, `item_decorations`,
//! `mob_submit`, `world_submit`, the `draw_scene` hooks) will follow:
//!
//! * The handle is a table of scoped functions (`Scope::create_function`,
//!   closures over the borrows, `Fn + 'scope`), built in a `Lua::scope`
//!   around the handler call. A handler that keeps `event.batch` and uses
//!   it after the call gets a Lua error (the scope expired), never
//!   undefined behaviour: the scope drops the callbacks, and calling one
//!   raises `CallbackDestructed`.
//! * Draw calls run the host's helpers at once, with the hook's `&dyn Host`
//!   (every helper takes `&self`): nothing is queued. Other actions a
//!   handler queues (`horizon.broadcast`...) run after the call, with the
//!   hook's `&mut dyn Host`, through `run_action`.
//! * Handlers read through the hook's `&dyn Host` (`Reader::Shared`),
//!   shared with the draw closures: every `&self` read (`horizon.player`,
//!   `horizon.time`, `horizon.mob`...) answers at once. Reads that need
//!   `&mut` (`world_data`, `player_data`, the acting player's inventory,
//!   a direct summon) are a Lua error naming the helper.
//! * A hook that fires every frame gets `FRAME_BUDGET`, not `HANDLER_BUDGET`.
//!   A handler that keeps missing it is disabled after
//!   `FRAME_OVERRUNS_UNTIL_DISABLED` consecutive misses, until `/reload`.
//!
//! Call convention: `event.batch.fill(...)`, with dots. The table holds
//! plain functions, so `event.batch:fill(...)` passes the table as the
//! first argument and fails its conversion: dot-call only. Rects and
//! positions are separate numbers (like the positions elsewhere), corners
//! are the `Corner` names (`top_left`, `top_right`, `bottom_left`,
//! `bottom_right`).
use crate::lua_mods::{LuaMod, is_overrun, marker_color, marker_int, marker_vec3, qualify};
use horizon_client::mod_api::{
    Corner, Corners, Host, HudBatch, ItemStack, PrimitiveShape, WorldPrimitives,
};
use horizon_client::world_primitives::MAX_BEAM_HEIGHT;
use mlua::{Error, Lua, Table, Value};
use std::cell::{Cell, RefCell};

/// The float arguments of a draw helper must be finite: a NaN or infinite
/// size or position from a script would reach native layout. Refuses the
/// first one that is not, naming it (`batch.fill`: `w`).
fn finite_args(helper: &str, args: &[(&str, f32)]) -> mlua::Result<()> {
    match args.iter().find(|(_, v)| !v.is_finite()) {
        Some((name, v)) => Err(Error::runtime(format!(
            "{helper}: `{name}` must be a finite number, got {v}"
        ))),
        None => Ok(()),
    }
}

/// The corner `name` (`top_left`...) names, or an error naming them all.
fn corner(name: &str) -> mlua::Result<Corner> {
    for c in Corner::ALL {
        if c.name() == name {
            return Ok(c);
        }
    }
    Err(Error::runtime(format!(
        "unknown corner {name:?}; the corners are top_left, top_right, bottom_left, bottom_right"
    )))
}

/// An item stack table (`{id, count, damage}`, as `item_table_of` hands
/// them out) as the host draws it: a plain stack of the qualified id.
fn stack_of(table: &Table) -> mlua::Result<ItemStack> {
    let id: String = table.get("id")?;
    Ok(ItemStack {
        item: qualify(&id).into(),
        count: table.get::<Option<u32>>("count")?.unwrap_or(1),
        damage: table.get::<Option<u32>>("damage")?.unwrap_or(0),
        extra: None,
    })
}

/// `batch.fill(x, y, w, h, argb)`: a rectangle in GUI pixels.
pub(crate) fn fill_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (f32, f32, f32, f32, u32)) -> mlua::Result<()> + 'a {
    move |_, (x, y, w, h, argb)| {
        finite_args("batch.fill", &[("x", x), ("y", y), ("w", w), ("h", h)])?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        host.fill(&mut borrowed, [x, y, w, h], argb);
        Ok(())
    }
}

/// `batch.text(text, x, y, rgb, shadow)`: a line in GUI pixels.
pub(crate) fn text_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, f32, f32, u32, bool)) -> mlua::Result<()> + 'a {
    move |_, (text, x, y, rgb, shadow)| {
        finite_args("batch.text", &[("x", x), ("y", y)])?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        host.text(&mut borrowed, &text, [x, y], rgb, shadow);
        Ok(())
    }
}

/// `batch.sprite(name, sx, sy, sw, sh, dx, dy, dw, dh)`: part `[sx, sy, sw,
/// sh]` of vanilla's `gui/...` sprite (or a content mod's `mymod:gui/...`
/// texture) drawn stretched into `[dx, dy, dw, dh]`. Whether it drew.
pub(crate) fn sprite_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, f32, f32, f32, f32, f32, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (name, sx, sy, sw, sh, dx, dy, dw, dh)| {
        finite_args(
            "batch.sprite",
            &[
                ("sx", sx),
                ("sy", sy),
                ("sw", sw),
                ("sh", sh),
                ("dx", dx),
                ("dy", dy),
                ("dw", dw),
                ("dh", dh),
            ],
        )?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.sprite(&mut borrowed, &name, [sx, sy, sw, sh], [dx, dy, dw, dh]))
    }
}

/// `batch.item(id, x, y, size)`: the item `id` (`diamond_sword`...), drawn
/// `size` GUI pixels square at (`x`, `y`). Whether it drew.
pub(crate) fn item_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (id, x, y, size)| {
        finite_args("batch.item", &[("x", x), ("y", y), ("size", size)])?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.item(&mut borrowed, &qualify(&id), [x, y], size))
    }
}

/// `batch.item_stack(stack, x, y, size)`: the stack (`{id, count, damage}`
/// as the hooks hand them out) at (`x`, `y`). Whether it drew.
pub(crate) fn item_stack_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (Table, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (stack, x, y, size)| {
        finite_args("batch.item_stack", &[("x", x), ("y", y), ("size", size)])?;
        let stack = stack_of(&stack)?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.item_stack(&mut borrowed, &stack, [x, y], size))
    }
}

/// `batch.item_model(item, model, x, y, size)`: `item` drawn with `model`.
/// Whether it drew.
pub(crate) fn item_model_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, String, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (item, model, x, y, size)| {
        finite_args("batch.item_model", &[("x", x), ("y", y), ("size", size)])?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.item_model(&mut borrowed, &qualify(&item), &model, [x, y], size))
    }
}

/// `batch.quad(x1, y1, x2, y2, x3, y3, x4, y4, argb)`: a filled quad.
pub(crate) fn quad_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (f32, f32, f32, f32, f32, f32, f32, f32, u32)) -> mlua::Result<()> + 'a {
    move |_, (x1, y1, x2, y2, x3, y3, x4, y4, argb)| {
        finite_args(
            "batch.quad",
            &[
                ("x1", x1),
                ("y1", y1),
                ("x2", x2),
                ("y2", y2),
                ("x3", x3),
                ("y3", y3),
                ("x4", x4),
                ("y4", y4),
            ],
        )?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        host.quad(
            &mut borrowed,
            [[x1, y1], [x2, y2], [x3, y3], [x4, y4]],
            argb,
        );
        Ok(())
    }
}

/// `batch.text_width(text)`: the width of `text` in GUI pixels.
pub(crate) fn text_width_fn<'a>(
    host: &'a dyn Host,
) -> impl Fn(&Lua, String) -> mlua::Result<f32> + 'a {
    move |_, text| Ok(host.text_width(&text))
}

/// `corners.place(corner, w, h)`: the top-left position of a `w` x `h` box
/// in `corner`, below (or above) what the corner already holds, as `x, y`.
pub(crate) fn place_fn<'a>(
    corners: &'a RefCell<&mut Corners>,
) -> impl Fn(&Lua, (String, f32, f32)) -> mlua::Result<(f32, f32)> + 'a {
    move |_, (name, w, h)| {
        finite_args("corners.place", &[("w", w), ("h", h)])?;
        let mut borrowed = corners.try_borrow_mut().map_err(|_| {
            Error::runtime("hud: the corners are already placed into (reentrant draw)")
        })?;
        Ok(borrowed.place(corner(&name)?, w, h))
    }
}

/// `corners.shown(corner)`: whether the corner is not taken (by a debug
/// overlay...): skip it when one `shown` denies.
pub(crate) fn shown_fn<'a>(host: &'a dyn Host) -> impl Fn(&Lua, String) -> mlua::Result<bool> + 'a {
    move |_, name| Ok(host.corner_shown(corner(&name)?))
}

/// The outcome of a frame hook's handler call: what the handlers queued
/// (nothing when they failed), for the hook to run with its `&mut` host.
pub(crate) type Queued = mlua::Result<Vec<(String, Table)>>;

/// Runs a frame hook's handlers, with the budget and the bookkeeping every
/// frame hook shares: a miss counts toward disabling (until `/reload`), a
/// clean run or a handler's own error resets the count. Handlers without a
/// registered handler cost nothing: no scope, no tables.
pub(crate) fn finish(mod_: &LuaMod, host: &mut dyn Host, hook: &str, outcome: Queued) {
    match outcome {
        Ok(actions) => {
            mod_.note_frame_outcome(hook, false);
            for (action, args) in actions {
                if let Err(e) = mod_.run_action(host, &action, &args) {
                    mod_.error(crate::lua_mods::handler_error_text(hook, &e));
                }
            }
        }
        Err(e) => {
            let overrun = is_overrun(&e);
            mod_.error(crate::lua_mods::handler_error_text(hook, &e));
            mod_.note_frame_outcome(hook, overrun);
        }
    }
}

/// Runs a cancelling frame hook's (`gui_layer_pre`) handlers like
/// `finish`, but a queued `horizon.cancel()` hides the vanilla layer
/// instead of reaching `run_action` (which would only warn `unknown
/// action cancel`): first cancel wins within the mod, and the native
/// dispatch stops at the first cancelling mod. Every other queued action
/// (`tell`, `broadcast`) still runs after the call. An error is never a
/// cancel: the frame-overrun count moves as in `finish`, and the firing
/// answers `Flow::Continue`.
pub(crate) fn finish_cancel(
    mod_: &LuaMod,
    host: &mut dyn Host,
    hook: &str,
    outcome: Queued,
) -> horizon_client::mod_api::Flow {
    use horizon_client::mod_api::Flow;
    match outcome {
        Ok(actions) => {
            mod_.note_frame_outcome(hook, false);
            let mut canceled = false;
            for (action, args) in actions {
                if action == "cancel" {
                    canceled = true;
                    continue;
                }
                if let Err(e) = mod_.run_action(host, &action, &args) {
                    mod_.error(crate::lua_mods::handler_error_text(hook, &e));
                }
            }
            if canceled {
                Flow::Cancel
            } else {
                Flow::Continue
            }
        }
        Err(e) => {
            let overrun = is_overrun(&e);
            mod_.error(crate::lua_mods::handler_error_text(hook, &e));
            mod_.note_frame_outcome(hook, overrun);
            Flow::Continue
        }
    }
}

/// The most primitives one handler call may draw through its `primitives`
/// table, counted per frame and per mod. The renderer has no cap of its own
/// (its buffers grow with the list), so this is the engine's bound on world
/// markers, `MAX_MARKERS_TOTAL` in `game/native_markers.rs`: a mod may not
/// add more than the whole level's markers may.
pub(crate) const MAX_PRIMITIVES_PER_CALL: usize = 4096;

/// Which optional settings a primitive call took (`{ through_walls, ... }`).
#[derive(Clone, Copy, Default)]
struct PrimitiveOptions {
    through_walls: bool,
    scale_with_distance: bool,
    background: bool,
}

/// The optional options table of a primitive call: a boolean per name in
/// `allowed` (any other key is an error naming the call and the key). Absent
/// or nil means false; a value that is neither nil nor a table is an error
/// naming the call and `options`.
fn primitive_options(
    helper: &str,
    opts: Value,
    allowed: &[&str],
) -> mlua::Result<PrimitiveOptions> {
    let mut out = PrimitiveOptions::default();
    let table = match opts {
        Value::Nil => return Ok(out),
        Value::Table(table) => table,
        other => {
            return Err(Error::runtime(format!(
                "{helper}: options: must be a table or nil, got {}",
                other.type_name()
            )));
        }
    };
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|e| Error::runtime(format!("{helper}: options: {e}")))?;
        let key = match &key {
            Value::String(s) => s.to_string_lossy(),
            other => {
                return Err(Error::runtime(format!(
                    "{helper}: options: keys must be names, got {}",
                    other.type_name()
                )));
            }
        };
        if !allowed.contains(&key.as_str()) {
            return Err(Error::runtime(format!(
                "{helper}: {key}: unknown option (this call takes {})",
                allowed.join(", ")
            )));
        }
        let on = match value {
            Value::Nil => false,
            Value::Boolean(b) => b,
            other => {
                return Err(Error::runtime(format!(
                    "{helper}: {key}: must be a boolean, got {}",
                    other.type_name()
                )));
            }
        };
        match key.as_str() {
            "through_walls" => out.through_walls = on,
            "scale_with_distance" => out.scale_with_distance = on,
            _ => out.background = on,
        }
    }
    Ok(out)
}

/// A position argument (`{x, y, z}`) named `helper`'s `field`.
fn primitive_point(helper: &str, field: &str, v: &Value) -> mlua::Result<[f64; 3]> {
    match v {
        Value::Table(t) => marker_vec3(helper, field, t),
        Value::Nil => Err(Error::runtime(format!(
            "{helper}: {field}: missing (a table {{x, y, z}})"
        ))),
        other => Err(Error::runtime(format!(
            "{helper}: {field}: must be a table {{x, y, z}}, got {}",
            other.type_name()
        ))),
    }
}

/// A colour argument (an ARGB integer or `{r, g, b, a?}`) named `helper`'s `color`.
fn primitive_color(helper: &str, v: &Value) -> mlua::Result<u32> {
    match v {
        Value::Nil => Err(Error::runtime(format!(
            "{helper}: color: missing (an ARGB integer or {{r, g, b, a?}})"
        ))),
        v => marker_color(helper, v),
    }
}

/// A size argument: a finite number of blocks or pixels, not negative.
fn primitive_size(helper: &str, field: &str, v: &Value) -> mlua::Result<f32> {
    let n = match v {
        Value::Integer(i) => *i as f64,
        Value::Number(n) => *n,
        other => {
            return Err(Error::runtime(format!(
                "{helper}: {field}: must be a finite number, got {}",
                other.type_name()
            )));
        }
    };
    // The sign is tested on the f64: a tiny negative rounds to `-0.0` as an f32.
    let size = n as f32;
    if !n.is_finite() || !size.is_finite() || n < 0.0 {
        return Err(Error::runtime(format!(
            "{helper}: {field}: must be a finite number that is not negative, got {n:e}"
        )));
    }
    // `-0.0` is zero: it draws as `+0.0`.
    Ok(if size == 0.0 { 0.0 } else { size })
}

/// A non-empty string argument (text, a sprite name) named `helper`'s `field`.
fn primitive_name(helper: &str, field: &str, v: &Value) -> mlua::Result<String> {
    match v {
        Value::String(s) => {
            let text = s.to_string_lossy();
            if text.is_empty() {
                return Err(Error::runtime(format!(
                    "{helper}: {field}: must not be empty"
                )));
            }
            Ok(text)
        }
        Value::Nil => Err(Error::runtime(format!(
            "{helper}: {field}: missing (a string)"
        ))),
        other => Err(Error::runtime(format!(
            "{helper}: {field}: must be a string, got {}",
            other.type_name()
        ))),
    }
}

/// Adds one primitive to the frame's list, through the per-call cap. A
/// primitive past the cap is an error naming the call and the cap; the
/// primitives drawn before it stay drawn.
fn push_primitive(
    helper: &str,
    primitives: &RefCell<&mut WorldPrimitives>,
    drawn: &Cell<usize>,
    shape: PrimitiveShape,
    options: PrimitiveOptions,
) -> mlua::Result<()> {
    if drawn.get() >= MAX_PRIMITIVES_PER_CALL {
        return Err(Error::runtime(format!(
            "{helper}: a handler may draw at most {MAX_PRIMITIVES_PER_CALL} primitives per frame"
        )));
    }
    let mut borrowed = primitives.try_borrow_mut().map_err(|_| {
        Error::runtime(format!(
            "{helper}: the primitives are already drawn into (reentrant draw)"
        ))
    })?;
    let primitive = borrowed.push(shape);
    if options.through_walls {
        primitive.through_walls();
    }
    if options.scale_with_distance {
        primitive.scale_with_distance();
    }
    if options.background {
        primitive.background();
    }
    drawn.set(drawn.get() + 1);
    Ok(())
}

/// The `primitives` table of a `world_primitives` handler's event: scoped
/// functions over the frame's native list, each drawing at once. Positions
/// are `{x, y, z}` tables, colours ARGB integers or `{r, g, b, a?}`, and the
/// last argument of each call an optional options table. The table expires
/// with the call.
pub(crate) fn primitives_table<'s, 'e: 's, 'h: 's>(
    scope: &'s mlua::Scope<'s, 'e>,
    lua: &Lua,
    primitives: &'h RefCell<&mut WorldPrimitives>,
    drawn: &'h Cell<usize>,
) -> mlua::Result<Table> {
    const ALL: &[&str] = &["through_walls"];
    const TEXT: &[&str] = &["through_walls", "scale_with_distance", "background"];
    let table = lua.create_table()?;
    table.set(
        "line",
        scope.create_function(
            move |_, (from, to, color, width, opts): (Value, Value, Value, Value, Value)| {
                const H: &str = "primitives.line";
                let shape = PrimitiveShape::Line {
                    from: primitive_point(H, "from", &from)?,
                    to: primitive_point(H, "to", &to)?,
                    color: primitive_color(H, &color)?,
                    width: primitive_size(H, "width", &width)?,
                };
                push_primitive(
                    H,
                    primitives,
                    drawn,
                    shape,
                    primitive_options(H, opts, ALL)?,
                )
            },
        )?,
    )?;
    table.set(
        "box_outline",
        scope.create_function(
            move |_, (min, max, color, width, opts): (Value, Value, Value, Value, Value)| {
                const H: &str = "primitives.box_outline";
                let shape = PrimitiveShape::BoxOutline {
                    min: primitive_point(H, "min", &min)?,
                    max: primitive_point(H, "max", &max)?,
                    color: primitive_color(H, &color)?,
                    width: primitive_size(H, "width", &width)?,
                };
                push_primitive(
                    H,
                    primitives,
                    drawn,
                    shape,
                    primitive_options(H, opts, ALL)?,
                )
            },
        )?,
    )?;
    table.set(
        "filled_box",
        scope.create_function(
            move |_, (min, max, color, opts): (Value, Value, Value, Value)| {
                const H: &str = "primitives.filled_box";
                let shape = PrimitiveShape::FilledBox {
                    min: primitive_point(H, "min", &min)?,
                    max: primitive_point(H, "max", &max)?,
                    color: primitive_color(H, &color)?,
                };
                push_primitive(
                    H,
                    primitives,
                    drawn,
                    shape,
                    primitive_options(H, opts, ALL)?,
                )
            },
        )?,
    )?;
    table.set(
        "beam",
        scope.create_function(
            move |_, (base, height, color, opts): (Value, Value, Value, Value)| {
                const H: &str = "primitives.beam";
                let shape = PrimitiveShape::Beam {
                    base: primitive_point(H, "base", &base)?,
                    height: marker_int(H, "height", &height, 0, MAX_BEAM_HEIGHT as i64)? as u32,
                    color: primitive_color(H, &color)?,
                };
                push_primitive(
                    H,
                    primitives,
                    drawn,
                    shape,
                    primitive_options(H, opts, ALL)?,
                )
            },
        )?,
    )?;
    table.set(
        "text",
        scope.create_function(
            move |_, (at, text, color, scale, opts): (Value, Value, Value, Value, Value)| {
                const H: &str = "primitives.text";
                let shape = PrimitiveShape::Text {
                    at: primitive_point(H, "at", &at)?,
                    text: primitive_name(H, "text", &text)?,
                    color: primitive_color(H, &color)?,
                    scale: primitive_size(H, "scale", &scale)?,
                    scale_with_distance: false,
                    background: false,
                };
                push_primitive(
                    H,
                    primitives,
                    drawn,
                    shape,
                    primitive_options(H, opts, TEXT)?,
                )
            },
        )?,
    )?;
    table.set(
        "sprite",
        scope.create_function(
            move |_, (at, sprite, size, color, opts): (Value, Value, Value, Value, Value)| {
                const H: &str = "primitives.sprite";
                let shape = PrimitiveShape::Sprite {
                    at: primitive_point(H, "at", &at)?,
                    sprite: primitive_name(H, "sprite", &sprite)?,
                    size: primitive_size(H, "size", &size)?,
                    color: primitive_color(H, &color)?,
                };
                push_primitive(
                    H,
                    primitives,
                    drawn,
                    shape,
                    primitive_options(H, opts, ALL)?,
                )
            },
        )?,
    )?;
    Ok(table)
}

/// The outcome of a read-only frame hook's (`world_primitives`) handlers: the
/// frame budget counts as in `finish_cancel`. The hook's `&self` receiver has
/// no mutable host, so a queued action (`tell`, `broadcast`) cannot run: it is
/// reported as an error, once per queued action, and nothing else changes.
pub(crate) fn finish_read_only(mod_: &LuaMod, hook: &str, outcome: Queued) {
    match outcome {
        Ok(actions) => {
            mod_.note_frame_outcome(hook, false);
            for (action, _) in actions {
                mod_.error(format!(
                    "{hook}: horizon.{action} is not available here: this hook only draws"
                ));
            }
        }
        Err(e) => {
            let overrun = is_overrun(&e);
            mod_.error(crate::lua_mods::handler_error_text(hook, &e));
            mod_.note_frame_outcome(hook, overrun);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua_mods::{LuaMod, LuaSide};
    use horizon_client::dev_input::mods::{ModStatus, set_status, status};
    use horizon_client::mobs::{Mob, MobType};
    use horizon_client::mod_api::{
        BlockStateId, EntityRef, Hooks, HudEvent, ItemStack, MobView, NativeHeightmap, NativeHit,
        NativeServerPlayer, NativeWorldEpoch, PrimitiveShape, Text, WorldPrimitives,
        dispatch_gui_layer_post, dispatch_gui_layer_pre, dispatch_hud, dispatch_world_primitives,
    };
    use horizon_client::test_events;
    use horizon_core::geometry::Vec3;
    use horizon_mods::Package;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// A game host that records what the handler drew instead of drawing
    /// it: every draw helper appends a line, `broadcast` counts, `text`
    /// is 6 pixels a character, and only `top_left` is shown. The reads a
    /// draw hook may serve come from `players` (the first one acts, with
    /// their main hand from `held`), `stacks` (a non-acting player's slots,
    /// by uuid), `day_time`, `weather` and one cow (mob 7); `tell` appends
    /// a line.
    #[derive(Default)]
    struct RecordingHost {
        draws: Mutex<Vec<String>>,
        broadcasts: Mutex<u64>,
        tells: Mutex<Vec<String>>,
        players: Vec<NativeServerPlayer>,
        stacks: HashMap<u128, Vec<Option<ItemStack>>>,
        held: HashMap<u128, ItemStack>,
        day_time: i64,
        weather: (f32, f32),
    }

    impl Host for RecordingHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }

        fn level_players(&self) -> Vec<NativeServerPlayer> {
            self.players.clone()
        }

        fn state_name(&self, state: BlockStateId) -> String {
            format!("test:state{}", state.0)
        }

        fn entity(&self, id: u32) -> Option<EntityRef> {
            Some(EntityRef::new(NativeWorldEpoch::new(0), id))
        }

        fn entity_kind(&self, _entity: EntityRef) -> Option<String> {
            None
        }

        fn acting_player(&self) -> Option<NativeServerPlayer> {
            self.players.first().cloned()
        }

        fn day_time(&self) -> i64 {
            self.day_time
        }

        fn weather(&self) -> (f32, f32) {
            self.weather
        }

        fn mob_view(&self, dimension: &str, id: u32) -> Option<MobView> {
            (dimension == "minecraft:overworld" && id == 7).then(|| {
                MobView::of(
                    &Mob::new(7, MobType::cow_for_tests(), Vec3::new(2.0, 64.0, 3.0), 0.0),
                    None,
                )
            })
        }

        fn mob_views(&self, dimension: &str) -> Vec<MobView> {
            self.mob_view(dimension, 7).into_iter().collect()
        }

        fn player_stacks(&self, uuid: u128) -> Option<Vec<Option<ItemStack>>> {
            self.stacks.get(&uuid).cloned()
        }

        fn held_stack(&self, uuid: u128) -> Option<ItemStack> {
            self.held.get(&uuid).cloned()
        }

        fn tell(&mut self, uuid: u128, line: Text) -> bool {
            self.tells.lock().unwrap().push(format!("{uuid} {line:?}"));
            true
        }

        fn fill(&self, _batch: &mut HudBatch, rect: [f32; 4], argb: u32) {
            self.draws
                .lock()
                .unwrap()
                .push(format!("fill {rect:?} {argb:#010x}"));
        }

        fn text(&self, _batch: &mut HudBatch, text: &str, at: [f32; 2], rgb: u32, shadow: bool) {
            self.draws
                .lock()
                .unwrap()
                .push(format!("text {text:?} at {at:?} {rgb:#08x} {shadow}"));
        }

        fn sprite(&self, _batch: &mut HudBatch, name: &str, src: [f32; 4], dst: [f32; 4]) -> bool {
            self.draws
                .lock()
                .unwrap()
                .push(format!("sprite {name:?} {src:?} {dst:?}"));
            true
        }

        fn item(&self, _batch: &mut HudBatch, item: &str, at: [f32; 2], size: f32) -> bool {
            self.draws
                .lock()
                .unwrap()
                .push(format!("item {item:?} at {at:?} size {size}"));
            true
        }

        fn item_stack(
            &self,
            _batch: &mut HudBatch,
            stack: &ItemStack,
            at: [f32; 2],
            size: f32,
        ) -> bool {
            self.draws.lock().unwrap().push(format!(
                "item_stack {:?} x{} @{at:?} size {size}",
                stack.item, stack.count,
            ));
            assert_eq!(stack.damage, 3, "the test's damage");
            true
        }

        fn item_model(
            &self,
            _batch: &mut HudBatch,
            item: &str,
            model: &str,
            at: [f32; 2],
            size: f32,
        ) -> bool {
            self.draws.lock().unwrap().push(format!(
                "item_model {item:?} {model:?} at {at:?} size {size}"
            ));
            true
        }

        fn quad(&self, _batch: &mut HudBatch, points: [[f32; 2]; 4], argb: u32) {
            self.draws
                .lock()
                .unwrap()
                .push(format!("quad {points:?} {argb:#010x}"));
        }

        fn text_width(&self, text: &str) -> f32 {
            text.len() as f32 * 6.0
        }

        fn corner_shown(&self, corner: Corner) -> bool {
            corner == Corner::TopLeft
        }

        fn broadcast(&self, _line: Text) {
            *self.broadcasts.lock().unwrap() += 1;
        }
    }

    const MANIFEST_TEXT: &str = "id = \"ruby_hud\"\nname = \"Ruby HUD\"\nversion = \"0.1.0\"\napi = \"0.2\"\nside = \"both\"\ncapabilities = [\"scripts\"]\nexperimental = [\"scripts\"]\n";

    /// A mod running `lua`, with a clean status row: `last_error` then tells
    /// a handler's `assert` from silence. The id is unique per test, so the
    /// shared status store cannot mix tests up.
    fn hmod(name: &str, id: &str, lua: &str) -> (LuaMod, String, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("horizon-lua-hud-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ruby.hmod");
        let manifest = MANIFEST_TEXT.replacen("ruby_hud", id, 1);
        std::fs::write(
            &path,
            Package::write_files(
                &manifest,
                None,
                &[("scripts/client/init.lua", lua.as_bytes())],
            )
            .unwrap(),
        )
        .unwrap();
        let m = LuaMod::load(&path, LuaSide::Client, &[]).unwrap();
        set_status(ModStatus {
            id: id.to_owned(),
            side: m.side().as_str().to_owned(),
            ..Default::default()
        });
        (m, id.to_owned(), dir)
    }

    fn no_error(id: &str) {
        assert!(
            last_error(id).is_none(),
            "a handler failed: {:?}",
            last_error(id)
        );
    }

    fn last_error(id: &str) -> Option<String> {
        status()
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.error)
    }

    /// The player the draw-hook reads answer with: values off every
    /// default, so a test can tell a real answer from an empty one.
    fn draw_player() -> NativeServerPlayer {
        test_events::native_server_player(
            0x123e4567_e89b_12d3_a456_426614174001,
            "draw_steve".into(),
            0,
            "minecraft:overworld".into(),
            [10.5, 65.0, -3.25],
            0,
            20.0,
            0.0,
            0.0,
            0,
            String::new(),
        )
    }

    /// A second player, never the acting one: the non-acting inventory
    /// path reads their stacks from the shared host.
    fn other_player() -> NativeServerPlayer {
        test_events::native_server_player(
            0x123e4567_e89b_12d3_a456_426614174002,
            "alex".into(),
            0,
            "minecraft:overworld".into(),
            [1.5, 65.0, 7.25],
            0,
            20.0,
            0.0,
            0.0,
            0,
            String::new(),
        )
    }

    /// The window that draws the hud, as a client sees it (a client even
    /// in single player): no players listed, no acting player, no mob in
    /// view, raycasts unanswered, and heightmaps an error; the client's
    /// own world still answers `dimension`, `day_time`, `weather`,
    /// `block`, `block_property` and `light`. Characterization of today's
    /// client behaviour: the native side may change this (#47 follow-up),
    /// and these tests pin what the Lua crate must keep serving without
    /// an error or a panic meanwhile.
    #[derive(Default)]
    struct ClientWindowHost {
        draws: Mutex<Vec<String>>,
        day_time: i64,
        weather: (f32, f32),
    }

    impl Host for ClientWindowHost {
        fn dimension(&self) -> &'static str {
            "minecraft:overworld"
        }

        fn level_players(&self) -> Vec<NativeServerPlayer> {
            Vec::new()
        }

        fn acting_player(&self) -> Option<NativeServerPlayer> {
            None
        }

        fn day_time(&self) -> i64 {
            self.day_time
        }

        fn weather(&self) -> (f32, f32) {
            self.weather
        }

        fn block(&self, _pos: [i32; 3]) -> String {
            "minecraft:stone".to_owned()
        }

        fn block_property(&self, _pos: [i32; 3], _key: &str) -> Option<String> {
            Some("true".to_owned())
        }

        fn light(&self, _pos: [i32; 3]) -> (u8, u8) {
            (12, 15)
        }

        fn mob_view(&self, _dimension: &str, _id: u32) -> Option<MobView> {
            None
        }

        fn mob_views(&self, _dimension: &str) -> Vec<MobView> {
            Vec::new()
        }

        fn player_raycast(&self, _uuid: u128, _distance: f32) -> Option<NativeHit> {
            None
        }

        fn surface_heights(
            &self,
            _dimension: &str,
            _columns: &[(i32, i32)],
            _heightmap: NativeHeightmap,
        ) -> Result<Vec<Option<i32>>, String> {
            Err("a client window has no heightmap".to_owned())
        }

        fn fill(&self, _batch: &mut HudBatch, rect: [f32; 4], argb: u32) {
            self.draws
                .lock()
                .unwrap()
                .push(format!("fill {rect:?} {argb:#010x}"));
        }

        fn text(&self, _batch: &mut HudBatch, text: &str, at: [f32; 2], rgb: u32, shadow: bool) {
            self.draws
                .lock()
                .unwrap()
                .push(format!("text {text:?} at {at:?} {rgb:#08x} {shadow}"));
        }
    }

    fn boxed(m: LuaMod) -> Vec<Box<dyn Hooks>> {
        vec![Box::new(m) as Box<dyn Hooks>]
    }

    /// A `LuaMod` shared with the test, so the test can reload it between
    /// dispatches (which take `Box<dyn Hooks>`).
    struct Shared(Arc<Mutex<LuaMod>>);

    impl Hooks for Shared {
        fn hud(&self, host: &mut dyn Host, event: &mut HudEvent<'_>) {
            Hooks::hud(&*self.0.lock().unwrap(), host, event);
        }
    }

    #[test]
    fn a_hud_handler_draws_and_stacks_boxes() {
        let (m, id, dir) = hmod(
            "draws",
            "ruby_hud_draws",
            r#"
            horizon.on("hud", function(event)
              local batch = event.batch
              local x, y = event.corners.place("top_left", 100, 20)
              batch.fill(x, y, 100, 20, 4278190080)
              batch.text("hi", 10, 20, 16777215, true)
              assert(batch.text_width("hi") == 12)
              assert(batch.sprite("gui/widgets/button", 0, 0, 200, 20, 10, 10, 200, 20) == true)
              assert(batch.item("stick", 5, 5, 16) == true)
              assert(batch.item_stack({id = "stick", count = 2, damage = 3}, 6, 6, 16) == true)
              assert(batch.item_model("stick", "minecraft:item/handheld", 7, 7, 16) == true)
              batch.quad(0, 0, 1, 0, 1, 1, 0, 1, 4278190080)
              if event.corners.shown("top_left") then batch.fill(0, 0, 1, 1, 1) end
              if event.corners.shown("bottom_right") then batch.fill(0, 0, 2, 2, 2) end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        let (x, y) = Corners::new(800.0, 600.0, 0.0).place(Corner::TopLeft, 100.0, 20.0);
        assert_eq!(
            *host.draws.lock().unwrap(),
            [
                format!("fill {:?} 0xff000000", [x, y, 100.0, 20.0]),
                "text \"hi\" at [10.0, 20.0] 0xffffff true".to_owned(),
                "sprite \"gui/widgets/button\" [0.0, 0.0, 200.0, 20.0] [10.0, 10.0, 200.0, 20.0]"
                    .to_owned(),
                "item \"minecraft:stick\" at [5.0, 5.0] size 16".to_owned(),
                "item_stack \"minecraft:stick\" x2 @[6.0, 6.0] size 16".to_owned(),
                "item_model \"minecraft:stick\" \"minecraft:item/handheld\" at [7.0, 7.0] size 16"
                    .to_owned(),
                "quad [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] 0xff000000".to_owned(),
                "fill [0.0, 0.0, 1.0, 1.0] 0x00000001".to_owned(),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unknown_corner_is_an_error() {
        let (m, id, dir) = hmod(
            "corner",
            "ruby_hud_corner",
            r#"horizon.on("hud", function(event) event.corners.place("middle", 1, 1) end)"#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        let e = last_error(&id).expect("the handler failed");
        assert!(e.contains("unknown corner"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_gui_layer_post_handler_sees_its_layer() {
        let (m, id, dir) = hmod(
            "layer",
            "ruby_hud_layer",
            r#"
            horizon.on("gui_layer_post", function(event)
              if event.layer == "test:layer" then event.batch.fill(1, 2, 3, 4, 4278190080) end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        dispatch_gui_layer_post(&mut host, &mods, "test:layer", &mut batch);
        no_error(&id);
        dispatch_gui_layer_post(&mut host, &mods, "other:layer", &mut batch);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [1.0, 2.0, 3.0, 4.0] 0xff000000"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The `gui_layer_pre` twin of the layer test above: the handler draws
    /// below its layer through the same scoped `batch`, and the layer id
    /// filters in Lua. Nothing is cancelled: the native flag stays down.
    #[test]
    fn a_gui_layer_pre_handler_sees_its_layer() {
        let (m, id, dir) = hmod(
            "prelayer",
            "ruby_hud_prelayer",
            r#"
            horizon.on("gui_layer_pre", function(event)
              if event.layer == "test:layer" then event.batch.fill(1, 2, 3, 4, 4278190080) end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        no_error(&id);
        assert!(!canceled, "drawing below the layer hides nothing");
        dispatch_gui_layer_pre(&mut host, &mods, "other:layer", &mut batch, &mut canceled);
        no_error(&id);
        assert!(!canceled, "the other layer hides nothing either");
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [1.0, 2.0, 3.0, 4.0] 0xff000000"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `horizon.cancel()` in a `gui_layer_pre` handler hides the vanilla
    /// layer: the queued cancel reaches the native `canceled` flag, while
    /// the drawing below it still reaches the batch.
    #[test]
    fn a_gui_layer_pre_cancel_hides_the_layer() {
        let (m, id, dir) = hmod(
            "precancel",
            "ruby_hud_precancel",
            r#"
            horizon.on("gui_layer_pre", function(event)
              event.batch.fill(0, 0, 1, 1, 4278190080)
              horizon.cancel()
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        no_error(&id);
        assert!(canceled, "the queued cancel hid the layer");
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [0.0, 0.0, 1.0, 1.0] 0xff000000"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// First cancel wins across mods: the native dispatch stops at the
    /// first cancelling mod, so a later mod's handler never runs for the
    /// hidden layer.
    #[test]
    fn a_gui_layer_pre_first_cancel_wins() {
        let (first, first_id, first_dir) = hmod(
            "precancel_first",
            "ruby_hud_precancel_first",
            r#"horizon.on("gui_layer_pre", function(event) horizon.cancel() end)"#,
        );
        let (second, second_id, second_dir) = hmod(
            "precancel_second",
            "ruby_hud_precancel_second",
            r#"
            horizon.on("gui_layer_pre", function(event)
              __witness = (__witness or 0) + 1
              event.batch.fill(0, 0, 1, 1, 4278190080)
            end)
            "#,
        );
        let second_state = second.lua.clone();
        let mods = vec![
            Box::new(first) as Box<dyn Hooks>,
            Box::new(second) as Box<dyn Hooks>,
        ];
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        no_error(&first_id);
        no_error(&second_id);
        assert!(canceled, "the first mod hid the layer");
        assert_eq!(
            second_state.globals().get::<i64>("__witness").unwrap_or(0),
            0,
            "the hidden layer never reached the second mod"
        );
        assert!(
            host.draws.lock().unwrap().is_empty(),
            "nothing drew below the hidden layer"
        );
        std::fs::remove_dir_all(&first_dir).unwrap();
        std::fs::remove_dir_all(&second_dir).unwrap();
    }

    /// A `gui_layer_pre` handler that never cancels leaves the layer alone:
    /// the flag stays down and a queued `tell` still runs after the call.
    #[test]
    fn a_gui_layer_pre_without_cancel_keeps_the_layer() {
        let (m, id, dir) = hmod(
            "prenocancel",
            "ruby_hud_prenocancel",
            r#"
            horizon.on("gui_layer_pre", function(event)
              horizon.tell("draw_steve", "below the layer")
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost {
            players: vec![draw_player()],
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        no_error(&id);
        assert!(!canceled, "no cancel queued, nothing hidden");
        let tells = host.tells.lock().unwrap();
        assert_eq!(tells.len(), 1);
        assert!(tells[0].contains("below the layer"), "{}", tells[0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `gui_layer_pre` batch kept past its call is an error, as the
    /// `hud` one is: the scope drops the callbacks, and calling one raises
    /// `CallbackDestructed` instead of drawing.
    #[test]
    fn a_pre_batch_kept_past_its_call_is_an_error() {
        let (m, id, dir) = hmod(
            "prekept",
            "ruby_hud_prekept",
            r#"
            horizon.on("gui_layer_pre", function(event)
              if SAVED == nil then SAVED = event.batch
              else SAVED.fill(0, 0, 1, 1, 4278190080) end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        no_error(&id);
        assert!(!canceled, "keeping the batch hides nothing");
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        let e = last_error(&id).expect("the kept batch failed");
        assert!(e.contains("destructed"), "{e}");
        assert!(
            host.draws.lock().unwrap().is_empty(),
            "the expired batch drew nothing"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An infinite `gui_layer_pre` handler is stopped within the frame
    /// budget, like the `hud` one: a per-frame hook never gets the 250 ms
    /// handler budget.
    #[test]
    fn an_infinite_gui_layer_pre_handler_is_stopped_within_the_frame_budget() {
        let (m, id, dir) = hmod(
            "preloop",
            "ruby_hud_preloop",
            r#"horizon.on("gui_layer_pre", function() while true do end end)"#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        let started = Instant::now();
        dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        let elapsed = started.elapsed();
        println!("gui_layer_pre infinite loop stopped after {elapsed:?}");
        // The frame budget stopped it, not the 250 ms tick budget.
        assert!(
            elapsed < Duration::from_millis(100),
            "stopped after {elapsed:?}"
        );
        assert!(!canceled, "a stopped handler hides nothing");
        let e = last_error(&id).expect("the loop failed");
        assert!(e.contains("ran over its time budget"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `gui_layer_pre` dispatch nobody hears costs no Lua work: no
    /// scope, no tables, like the unheard `hud` hook.
    #[test]
    fn an_unregistered_gui_layer_pre_hook_costs_no_lua_work() {
        let (m, _id, dir) = hmod(
            "preidle",
            "ruby_hud_preidle",
            "horizon.on(\"gui_layer_post\", function() end)",
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut canceled = false;
        for _ in 0..10 {
            dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        }
        assert!(!canceled, "unheard, nothing hidden");
        let started = Instant::now();
        let n = 2000;
        for _ in 0..n {
            dispatch_gui_layer_pre(&mut host, &mods, "test:layer", &mut batch, &mut canceled);
        }
        let per_call = started.elapsed() / n;
        println!("gui_layer_pre without a handler: {per_call:?} per call");
        assert!(!canceled, "an unheard hook hides nothing");
        assert!(
            per_call < Duration::from_millis(1),
            "a hook nobody hears costs {per_call:?} a frame"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Dispatches `world_primitives` once over a fresh list (the host draws
    /// nothing: the builder writes the list itself) and returns the list.
    fn primitives_of(host: &RecordingHost, mods: &[Box<dyn Hooks>]) -> WorldPrimitives {
        let mut primitives = WorldPrimitives::default();
        dispatch_world_primitives(host, mods, 0.25, [10.0, 64.5, -3.0], &mut primitives);
        primitives
    }

    /// Every primitive call reaches the frame's list with its own values:
    /// positions as `{x, y, z}`, both colour forms, sizes, the text and sprite
    /// names, and the options (`through_walls`, `scale_with_distance`,
    /// `background`) on the primitive they name. The handler also sees the
    /// partial tick and the camera it was given.
    #[test]
    fn each_primitive_call_reaches_the_list_with_its_values() {
        let (m, id, dir) = hmod(
            "primdraw",
            "ruby_hud_primdraw",
            r#"
            horizon.on("world_primitives", function(event)
              assert(event.partial == 0.25, "partial")
              assert(event.camera.x == 10.0 and event.camera.y == 64.5 and event.camera.z == -3.0, "camera")
              event.primitives.line({x = 1, y = 2, z = 3}, {x = 4, y = 5, z = 6}, 4278190080, 2)
              event.primitives.box_outline({x = 0, y = 0, z = 0}, {x = 1, y = 1, z = 1}, {r = 255, g = 0, b = 0}, 1.5)
              event.primitives.filled_box({x = 2, y = 2, z = 2}, {x = 3, y = 3, z = 3}, {r = 0, g = 255, b = 0, a = 128})
              event.primitives.beam({x = 10, y = 64, z = -20}, 256, 0x80FF2010)
              event.primitives.text({x = 0.5, y = 70, z = 0.5}, "Home", 4294967295, 1.0,
                {through_walls = true, scale_with_distance = true, background = true})
              event.primitives.sprite({x = 5, y = 6, z = 7}, "mymod:icon", 2, 255, {through_walls = true})
            end)
            "#,
        );
        let mods = boxed(m);
        let host = RecordingHost::default();
        let primitives = primitives_of(&host, &mods);
        no_error(&id);
        let shapes: Vec<PrimitiveShape> = primitives.iter().map(|p| p.shape.clone()).collect();
        assert_eq!(
            shapes,
            vec![
                PrimitiveShape::Line {
                    from: [1.0, 2.0, 3.0],
                    to: [4.0, 5.0, 6.0],
                    color: 0xFF00_0000,
                    width: 2.0,
                },
                PrimitiveShape::BoxOutline {
                    min: [0.0, 0.0, 0.0],
                    max: [1.0, 1.0, 1.0],
                    color: 0xFFFF_0000,
                    width: 1.5,
                },
                PrimitiveShape::FilledBox {
                    min: [2.0, 2.0, 2.0],
                    max: [3.0, 3.0, 3.0],
                    color: 0x8000_FF00,
                },
                PrimitiveShape::Beam {
                    base: [10.0, 64.0, -20.0],
                    height: 256,
                    color: 0x80FF_2010,
                },
                PrimitiveShape::Text {
                    at: [0.5, 70.0, 0.5],
                    text: "Home".to_owned(),
                    color: 0xFFFF_FFFF,
                    scale: 1.0,
                    scale_with_distance: true,
                    background: true,
                },
                PrimitiveShape::Sprite {
                    at: [5.0, 6.0, 7.0],
                    sprite: "mymod:icon".to_owned(),
                    size: 2.0,
                    color: 255,
                },
            ]
        );
        let through: Vec<bool> = primitives.iter().map(|p| p.through_walls).collect();
        assert_eq!(through, [false, false, false, false, true, true]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A bad argument is an error naming the call and the field, and the
    /// handler carries on: each `pcall` below records its message, and every
    /// one names `primitives.<call>: <field>`. Nothing is drawn for a bad call.
    #[test]
    fn a_bad_primitive_argument_is_an_error_naming_the_call_and_field() {
        let (m, id, dir) = hmod(
            "primbad",
            "ruby_hud_primbad",
            r#"
            horizon.on("world_primitives", function(event)
              local p = event.primitives
              local origin = {x = 0, y = 0, z = 0}
              local one = {x = 1, y = 1, z = 1}
              local function try(f)
                local ok, e = pcall(f)
                if not ok then __errs = (__errs or "") .. tostring(e) .. "\n" end
              end
              try(function() p.line(origin, one, 255, 0 / 0) end)
              try(function() p.line(origin, one, 255, -1) end)
              try(function() p.line(origin, nil, 255, 1) end)
              try(function() p.line({x = 0 / 0, y = 0, z = 0}, one, 255, 1) end)
              try(function() p.line(origin, one, {r = 256, g = 0, b = 0}, 1) end)
              try(function() p.line(origin, one, nil, 1) end)
              try(function() p.line(origin, one, 255, 1, {background = true}) end)
              try(function() p.line(origin, one, 255, 1, {through_walls = "yes"}) end)
              try(function() p.beam(origin, 4096, 255) end)
              try(function() p.text(origin, "", 255, 1) end)
              try(function() p.sprite(origin, "", 1, 255) end)
            end)
            "#,
        );
        let state = m.lua.clone();
        let mods = boxed(m);
        let host = RecordingHost::default();
        let primitives = primitives_of(&host, &mods);
        no_error(&id);
        assert!(primitives.is_empty(), "a bad call draws nothing");
        let errs: String = state.globals().get("__errs").unwrap();
        for needle in [
            "primitives.line: width",
            "primitives.line: to: missing",
            "primitives.line: from",
            "primitives.line: color.r",
            "primitives.line: color: missing",
            "primitives.line: background: unknown option (this call takes through_walls)",
            "primitives.line: through_walls: must be a boolean",
            "primitives.beam: height: must be an integer in 0..=2048",
            "primitives.text: text: must not be empty",
            "primitives.sprite: sprite: must not be empty",
        ] {
            assert!(errs.contains(needle), "missing {needle:?} in {errs}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A negative size too small for an `f32` is an error like any other
    /// negative size: it does not round to `-0.0` and pass. Each sized call
    /// names its field and draws nothing. A `-0.0` width is zero, not negative,
    /// and draws as `+0.0`.
    #[test]
    fn a_negative_size_below_f32_precision_is_an_error_naming_the_field() {
        let (m, id, dir) = hmod(
            "primtiny",
            "ruby_hud_primtiny",
            r#"
            horizon.on("world_primitives", function(event)
              local p = event.primitives
              local origin = {x = 0, y = 0, z = 0}
              local one = {x = 1, y = 1, z = 1}
              local function try(f)
                local ok, e = pcall(f)
                if not ok then __errs = (__errs or "") .. tostring(e) .. "\n" end
              end
              try(function() p.line(origin, one, 255, -1e-50) end)
              try(function() p.text(origin, "Home", 255, -1e-50) end)
              try(function() p.sprite(origin, "mymod:icon", -1e-50, 255) end)
              p.line(origin, one, 255, -0.0)
            end)
            "#,
        );
        let state = m.lua.clone();
        let mods = boxed(m);
        let host = RecordingHost::default();
        let primitives = primitives_of(&host, &mods);
        no_error(&id);
        let errs: String = state.globals().get("__errs").unwrap();
        for needle in [
            "primitives.line: width: must be a finite number that is not negative, got -1e-50",
            "primitives.text: scale: must be a finite number that is not negative, got -1e-50",
            "primitives.sprite: size: must be a finite number that is not negative, got -1e-50",
        ] {
            assert!(errs.contains(needle), "missing {needle:?} in {errs}");
        }
        let shapes: Vec<PrimitiveShape> = primitives.iter().map(|p| p.shape.clone()).collect();
        assert_eq!(shapes.len(), 1, "only the -0.0 line is drawn");
        let PrimitiveShape::Line { width, .. } = shapes[0] else {
            panic!("expected a line, got {:?}", shapes[0]);
        };
        assert!(width == 0.0 && width.is_sign_positive(), "width {width:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The options argument is nil or a table. A boolean or a number there is
    /// an error naming the call and `options`, not mlua's conversion message.
    /// nil and `{}` still mean no options and draw.
    #[test]
    fn primitive_options_must_be_a_table_or_nil() {
        let (m, id, dir) = hmod(
            "primopts",
            "ruby_hud_primopts",
            r#"
            horizon.on("world_primitives", function(event)
              local p = event.primitives
              local origin = {x = 0, y = 0, z = 0}
              local one = {x = 1, y = 1, z = 1}
              local function try(f)
                local ok, e = pcall(f)
                if not ok then __errs = (__errs or "") .. tostring(e) .. "\n" end
              end
              try(function() p.line(origin, one, 255, 1, true) end)
              try(function() p.filled_box(origin, one, 255, 1) end)
              try(function() p.text(origin, "Home", 255, 1, 5) end)
              p.line(origin, one, 255, 1, nil)
              p.box_outline(origin, one, 255, 1, {})
              p.sprite(origin, "mymod:icon", 1, 255, {})
            end)
            "#,
        );
        let state = m.lua.clone();
        let mods = boxed(m);
        let host = RecordingHost::default();
        let primitives = primitives_of(&host, &mods);
        no_error(&id);
        let errs: String = state.globals().get("__errs").unwrap();
        for needle in [
            "primitives.line: options: must be a table or nil",
            "primitives.filled_box: options: must be a table or nil",
            "primitives.text: options: must be a table or nil",
        ] {
            assert!(errs.contains(needle), "missing {needle:?} in {errs}");
        }
        assert!(!errs.contains("bad argument"), "{errs}");
        let shapes: Vec<PrimitiveShape> = primitives.iter().map(|p| p.shape.clone()).collect();
        assert_eq!(shapes.len(), 3, "only the nil and {{}} calls draw");
        assert!(matches!(shapes[0], PrimitiveShape::Line { .. }));
        assert!(matches!(shapes[1], PrimitiveShape::BoxOutline { .. }));
        assert!(matches!(shapes[2], PrimitiveShape::Sprite { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The primitive cap: one call past `MAX_PRIMITIVES_PER_CALL` is an error
    /// naming the cap, and the primitives before it stay drawn. Driven through
    /// the builder directly, so the frame budget does not stop the loop first.
    #[test]
    fn the_primitive_cap_names_the_cap_and_keeps_what_came_before() {
        let lua = mlua::Lua::new();
        let mut list = WorldPrimitives::default();
        let cell = RefCell::new(&mut list);
        let drawn = Cell::new(0_usize);
        let origin = lua.create_table().unwrap();
        origin.set("x", 0.0).unwrap();
        origin.set("y", 0.0).unwrap();
        origin.set("z", 0.0).unwrap();
        let far = lua.create_table().unwrap();
        far.set("x", 1.0).unwrap();
        far.set("y", 1.0).unwrap();
        far.set("z", 1.0).unwrap();
        lua.scope(|scope| {
            let table = primitives_table(scope, &lua, &cell, &drawn)?;
            let line: mlua::Function = table.get("line")?;
            for _ in 0..MAX_PRIMITIVES_PER_CALL {
                line.call::<()>((origin.clone(), far.clone(), 255_u32, 1.0_f32, mlua::Value::Nil))?;
            }
            let over = line
                .call::<()>((origin.clone(), far.clone(), 255_u32, 1.0_f32, mlua::Value::Nil))
                .expect_err("the call past the cap fails");
            let text = over.to_string();
            assert!(
                text.contains(&format!("primitives.line: a handler may draw at most {MAX_PRIMITIVES_PER_CALL} primitives per frame")),
                "{text}"
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(list.len(), MAX_PRIMITIVES_PER_CALL);
        assert_eq!(drawn.get(), MAX_PRIMITIVES_PER_CALL);
    }

    /// A handler that keeps `event.primitives` and draws with it after its call
    /// gets a Lua error: the scope expired, and the builder raises
    /// `CallbackDestructed` instead of drawing into a list it no longer holds.
    #[test]
    fn a_kept_primitives_table_is_an_error_after_its_call() {
        let (m, id, dir) = hmod(
            "primkept",
            "ruby_hud_primkept",
            r#"
            horizon.on("world_primitives", function(event)
              if SAVED == nil then SAVED = event.primitives
              else SAVED.line({x = 0, y = 0, z = 0}, {x = 1, y = 1, z = 1}, 255, 1) end
            end)
            "#,
        );
        let mods = boxed(m);
        let host = RecordingHost::default();
        assert!(
            primitives_of(&host, &mods).is_empty(),
            "keeping the table draws nothing"
        );
        no_error(&id);
        let primitives = primitives_of(&host, &mods);
        let e = last_error(&id).expect("the kept table failed");
        assert!(e.contains("destructed"), "{e}");
        assert!(primitives.is_empty(), "the expired table drew nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two mods share the frame's list in load order: the first mod's
    /// primitives come first, then the second's, each from its own handler.
    #[test]
    fn two_world_primitives_handlers_draw_in_load_order() {
        let (first, first_id, first_dir) = hmod(
            "primfirst",
            "ruby_hud_primfirst",
            r#"horizon.on("world_primitives", function(event)
              event.primitives.box_outline({x = 1, y = 0, z = 0}, {x = 2, y = 1, z = 1}, 255, 1)
            end)"#,
        );
        let (second, second_id, second_dir) = hmod(
            "primsecond",
            "ruby_hud_primsecond",
            r#"horizon.on("world_primitives", function(event)
              event.primitives.filled_box({x = 3, y = 0, z = 0}, {x = 4, y = 1, z = 1}, 255)
            end)"#,
        );
        let mods = vec![
            Box::new(first) as Box<dyn Hooks>,
            Box::new(second) as Box<dyn Hooks>,
        ];
        let host = RecordingHost::default();
        let primitives = primitives_of(&host, &mods);
        no_error(&first_id);
        no_error(&second_id);
        let shapes: Vec<PrimitiveShape> = primitives.iter().map(|p| p.shape.clone()).collect();
        assert_eq!(
            shapes,
            vec![
                PrimitiveShape::BoxOutline {
                    min: [1.0, 0.0, 0.0],
                    max: [2.0, 1.0, 1.0],
                    color: 255,
                    width: 1.0,
                },
                PrimitiveShape::FilledBox {
                    min: [3.0, 0.0, 0.0],
                    max: [4.0, 1.0, 1.0],
                    color: 255,
                },
            ]
        );
        std::fs::remove_dir_all(&first_dir).unwrap();
        std::fs::remove_dir_all(&second_dir).unwrap();
    }

    /// A queued action cannot run from this hook (its receiver is `&self`):
    /// the handler's `tell` is reported as an error and nothing is sent.
    #[test]
    fn a_queued_action_in_world_primitives_is_reported_not_run() {
        let (m, id, dir) = hmod(
            "primtell",
            "ruby_hud_primtell",
            r#"horizon.on("world_primitives", function(event)
              horizon.tell("draw_steve", "from the world pass")
            end)"#,
        );
        let mods = boxed(m);
        let host = RecordingHost {
            players: vec![draw_player()],
            ..Default::default()
        };
        let primitives = primitives_of(&host, &mods);
        assert!(primitives.is_empty());
        let e = last_error(&id).expect("the queued action was reported");
        assert!(e.contains("horizon.tell is not available here"), "{e}");
        assert!(host.tells.lock().unwrap().is_empty(), "nothing was sent");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An infinite `world_primitives` handler is stopped within the frame
    /// budget, like the other frame hooks: the 250 ms handler budget never
    /// applies to a hook that fires every frame.
    #[test]
    fn an_infinite_world_primitives_handler_is_stopped_within_the_frame_budget() {
        let (m, id, dir) = hmod(
            "primloop",
            "ruby_hud_primloop",
            r#"horizon.on("world_primitives", function() while true do end end)"#,
        );
        let mods = boxed(m);
        let host = RecordingHost::default();
        let started = Instant::now();
        let primitives = primitives_of(&host, &mods);
        let elapsed = started.elapsed();
        println!("world_primitives infinite loop stopped after {elapsed:?}");
        assert!(
            elapsed < Duration::from_millis(100),
            "stopped after {elapsed:?}"
        );
        assert!(primitives.is_empty(), "a stopped handler draws nothing");
        let e = last_error(&id).expect("the loop failed");
        assert!(e.contains("ran over its time budget"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `world_primitives` dispatch nobody hears costs no Lua work: no scope,
    /// no tables, like the unheard `gui_layer_pre` hook.
    #[test]
    fn an_unregistered_world_primitives_hook_costs_no_lua_work() {
        let (m, _id, dir) = hmod(
            "primidle",
            "ruby_hud_primidle",
            "horizon.on(\"gui_layer_post\", function() end)",
        );
        let mods = boxed(m);
        let host = RecordingHost::default();
        let mut primitives = WorldPrimitives::default();
        for _ in 0..10 {
            dispatch_world_primitives(&host, &mods, 0.0, [0.0; 3], &mut primitives);
        }
        let started = Instant::now();
        let n = 2000;
        for _ in 0..n {
            dispatch_world_primitives(&host, &mods, 0.0, [0.0; 3], &mut primitives);
        }
        let per_call = started.elapsed() / n;
        println!("world_primitives without a handler: {per_call:?} per call");
        assert!(primitives.is_empty(), "an unheard hook draws nothing");
        assert!(
            per_call < Duration::from_millis(1),
            "a hook nobody hears costs {per_call:?} a frame"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A server state does not get the hook: a `world_primitives` handler in
    /// `scripts/server/` is refused at load, like every client hook.
    #[test]
    fn a_server_state_refuses_world_primitives_at_load() {
        let dir =
            std::env::temp_dir().join(format!("horizon-lua-hud-primserver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ruby.hmod");
        let manifest = MANIFEST_TEXT.replacen("ruby_hud", "ruby_hud_primserver", 1);
        let lua = "horizon.on(\"world_primitives\", function() end)";
        std::fs::write(
            &path,
            Package::write_files(
                &manifest,
                None,
                &[("scripts/server/init.lua", lua.as_bytes())],
            )
            .unwrap(),
        )
        .unwrap();
        let e = LuaMod::load(&path, LuaSide::Server, &[])
            .expect_err("a client hook in the server state must fail to load");
        assert!(
            e.contains("hook 'world_primitives' runs on the client"),
            "{e}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_kept_past_its_call_is_an_error() {
        let (m, id, dir) = hmod(
            "kept",
            "ruby_hud_kept",
            r#"
            horizon.on("hud", function(event)
              if SAVED == nil then SAVED = event.batch
              else SAVED.fill(0, 0, 1, 1, 4278190080) end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        let e = last_error(&id).expect("the kept batch failed");
        assert!(e.contains("destructed"), "{e}");
        assert!(
            host.draws.lock().unwrap().is_empty(),
            "the expired batch drew nothing"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_used_from_another_hook_is_an_error() {
        let (m, id, dir) = hmod(
            "cross",
            "ruby_hud_cross",
            r#"
            horizon.on("hud", function(event) SAVED = event.batch end)
            horizon.on("gui_layer_post", function() SAVED.fill(0, 0, 1, 1, 4278190080) end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        dispatch_gui_layer_post(&mut host, &mods, "test:layer", &mut batch);
        let e = last_error(&id).expect("the kept batch failed");
        assert!(e.contains("destructed"), "{e}");
        assert!(
            host.draws.lock().unwrap().is_empty(),
            "the expired batch drew nothing"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_erroring_hud_handler_runs_again_next_frame() {
        let (m, id, dir) = hmod(
            "boom",
            "ruby_hud_boom",
            r#"
            horizon.on("hud", function(event)
              event.batch.fill(0, 0, 1, 1, 4278190080)
              error("boom")
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        for _ in 0..3 {
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        }
        // Every frame drew (the error did not disable the hook), and every
        // frame reported.
        assert_eq!(host.draws.lock().unwrap().len(), 3);
        let e = last_error(&id).expect("the handler failed");
        assert!(e.contains("boom"), "{e}");
        assert!(!e.contains("disabled"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_infinite_hud_handler_is_stopped_within_the_frame_budget() {
        let (m, id, dir) = hmod(
            "loop",
            "ruby_hud_loop",
            r#"horizon.on("hud", function() while true do end end)"#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        let started = Instant::now();
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        let elapsed = started.elapsed();
        println!("hud infinite loop stopped after {elapsed:?}");
        // The frame budget stopped it, not the 250 ms tick budget.
        assert!(
            elapsed < Duration::from_millis(100),
            "stopped after {elapsed:?}"
        );
        let e = last_error(&id).expect("the loop failed");
        assert!(e.contains("ran over its time budget"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn three_overruns_disable_the_hook_while_others_keep_working() {
        let (looper, looper_id, looper_dir) = hmod(
            "looper",
            "ruby_hud_looper",
            r#"horizon.on("hud", function() while true do end end)"#,
        );
        let (counter, _counter_id, counter_dir) = hmod(
            "counter",
            "ruby_hud_counter",
            r#"horizon.on("hud", function() horizon.broadcast("tick") end)"#,
        );
        let mods = vec![
            Box::new(looper) as Box<dyn Hooks>,
            Box::new(counter) as Box<dyn Hooks>,
        ];
        let mut host = RecordingHost::default();
        for n in 1..=4 {
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
            let e = last_error(&looper_id).expect("the loop failed");
            if n < 3 {
                // Stopped, but still registered.
                assert!(e.contains("ran over its time budget"), "{e}");
                assert!(!e.contains("disabled"), "{e}");
            } else {
                // The third consecutive miss disables the hook.
                assert!(e.contains("disabled"), "{e}");
            }
        }
        // The well-behaved handler worked every frame, the looper's last
        // frame drew and queued nothing.
        assert_eq!(*host.broadcasts.lock().unwrap(), 4);
        std::fs::remove_dir_all(&looper_dir).unwrap();
        std::fs::remove_dir_all(&counter_dir).unwrap();
    }

    #[test]
    fn reload_enables_a_disabled_hook_again() {
        let (m, id, dir) = hmod(
            "relive",
            "ruby_hud_relive",
            r#"horizon.on("hud", function() while true do end end)"#,
        );
        let cell = Arc::new(Mutex::new(m));
        let mods = vec![Box::new(Shared(cell.clone())) as Box<dyn Hooks>];
        let dispatch = |host: &mut RecordingHost| {
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(host, &mods, &mut batch, &mut corners);
        };
        let mut host = RecordingHost::default();
        for _ in 0..3 {
            dispatch(&mut host);
        }
        assert!(
            last_error(&id)
                .expect("the loop failed")
                .contains("disabled"),
            "{:?}",
            last_error(&id)
        );
        // `/reload` registers the handler again, with fresh chances: the
        // next miss stops it without disabling it.
        cell.lock().unwrap().reload_now("/reload");
        no_error(&id);
        dispatch(&mut host);
        let e = last_error(&id).expect("the loop failed");
        assert!(e.contains("ran over its time budget"), "{e}");
        assert!(!e.contains("disabled"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unheard_frame_clears_a_stale_overrun_count() {
        let (m, id, dir) = hmod(
            "stale",
            "ruby_hud_stale",
            r#"horizon.on("hud", function() while true do end end)"#,
        );
        let cell = Arc::new(Mutex::new(m));
        let mods = vec![Box::new(Shared(cell.clone())) as Box<dyn Hooks>];
        let dispatch = |host: &mut RecordingHost| {
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(host, &mods, &mut batch, &mut corners);
        };
        let mut host = RecordingHost::default();
        // Three consecutive misses disable the hook.
        for _ in 0..3 {
            dispatch(&mut host);
        }
        assert!(
            last_error(&id)
                .expect("the loop failed")
                .contains("disabled"),
            "{:?}",
            last_error(&id)
        );
        // One more dispatch while the handlers are already nil: the
        // early-out treats the unheard frame as a clean outcome and
        // clears the stale count of 3. Nothing new is reported.
        dispatch(&mut host);
        assert!(
            last_error(&id)
                .expect("the loop failed")
                .contains("disabled"),
            "{:?}",
            last_error(&id)
        );
        // Re-register without `/reload`: the next miss stops the handler
        // without disabling it, which only holds when the unheard frame
        // cleared the count above.
        cell.lock()
            .unwrap()
            .lua
            .load(r#"horizon.on("hud", function() while true do end end)"#)
            .exec()
            .unwrap();
        dispatch(&mut host);
        let e = last_error(&id).expect("the loop failed");
        assert!(e.contains("ran over its time budget"), "{e}");
        assert!(!e.contains("disabled"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_kind_filter_on_a_frame_hook_fails_the_load() {
        let dir = std::env::temp_dir().join(format!("horizon-lua-hud-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ruby.hmod");
        let manifest = MANIFEST_TEXT.replacen("ruby_hud", "ruby_hud_kind", 1);
        std::fs::write(
            &path,
            Package::write_files(
                &manifest,
                None,
                &[(
                    "scripts/client/init.lua",
                    b"horizon.on(\"hud\", function() end, \"minecraft:cow\")" as &[u8],
                )],
            )
            .unwrap(),
        )
        .unwrap();
        let e = LuaMod::load(&path, LuaSide::Client, &[]).expect_err("a kind must be refused");
        assert!(e.contains("takes no kind"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unregistered_hud_hook_costs_no_lua_work() {
        let (m, _id, dir) = hmod(
            "idle",
            "ruby_hud_idle",
            "horizon.on(\"gui_layer_post\", function() end)",
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        for _ in 0..10 {
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        }
        let started = Instant::now();
        let n = 2000;
        for _ in 0..n {
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        }
        let per_call = started.elapsed() / n;
        println!("hud without a handler: {per_call:?} per call");
        assert!(
            host.draws.lock().unwrap().is_empty(),
            "nothing drew without a handler"
        );
        assert!(
            per_call < Duration::from_millis(1),
            "a hook nobody hears costs {per_call:?} a frame"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fifty_fills_take_well_under_a_frame() {
        let (m, id, dir) = hmod(
            "fills",
            "ruby_hud_fills",
            r#"
            horizon.on("hud", function(event)
              for i = 1, 50 do event.batch.fill(i, i, 10, 10, 4278190080) end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        // Take the minimum over 5 repetitions: a single timing is inflated
        // by scheduler noise under parallel build load; the minimum is the
        // quiet-machine cost.
        let n: usize = 200;
        let mut minimum = Duration::MAX;
        for _ in 0..5 {
            let started = Instant::now();
            for _ in 0..n {
                dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
            }
            minimum = minimum.min(started.elapsed() / n as u32);
        }
        println!("hud with a handler drawing 50 fills: {minimum:?} per call (minimum of 5)");
        assert_eq!(host.draws.lock().unwrap().len(), 50 * (n * 5 + 1));
        assert!(
            minimum < Duration::from_millis(50),
            "50 fills cost {minimum:?} a frame"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hud_handler_reads_the_player_and_the_time() {
        let (m, id, dir) = hmod(
            "reads",
            "ruby_hud_reads",
            r#"
            horizon.on("hud", function(event)
              local p = horizon.player("draw_steve")
              local t = horizon.time()
              event.batch.text(p.name .. "@" .. t.day .. "/" .. t.time_of_day .. "/" .. t.rain, 10, 20, 16777215, true)
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost {
            players: vec![draw_player()],
            ..Default::default()
        };
        // Off midnight, noon and the default 0: day 1, 6001 ticks in.
        host.day_time = 30001;
        host.weather = (0.25, 0.0);
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["text \"draw_steve@1/6001/0.25\" at [10.0, 20.0] 0xffffff true"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_gui_layer_post_handler_reads_mobs() {
        let (m, id, dir) = hmod(
            "mobreads",
            "ruby_hud_mobreads",
            r##"
            horizon.on("gui_layer_post", function(event)
              local one = horizon.mob(7)
              local all = horizon.mobs()
              local n = #all
              event.batch.text(one.kind .. "#" .. one.id .. "x" .. n, 1, 2, 16777215, false)
            end)
            "##,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        dispatch_gui_layer_post(&mut host, &mods, "test:layer", &mut batch);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["text \"minecraft:cow#7x1\" at [1.0, 2.0] 0xffffff false"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_hud_handler_reads_a_non_acting_players_stacks() {
        // Only the acting player's stacks need `&mut` and fail in a draw
        // hook: anyone else's come from the shared host (`player_stacks`
        // takes `&self`). With players [draw_steve, alex] the first acts,
        // so `alex` reads shared. Before the fix this arm panicked on
        // `unreachable!()`.
        let (m, id, dir) = hmod(
            "sharedstacks",
            "ruby_hud_sharedstacks",
            r#"
            horizon.on("hud", function(event)
              local inv = horizon.inventory("alex")
              local n = horizon.count_items("alex", "minecraft:stick")
              local carried = horizon.carried_items("alex")
              assert(inv[1].id == "minecraft:stick" and inv[1].count == 3, "inventory slot 1")
              assert(inv[2].id == "minecraft:bone" and inv[2].count == 5, "inventory slot 2")
              assert(n == 3, "count_items")
              assert(carried["minecraft:stick"] == 3, "carried sticks")
              assert(carried["minecraft:bone"] == 5, "carried bones")
              event.batch.fill(0, 0, 1, 1, 4278190080)
            end)
            "#,
        );
        let mods = boxed(m);
        let alex = other_player();
        let mut stacks = HashMap::new();
        stacks.insert(
            alex.uuid,
            vec![
                Some(ItemStack {
                    item: "minecraft:stick".into(),
                    count: 3,
                    damage: 0,
                    extra: None,
                }),
                Some(ItemStack {
                    item: "minecraft:bone".into(),
                    count: 5,
                    damage: 0,
                    extra: None,
                }),
            ],
        );
        let mut host = RecordingHost {
            players: vec![draw_player(), alex],
            stacks,
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [0.0, 0.0, 1.0, 1.0] 0xff000000"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_draw_hook_refuses_server_only_actions_and_declarations() {
        // In a client draw hook only the reads and `tell`/`broadcast` may
        // run: every other server-only call raises the server-only error,
        // and queues nothing (`run_action` on a queued `set_block` would
        // panic here: `RecordingHost` implements no `state_named`).
        let (m, id, dir) = hmod(
            "drawapi",
            "ruby_hud_drawapi",
            r#"
            horizon.on("hud", function(event)
              local t = horizon.time()
              assert(t.day == 1, "time read")
              horizon.tell("draw_steve", "hello from hud")
              horizon.broadcast("hi")
              local function expect_server_only(name, f)
                local ok, err = pcall(f)
                assert(not ok, name .. " should fail in a draw hook")
                assert(
                  string.find(tostring(err), "is only available in scripts/server/", 1, true),
                  name .. ": " .. tostring(err)
                )
              end
              expect_server_only("set_block", function() horizon.set_block(0, 64, 0, "minecraft:stone") end)
              expect_server_only("kill_mob", function() horizon.kill_mob("minecraft:overworld", 7) end)
              expect_server_only("run_command", function() horizon.run_command("say hi") end)
              expect_server_only("command", function() horizon.command("ruby hi", 0, function() end) end)
              expect_server_only("goal", function() horizon.goal("ruby_g", {}) end)
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost {
            players: vec![draw_player()],
            ..Default::default()
        };
        host.day_time = 30001;
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        // The read answered and both queued chat actions ran after the
        // call; nothing else executed.
        assert_eq!(host.tells.lock().unwrap().len(), 1);
        assert_eq!(*host.broadcasts.lock().unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_entity_data_is_refused_in_a_draw_hook() {
        // `horizon.entity_data` writes back through `&mut`, which a draw
        // hook cannot lend: like `world_data` it names the helper and
        // fails, instead of queueing a write for later.
        let (m, id, dir) = hmod(
            "drawentitydata",
            "ruby_hud_drawentitydata",
            r#"
            horizon.on("hud", function(event)
              __witness = (__witness or 0) + 1
              local ok, err = pcall(horizon.entity_data, 7)
              REFUSED = (not ok) and tostring(err) or "ran"
            end)
            "#,
        );
        let state = m.lua.clone();
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(state.globals().get::<i64>("__witness").unwrap(), 1);
        let refused: String = state.globals().get("REFUSED").unwrap();
        assert!(
            refused.contains("not available in draw hooks"),
            "refuses like world_data: {refused}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn on_a_client_window_player_and_mob_reads_are_empty() {
        // Characterization of today's client behaviour (see
        // `ClientWindowHost`): player and mob reads need a server and come
        // back empty on the window that draws the hud, while `time`,
        // `block`, `block_property` and `light` read the client's world.
        // The native side may change this (#47 follow-up).
        let (m, id, dir) = hmod(
            "clientreads",
            "ruby_hud_clientreads",
            r#"
            horizon.on("hud", function(event)
              local t = horizon.time()
              assert(t.dimension == "minecraft:overworld", "dimension")
              assert(t.day == 1 and t.time_of_day == 6001, "day and time")
              assert(t.rain == 0.25 and t.thunder == 0.0, "weather")
              assert(horizon.block(0, 64, 0) == "minecraft:stone", "block")
              assert(horizon.block_property(0, 64, 0, "snowy") == "true", "block_property")
              local l = horizon.light(0, 64, 0)
              assert(l.block == 12 and l.sky == 15, "light")
              assert(horizon.player("draw_steve") == nil, "player")
              assert(#horizon.players() == 0, "players")
              assert(horizon.mob(7) == nil, "mob")
              assert(#horizon.mobs() == 0, "mobs")
              assert(horizon.raycast("draw_steve") == nil, "raycast")
              assert(horizon.surface_height(0, 0) == nil, "surface_height")
              event.batch.fill(0, 0, 1, 1, 4278190080)
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = ClientWindowHost {
            day_time: 30001,
            weather: (0.25, 0.0),
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [0.0, 0.0, 1.0, 1.0] 0xff000000"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn on_a_client_window_the_acting_player_is_nil() {
        // A client window has no acting player (`ClientWindowHost` answers
        // none), no listed players and no stacks: `acting_player` and the
        // `player(s)` tables read nil, like the other server reads there.
        let (m, id, dir) = hmod(
            "clientacting",
            "ruby_hud_clientacting",
            r#"
            horizon.on("hud", function(event)
              __witness = (__witness or 0) + 1
              GOT = horizon.acting_player()
              assert(horizon.player("draw_steve") == nil, "player")
            end)
            "#,
        );
        let state = m.lua.clone();
        let mods = boxed(m);
        let mut host = ClientWindowHost {
            day_time: 30001,
            weather: (0.25, 0.0),
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(state.globals().get::<i64>("__witness").unwrap(), 1);
        assert!(state.globals().get::<mlua::Value>("GOT").unwrap().is_nil());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_draw_hook_reads_held_through_the_shared_host() {
        // `held` comes from `held_stack`, which takes `&self`: it works in
        // a draw hook, for the acting player and the others alike. The
        // first listed player acts here.
        let (m, id, dir) = hmod(
            "drawheld",
            "ruby_hud_drawheld",
            r#"
            horizon.on("hud", function(event)
              __witness = (__witness or 0) + 1
              local all = horizon.players()
              assert(all[1].held.id == "ruby:wand" and all[1].held.count == 3, "draw held")
              assert(all[1].held.damage == 0 and all[1].held.plain, "draw held shape")
              local a = horizon.acting_player()
              assert(a.name == "draw_steve" and a.held.id == "ruby:wand", "draw acting")
            end)
            "#,
        );
        let state = m.lua.clone();
        let mods = boxed(m);
        let steve = draw_player();
        let mut held = HashMap::new();
        held.insert(
            steve.uuid,
            ItemStack {
                item: "ruby:wand".into(),
                count: 3,
                damage: 0,
                extra: None,
            },
        );
        let mut host = RecordingHost {
            players: vec![steve],
            held,
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(state.globals().get::<i64>("__witness").unwrap(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn on_a_client_window_gui_layer_post_reads_are_empty() {
        // The same characterization through `gui_layer_post`: the client
        // world answers, player and mob reads stay empty. The native side
        // may change this (#47 follow-up).
        let (m, id, dir) = hmod(
            "clientlayerreads",
            "ruby_hud_clientlayerreads",
            r#"
            horizon.on("gui_layer_post", function(event)
              local t = horizon.time()
              assert(t.dimension == "minecraft:overworld", "dimension")
              assert(t.day == 1 and t.time_of_day == 6001, "day and time")
              assert(horizon.block(0, 64, 0) == "minecraft:stone", "block")
              local l = horizon.light(0, 64, 0)
              assert(l.block == 12 and l.sky == 15, "light")
              assert(horizon.player("draw_steve") == nil, "player")
              assert(#horizon.players() == 0, "players")
              assert(horizon.mob(7) == nil, "mob")
              assert(#horizon.mobs() == 0, "mobs")
              assert(horizon.raycast("draw_steve") == nil, "raycast")
              assert(horizon.surface_height(0, 0) == nil, "surface_height")
              event.batch.fill(0, 0, 1, 1, 4278190080)
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = ClientWindowHost {
            day_time: 30001,
            weather: (0.25, 0.0),
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        dispatch_gui_layer_post(&mut host, &mods, "test:layer", &mut batch);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [0.0, 0.0, 1.0, 1.0] 0xff000000"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn draw_hooks_refuse_mutating_reads() {
        // Every read that needs `&mut`: the Lua call naming it, and what
        // the handler error must say. The exact draw-hook message (not
        // just the helper name) is what fails the old behaviour, where a
        // hook without a host errors with the helper's own name too.
        for (helper, call) in [
            ("world_data", "horizon.world_data()"),
            ("player_data", "horizon.player_data(\"draw_steve\")"),
            ("inventory", "horizon.inventory(\"draw_steve\")"),
            (
                "count_items",
                "horizon.count_items(\"draw_steve\", \"minecraft:stick\")",
            ),
            ("carried_items", "horizon.carried_items(\"draw_steve\")"),
            (
                "summon",
                "horizon.summon(\"minecraft:overworld\", \"minecraft:cow\", 0.5, 64.0, 0.5)",
            ),
        ] {
            let (m, id, dir) = hmod(
                &format!("refused-{helper}"),
                &format!("ruby_hud_refused_{helper}"),
                &format!(
                    "horizon.on(\"hud\", function(event) event.batch.fill(0, 0, 1, 1, 4278190080) {call} end)"
                ),
            );
            let mods = boxed(m);
            // The first player acts, so the inventory reads take the
            // `&mut` path (the server's records would answer for anyone
            // else).
            let mut host = RecordingHost {
                players: vec![draw_player()],
                ..Default::default()
            };
            for _ in 0..2 {
                let mut batch = HudBatch::new([800, 600]);
                let mut corners = Corners::new(800.0, 600.0, 0.0);
                dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
                let e = last_error(&id).expect("a &mut-only read failed");
                assert!(e.contains(helper), "{e}");
                assert!(
                    e.contains("not available in draw hooks"),
                    "the draw-hook refusal names itself: {e}"
                );
                assert!(!e.contains("disabled"), "{e}");
                assert!(!e.contains("ran over its time budget"), "{e}");
            }
            // The hook ran again next frame: both frames drew before the
            // read failed.
            assert_eq!(host.draws.lock().unwrap().len(), 2);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn a_draw_read_error_resets_the_overrun_count() {
        let (m, id, dir) = hmod(
            "reset",
            "ruby_hud_reset",
            r#"
            horizon.on("hud", function(event)
              N = (N or 0) + 1
              if N == 1 then horizon.world_data() end
              while true do end
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        for n in 1..=4 {
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
            let e = last_error(&id).expect("the handler failed");
            if n == 1 {
                // An ordinary failure: the draw-hook refusal, not an
                // overrun. The exact message (not just the helper name)
                // is what fails the old behaviour, where a hook without
                // a host errors with the helper's own name too.
                assert!(e.contains("world_data"), "{e}");
                assert!(
                    e.contains("not available in draw hooks"),
                    "the draw-hook refusal names itself: {e}"
                );
                assert!(!e.contains("ran over its time budget"), "{e}");
                assert!(!e.contains("disabled"), "{e}");
            } else if n < 4 {
                assert!(e.contains("ran over its time budget"), "{e}");
                assert!(!e.contains("disabled"), "{e}");
            } else {
                // The third consecutive overrun disables: had the read
                // error counted, this would have fired a frame earlier.
                assert!(e.contains("disabled"), "{e}");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_write_queued_from_hud_lands_after_the_call() {
        // The tell's target comes from a read, so the old behaviour (a
        // hook without a host, where the read fails) queues nothing and
        // this test fails there: the write needs the draw hook's reads.
        let (m, id, dir) = hmod(
            "queued",
            "ruby_hud_queued",
            r#"
            horizon.on("hud", function(event)
              event.batch.fill(0, 0, 1, 1, 4278190080)
              local name = horizon.player("draw_steve").name
              horizon.tell(name, "hello from hud")
            end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost {
            players: vec![draw_player()],
            ..Default::default()
        };
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        assert_eq!(
            *host.draws.lock().unwrap(),
            ["fill [0.0, 0.0, 1.0, 1.0] 0xff000000"]
        );
        // The tell queued during the call ran after it, with `&mut`.
        let tells = host.tells.lock().unwrap();
        assert_eq!(tells.len(), 1);
        assert!(tells[0].contains("hello from hud"), "{}", tells[0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A NaN or infinite float argument of a draw helper is a Lua error
    /// that names the argument, and nothing is drawn: the sizes and
    /// positions reach native layout. A finite number too big for f32 (1e39
    /// narrows to infinity) is refused the same way.
    #[test]
    fn a_non_finite_draw_argument_is_a_lua_error_naming_it() {
        let cases = [
            (
                "fill_nan_w",
                "event.batch.fill(0, 0, 0/0, 4, 4278190080)",
                "`w`",
            ),
            (
                "fill_inf_h",
                "event.batch.fill(0, 0, 1, 1/0, 4278190080)",
                "`h`",
            ),
            (
                "text_inf_x",
                r#"event.batch.text("hi", 1/0, 2, 16777215, false)"#,
                "`x`",
            ),
            (
                "text_huge_y",
                r#"event.batch.text("hi", 1, 1e39, 16777215, false)"#,
                "`y`",
            ),
            (
                "sprite_nan_dw",
                r#"event.batch.sprite("gui/x", 0, 0, 1, 1, 0, 0, 0/0, 1)"#,
                "`dw`",
            ),
            (
                "item_inf_size",
                r#"event.batch.item("stone", 0, 0, 1/0)"#,
                "`size`",
            ),
            (
                "item_stack_nan_x",
                r#"event.batch.item_stack({id = "stone", count = 1, damage = 0}, 0/0, 0, 16)"#,
                "`x`",
            ),
            (
                "item_model_nan_y",
                r#"event.batch.item_model("stone", "m", 0, 0/0, 16)"#,
                "`y`",
            ),
            (
                "quad_nan_x4",
                "event.batch.quad(0, 0, 1, 0, 1, 1, 0/0, 1, 4278190080)",
                "`x4`",
            ),
            (
                "place_nan_w",
                r#"event.corners.place("top_left", 0/0, 1)"#,
                "`w`",
            ),
        ];
        for (name, call, arg) in cases {
            let id = format!("ruby_hud_{name}");
            let source = format!(r#"horizon.on("hud", function(event) {call} end)"#);
            let (m, id, dir) = hmod(name, &id, &source);
            let mods = boxed(m);
            let mut host = RecordingHost::default();
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
            let e = last_error(&id).unwrap_or_else(|| panic!("{name}: the handler did not fail"));
            assert!(e.contains(arg), "{name}: the error names {arg}: {e}");
            assert!(
                host.draws.lock().unwrap().is_empty(),
                "{name}: nothing is drawn"
            );
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn batch_item_stack_uses_the_legacy_lenient_parser() {
        // The HUD parser predates the strict `stack_from_table`: any
        // `u32` count, stray keys ignored, a numeric string coerced
        // by mlua — all draw, exactly as on hub/engine-work.
        for (name, script, drawn) in [
            (
                "zero_count",
                r#"horizon.on("hud", function(event) assert(event.batch.item_stack({id = "stick", count = 0, damage = 3}, 6, 6, 16) == true) end)"#,
                "item_stack \"minecraft:stick\" x0 @[6.0, 6.0] size 16",
            ),
            (
                "hundred_count",
                r#"horizon.on("hud", function(event) assert(event.batch.item_stack({id = "stick", count = 100, damage = 3}, 6, 6, 16) == true) end)"#,
                "item_stack \"minecraft:stick\" x100 @[6.0, 6.0] size 16",
            ),
            (
                "stray_key",
                r#"horizon.on("hud", function(event) assert(event.batch.item_stack({id = "stick", count = 1, damage = 3, stray = 42}, 6, 6, 16) == true) end)"#,
                "item_stack \"minecraft:stick\" x1 @[6.0, 6.0] size 16",
            ),
            (
                "string_count",
                r#"horizon.on("hud", function(event) assert(event.batch.item_stack({id = "stick", count = "2", damage = 3}, 6, 6, 16) == true) end)"#,
                "item_stack \"minecraft:stick\" x2 @[6.0, 6.0] size 16",
            ),
        ] {
            let id = format!("ruby_hud_{name}");
            let (m, id, dir) = hmod(&format!("legacy_{name}"), &id, script);
            let mods = boxed(m);
            let mut host = RecordingHost::default();
            let mut batch = HudBatch::new([800, 600]);
            let mut corners = Corners::new(800.0, 600.0, 0.0);
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
            no_error(&id);
            assert_eq!(*host.draws.lock().unwrap(), [drawn]);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
