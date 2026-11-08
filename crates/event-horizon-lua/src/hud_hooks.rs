//! The HUD pilot: `hud` and `gui_layer_post` for scripted mods.
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
//! * Handlers get no `horizon.block` reads (`Reader::None`, like `mob_hurt`):
//!   the hook's `&mut dyn Host` cannot be shared with the draw closures and
//!   the reader at once, and drawing is the point. A read is a Lua error
//!   naming the hook.
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
use crate::lua_mods::{FRAME_BUDGET, LuaMod, Reader, qualify};
use horizon_client::mod_api::{
    Corner, Corners, GuiLayerPostEvent, Host, HudBatch, HudEvent, ItemStack,
};
use mlua::{Error, Lua, Table};
use std::cell::RefCell;

/// A handler stopped by the interrupt, as opposed to one that failed on its
/// own: the budget's message (`Budget::run` in `lua_mods`).
fn is_overrun(e: &mlua::Error) -> bool {
    e.to_string().contains("ran over its time budget")
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
fn fill_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (f32, f32, f32, f32, u32)) -> mlua::Result<()> + 'a {
    move |_, (x, y, w, h, argb)| {
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        host.fill(&mut borrowed, [x, y, w, h], argb);
        Ok(())
    }
}

/// `batch.text(text, x, y, rgb, shadow)`: a line in GUI pixels.
fn text_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, f32, f32, u32, bool)) -> mlua::Result<()> + 'a {
    move |_, (text, x, y, rgb, shadow)| {
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
fn sprite_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, f32, f32, f32, f32, f32, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (name, sx, sy, sw, sh, dx, dy, dw, dh)| {
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.sprite(&mut borrowed, &name, [sx, sy, sw, sh], [dx, dy, dw, dh]))
    }
}

/// `batch.item(id, x, y, size)`: the item `id` (`diamond_sword`...), drawn
/// `size` GUI pixels square at (`x`, `y`). Whether it drew.
fn item_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (id, x, y, size)| {
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.item(&mut borrowed, &qualify(&id), [x, y], size))
    }
}

/// `batch.item_stack(stack, x, y, size)`: the stack (`{id, count, damage}`
/// as the hooks hand them out) at (`x`, `y`). Whether it drew.
fn item_stack_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (Table, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (stack, x, y, size)| {
        let stack = stack_of(&stack)?;
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.item_stack(&mut borrowed, &stack, [x, y], size))
    }
}

/// `batch.item_model(item, model, x, y, size)`: `item` drawn with `model`.
/// Whether it drew.
fn item_model_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (String, String, f32, f32, f32)) -> mlua::Result<bool> + 'a {
    move |_, (item, model, x, y, size)| {
        let mut borrowed = batch
            .try_borrow_mut()
            .map_err(|_| Error::runtime("hud: the batch is already drawn into (reentrant draw)"))?;
        Ok(host.item_model(&mut borrowed, &qualify(&item), &model, [x, y], size))
    }
}

/// `batch.quad(x1, y1, x2, y2, x3, y3, x4, y4, argb)`: a filled quad.
fn quad_fn<'a>(
    host: &'a dyn Host,
    batch: &'a RefCell<&mut HudBatch>,
) -> impl Fn(&Lua, (f32, f32, f32, f32, f32, f32, f32, f32, u32)) -> mlua::Result<()> + 'a {
    move |_, (x1, y1, x2, y2, x3, y3, x4, y4, argb)| {
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
fn text_width_fn<'a>(host: &'a dyn Host) -> impl Fn(&Lua, String) -> mlua::Result<f32> + 'a {
    move |_, text| Ok(host.text_width(&text))
}

/// `corners.place(corner, w, h)`: the top-left position of a `w` x `h` box
/// in `corner`, below (or above) what the corner already holds, as `x, y`.
fn place_fn<'a>(
    corners: &'a RefCell<&mut Corners>,
) -> impl Fn(&Lua, (String, f32, f32)) -> mlua::Result<(f32, f32)> + 'a {
    move |_, (name, w, h)| {
        let mut borrowed = corners.try_borrow_mut().map_err(|_| {
            Error::runtime("hud: the corners are already placed into (reentrant draw)")
        })?;
        Ok(borrowed.place(corner(&name)?, w, h))
    }
}

/// `corners.shown(corner)`: whether the corner is not taken (by a debug
/// overlay...): skip it when one `shown` denies.
fn shown_fn<'a>(host: &'a dyn Host) -> impl Fn(&Lua, String) -> mlua::Result<bool> + 'a {
    move |_, name| Ok(host.corner_shown(corner(&name)?))
}

/// The outcome of a frame hook's handler call: what the handlers queued
/// (nothing when they failed), for the hook to run with its `&mut` host.
type Queued = mlua::Result<Vec<(String, Table)>>;

/// Runs a frame hook's handlers, with the budget and the bookkeeping every
/// frame hook shares: a miss counts toward disabling (until `/reload`), a
/// clean run or a handler's own error resets the count. Handlers without a
/// registered handler cost nothing: no scope, no tables.
fn finish(mod_: &LuaMod, host: &mut dyn Host, hook: &str, outcome: Queued) {
    match outcome {
        Ok(actions) => {
            mod_.note_frame_outcome(hook, false);
            for (action, args) in actions {
                if let Err(e) = mod_.run_action(host, &action, &args) {
                    mod_.error(format!("{hook}: {e}"));
                }
            }
        }
        Err(e) => {
            let overrun = is_overrun(&e);
            mod_.error(format!("{hook}: {e}"));
            mod_.note_frame_outcome(hook, overrun);
        }
    }
}

/// The HUD (not drawn under F1), after the minimap: `hud(event)`, with
/// `event.batch` to draw on and `event.corners` to stack boxes in.
pub(crate) fn hud(mod_: &LuaMod, host: &mut dyn Host, event: &mut HudEvent<'_>) {
    let hook = "hud";
    if !mod_.has_handlers(hook) {
        mod_.note_frame_outcome(hook, false);
        return;
    }
    let lua = &mod_.lua;
    let batch = RefCell::new(&mut *event.batch);
    let corners = RefCell::new(&mut *event.corners);
    let shared: &dyn Host = &*host;
    let outcome = lua.scope(|scope| {
        let table = lua.create_table()?;
        let batch_table = lua.create_table()?;
        batch_table.set("fill", scope.create_function(fill_fn(shared, &batch))?)?;
        batch_table.set("text", scope.create_function(text_fn(shared, &batch))?)?;
        batch_table.set("sprite", scope.create_function(sprite_fn(shared, &batch))?)?;
        batch_table.set("item", scope.create_function(item_fn(shared, &batch))?)?;
        batch_table.set(
            "item_stack",
            scope.create_function(item_stack_fn(shared, &batch))?,
        )?;
        batch_table.set(
            "item_model",
            scope.create_function(item_model_fn(shared, &batch))?,
        )?;
        batch_table.set("quad", scope.create_function(quad_fn(shared, &batch))?)?;
        batch_table.set("text_width", scope.create_function(text_width_fn(shared))?)?;
        table.set("batch", batch_table)?;
        let corners_table = lua.create_table()?;
        corners_table.set("place", scope.create_function(place_fn(&corners))?)?;
        corners_table.set("shown", scope.create_function(shown_fn(shared))?)?;
        table.set("corners", corners_table)?;
        mod_.run_handlers_with(hook, None, Reader::None, table, FRAME_BUDGET)
    });
    finish(mod_, host, hook, outcome);
}

/// After one of vanilla's GUI layers: `gui_layer_post(event)`, with
/// `event.layer` and `event.batch` to draw above it.
pub(crate) fn gui_layer_post(
    mod_: &LuaMod,
    host: &mut dyn Host,
    event: &mut GuiLayerPostEvent<'_>,
) {
    let hook = "gui_layer_post";
    if !mod_.has_handlers(hook) {
        mod_.note_frame_outcome(hook, false);
        return;
    }
    let lua = &mod_.lua;
    let batch = RefCell::new(&mut *event.batch);
    let shared: &dyn Host = &*host;
    let outcome = lua.scope(|scope| {
        let table = lua.create_table()?;
        table.set("layer", event.layer)?;
        let batch_table = lua.create_table()?;
        batch_table.set("fill", scope.create_function(fill_fn(shared, &batch))?)?;
        batch_table.set("text", scope.create_function(text_fn(shared, &batch))?)?;
        batch_table.set("sprite", scope.create_function(sprite_fn(shared, &batch))?)?;
        batch_table.set("item", scope.create_function(item_fn(shared, &batch))?)?;
        batch_table.set(
            "item_stack",
            scope.create_function(item_stack_fn(shared, &batch))?,
        )?;
        batch_table.set(
            "item_model",
            scope.create_function(item_model_fn(shared, &batch))?,
        )?;
        batch_table.set("quad", scope.create_function(quad_fn(shared, &batch))?)?;
        batch_table.set("text_width", scope.create_function(text_width_fn(shared))?)?;
        table.set("batch", batch_table)?;
        mod_.run_handlers_with(hook, None, Reader::None, table, FRAME_BUDGET)
    });
    finish(mod_, host, hook, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua_mods::LuaMod;
    use horizon_client::dev_input::mods::{ModStatus, set_status, status};
    use horizon_client::mod_api::{
        BlockStateId, EntityRef, Hooks, NativeServerPlayer, NativeWorldEpoch, Text,
        dispatch_gui_layer_post, dispatch_hud, dispatch_mob_spawned,
    };
    use horizon_mods::Package;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// A game host that records what the handler drew instead of drawing
    /// it: every draw helper appends a line, `broadcast` counts, `text`
    /// is 6 pixels a character, and only `top_left` is shown.
    #[derive(Default)]
    struct RecordingHost {
        draws: Mutex<Vec<String>>,
        broadcasts: Mutex<u64>,
    }

    impl Host for RecordingHost {
        fn level_players(&self) -> Vec<NativeServerPlayer> {
            Vec::new()
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
            None
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
            Package::write_files(&manifest, None, &[("scripts/init.lua", lua.as_bytes())]).unwrap(),
        )
        .unwrap();
        let m = LuaMod::load(&path, &[]).unwrap();
        set_status(ModStatus {
            id: id.to_owned(),
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
            horizon.on("mob_spawned", function() SAVED.fill(0, 0, 1, 1, 4278190080) end)
            "#,
        );
        let mods = boxed(m);
        let mut host = RecordingHost::default();
        let mut batch = HudBatch::new([800, 600]);
        let mut corners = Corners::new(800.0, 600.0, 0.0);
        dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        no_error(&id);
        dispatch_mob_spawned(
            &mut host,
            &mods,
            "minecraft:overworld",
            "minecraft:zombie",
            [0.0, 64.0, 0.0],
            true,
        );
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
                    "scripts/init.lua",
                    b"horizon.on(\"hud\", function() end, \"minecraft:cow\")" as &[u8],
                )],
            )
            .unwrap(),
        )
        .unwrap();
        let e = LuaMod::load(&path, &[]).expect_err("a kind must be refused");
        assert!(e.contains("takes no kind"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unregistered_hud_hook_costs_no_lua_work() {
        let (m, _id, dir) = hmod(
            "idle",
            "ruby_hud_idle",
            "horizon.on(\"server_tick\", function() end)",
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
        let started = Instant::now();
        let n: usize = 200;
        for _ in 0..n {
            dispatch_hud(&mut host, &mods, &mut batch, &mut corners);
        }
        let per_call = started.elapsed() / n as u32;
        println!("hud with a handler drawing 50 fills: {per_call:?} per call");
        assert_eq!(host.draws.lock().unwrap().len(), 50 * (n + 1));
        assert!(
            per_call < Duration::from_millis(50),
            "50 fills cost {per_call:?} a frame"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
