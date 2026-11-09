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
        mod_.run_handlers_with(hook, None, Reader::Shared(shared), table, FRAME_BUDGET)
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
        mod_.run_handlers_with(hook, None, Reader::Shared(shared), table, FRAME_BUDGET)
    });
    finish(mod_, host, hook, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua_mods::LuaMod;
    use horizon_client::dev_input::mods::{ModStatus, set_status, status};
    use horizon_client::mobs::{Mob, MobType};
    use horizon_client::mod_api::{
        BlockStateId, EntityRef, Hooks, ItemStack, MobView, NativeHeightmap, NativeHit,
        NativeServerPlayer, NativeWorldEpoch, Text, dispatch_gui_layer_post, dispatch_hud,
        dispatch_mob_spawned,
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
    /// draw hook may serve come from `players` (the first one acts),
    /// `stacks` (a non-acting player's slots, by uuid), `day_time`,
    /// `weather` and one cow (mob 7); `tell` appends a line.
    #[derive(Default)]
    struct RecordingHost {
        draws: Mutex<Vec<String>>,
        broadcasts: Mutex<u64>,
        tells: Mutex<Vec<String>>,
        players: Vec<NativeServerPlayer>,
        stacks: HashMap<u128, Vec<Option<ItemStack>>>,
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
}
