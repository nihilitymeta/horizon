//! The Lua side of every zero-hand-work native hook: one event table
//! per hook under the native name, over the hand-written runtime in
//! `super::generated_runtime`. Do not edit: `cargo run -p xtask -- lua-hooks`
//! regenerates this file, and `--check` fails when it is stale. Every
//! native hook is classified exactly once below (`HOOK_CLASSIFICATION`),
//! so a newly added native hook is either generated on re-run or fails the
//! check naming it.

/// Every native hook in exactly one class: `generated`, `adapter` +
/// the missing converter, `tick` (the level-tick host), `cannot` + the
/// reason. A new native hook lands here on re-run; `--check` fails until
/// the file is regenerated, so the new hook is reviewed, not silently
/// dropped. Only the classification test below reads it at runtime.
#[allow(dead_code)]
pub const HOOK_CLASSIFICATION: &[(&str, &str)] = &[
    (
        "frame",
        "adapter: frame: adapter: a duration needs its ticks",
    ),
    ("tick", "generated"),
    (
        "visuals",
        "cannot: visuals: unsupported: NativeBlockRim borrows render state (render thread); visuals: unsupported: NativeColoredLight borrows render state (render thread)",
    ),
    (
        "emissive",
        "adapter: emissive: adapter: a tuple needs its table form",
    ),
    (
        "world_lifecycle",
        "cannot: world_lifecycle: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("server_about_to_start", "generated"),
    ("server_stopping", "generated"),
    (
        "server_tick",
        "cannot: server_tick: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("level_tick_pre", "generated"),
    ("level_tick_post", "generated"),
    ("block_changed", "generated"),
    (
        "neighbor_changed",
        "adapter: neighbor_changed: adapter: a block id needs its name",
    ),
    (
        "random_tick",
        "cannot: random_tick: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "tick_block",
        "cannot: tick_block: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "update_shape",
        "tick: a `ticks` parameter (level-tick host)",
    ),
    (
        "hud",
        "cannot: hud: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("key", "generated"),
    ("wheel", "generated"),
    (
        "command",
        "cannot: command: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "commands",
        "adapter: commands: adapter: needs owned strings",
    ),
    (
        "zoom",
        "adapter: zoom: adapter: a replace out needs its table form",
    ),
    (
        "minimap",
        "adapter: minimap: adapter: a replace out needs its table form",
    ),
    (
        "mods",
        "adapter: mods: adapter: a native_mod list needs its table form",
    ),
    ("settings", "generated"),
    ("settings_click", "generated"),
    (
        "panel_event",
        "cannot: panel_event: unsupported: NativePanelEvent borrows render state (render thread)",
    ),
    (
        "block_entity",
        "cannot: block_entity: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "block_entity_ticks",
        "cannot: block_entity_ticks: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "block_entity_tick",
        "cannot: block_entity_tick: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "block_entity_update_tag",
        "adapter: block_entity_update_tag: adapter: a replace out needs its table form",
    ),
    (
        "block_entity_container",
        "adapter: block_entity_container: adapter: a replace out needs its table form",
    ),
    (
        "screen",
        "adapter: screen: adapter: a replace out needs its table form",
    ),
    (
        "screen_draw",
        "cannot: screen_draw: unsupported: HudBatch borrows render state (render thread)",
    ),
    ("screen_click", "generated"),
    ("screen_tooltip", "generated"),
    (
        "menu_button",
        "adapter: menu_button: adapter: a mod_block_entity needs its table form",
    ),
    (
        "register_commands",
        "cannot: register_commands: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "perform_command",
        "cannot: perform_command: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("menu_clicked", "generated"),
    ("menu_closed", "generated"),
    (
        "tooltip",
        "adapter: tooltip: adapter: a tuple needs its table form",
    ),
    (
        "use_item_on",
        "cannot: use_item_on: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "use_item",
        "cannot: use_item: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "destroy_block",
        "cannot: destroy_block: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "place_block",
        "cannot: place_block: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "right_click_block",
        "cannot: right_click_block: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("block_spread", "generated"),
    (
        "piston_move",
        "adapter: piston_move: adapter: a block list needs its table form; piston_move: adapter: a block list needs its table form",
    ),
    (
        "explosion_detonate",
        "cannot: explosion_detonate: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("fill_bucket", "generated"),
    (
        "attack_entity",
        "cannot: attack_entity: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "net_from_client",
        "adapter: net_from_client: adapter: bytes need their table form",
    ),
    (
        "net_from_server",
        "adapter: net_from_server: adapter: bytes need their table form",
    ),
    (
        "player_died",
        "cannot: player_died: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "mob_spawned",
        "cannot: mob_spawned: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "mob_killed",
        "cannot: mob_killed: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "item_dropped",
        "cannot: item_dropped: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "item_picked_up",
        "cannot: item_picked_up: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "hurt_enemy",
        "cannot: hurt_enemy: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "inventory_tick",
        "cannot: inventory_tick: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("on_equip", "generated"),
    (
        "block_signal",
        "adapter: block_signal: adapter: a direction needs its name",
    ),
    ("is_signal_source", "generated"),
    (
        "health_hud",
        "adapter: health_hud: adapter: a replace out needs its table form",
    ),
    (
        "interact_entity",
        "cannot: interact_entity: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("left_click_block", "generated"),
    (
        "player_tick",
        "cannot: player_tick: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "entity_join_level",
        "cannot: entity_join_level: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "living_incoming_damage",
        "cannot: living_incoming_damage: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "menu_button_v2",
        "adapter: menu_button_v2: adapter: a mod_block_entity needs its table form; menu_button_v2: adapter: a mutable stack needs its table form",
    ),
    ("entity_leave_level", "generated"),
    (
        "block_entity_fluids",
        "adapter: block_entity_fluids: adapter: a replace out needs its table form",
    ),
    ("should_apply_effect_tick_this_tick", "generated"),
    ("apply_effect_tick", "generated"),
    ("on_effect_added", "generated"),
    ("mob_effect_added", "generated"),
    ("chunk_load", "generated"),
    ("chunk_unload", "generated"),
    (
        "player_logged_in",
        "cannot: player_logged_in: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "player_logged_out",
        "cannot: player_logged_out: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "world_drawn",
        "cannot: world_drawn: unsupported: WorldDrawn borrows render state (render thread)",
    ),
    (
        "render_needs",
        "cannot: render_needs: unsupported: RenderNeeds borrows render state (render thread)",
    ),
    (
        "item_bar",
        "adapter: item_bar: adapter: a tuple needs its table form",
    ),
    ("has_analog_output", "generated"),
    (
        "analog_output",
        "adapter: analog_output: adapter: a direction needs its name",
    ),
    ("component_types", "generated"),
    (
        "dispense_item",
        "adapter: dispense_item: adapter: a replace out needs its table form",
    ),
    ("tooltip_component", "generated"),
    (
        "tooltip_component_draw",
        "cannot: tooltip_component_draw: unsupported: HudBatch borrows render state (render thread)",
    ),
    ("can_survive", "generated"),
    ("may_place_on", "generated"),
    (
        "block_entity_submit",
        "cannot: block_entity_submit: unsupported: BlockEntityDraw borrows render state (render thread)",
    ),
    (
        "block_entity_render_bounding_box",
        "adapter: block_entity_render_bounding_box: adapter: a block state needs a host to name it; block_entity_render_bounding_box: adapter: a mod_block_entity needs its table form; block_entity_render_bounding_box: adapter: a mutable array needs its table form",
    ),
    ("is_valid_bonemeal_target", "generated"),
    ("is_bonemeal_success", "generated"),
    (
        "perform_bonemeal",
        "tick: a `ticks` parameter (level-tick host)",
    ),
    (
        "block_step_on",
        "adapter: block_step_on: adapter: a block state needs a host to name it",
    ),
    (
        "block_entity_inside",
        "adapter: block_entity_inside: adapter: a block state needs a host to name it",
    ),
    (
        "block_fall_on",
        "adapter: block_fall_on: adapter: a block state needs a host to name it",
    ),
    (
        "consume_effect",
        "adapter: consume_effect: adapter: a compound needs the nbt table",
    ),
    (
        "loot_table_load",
        "adapter: loot_table_load: adapter: a compound needs the nbt table",
    ),
    (
        "loot_drops",
        "cannot: loot_drops: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "state_for_placement",
        "adapter: state_for_placement: adapter: a replace out needs its table form",
    ),
    (
        "enchant_effect",
        "adapter: enchant_effect: adapter: a compound needs the nbt table",
    ),
    (
        "screen_render_post",
        "cannot: screen_render_post: unsupported: HudBatch borrows render state (render thread); screen_render_post: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "modify_default_components",
        "adapter: modify_default_components: adapter: a compound needs the nbt table",
    ),
    (
        "screen_mouse_button_pressed_pre",
        "cannot: screen_mouse_button_pressed_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "screen_mouse_button_released_pre",
        "cannot: screen_mouse_button_released_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "camera_shake",
        "cannot: camera_shake: unsupported: NativeShake borrows render state (render thread)",
    ),
    (
        "shader_params",
        "adapter: shader_params: adapter: a mutable array needs its table form",
    ),
    (
        "block_highlight",
        "adapter: block_highlight: adapter: a block state needs a host to name it",
    ),
    (
        "render_hand",
        "adapter: render_hand: adapter: a hand needs its name",
    ),
    ("client_resources_reloaded", "generated"),
    (
        "item_decorations",
        "cannot: item_decorations: unsupported: HudBatch borrows render state (render thread)",
    ),
    ("use_duration", "generated"),
    (
        "use_animation",
        "adapter: use_animation: adapter: a use_animation needs its table form",
    ),
    ("can_eat", "generated"),
    (
        "living_entity_use_item_start",
        "cannot: living_entity_use_item_start: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "living_entity_use_item_finish",
        "cannot: living_entity_use_item_finish: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "screen_mouse_scrolled_pre",
        "cannot: screen_mouse_scrolled_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "player_command",
        "cannot: player_command: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("perform_console_command", "generated"),
    (
        "screen_key_pressed_pre",
        "cannot: screen_key_pressed_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "screen_char_typed_pre",
        "cannot: screen_char_typed_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "data_pack_files",
        "cannot: data_pack_files: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "biome_colors",
        "cannot: biome_colors: unsupported: BiomeColors borrows render state (render thread)",
    ),
    ("server_data_loaded", "generated"),
    ("compute_fov", "generated"),
    (
        "mob_submit",
        "cannot: mob_submit: unsupported: NativeMobRender borrows render state (render thread); mob_submit: unsupported: BlockEntityDraw borrows render state (render thread)",
    ),
    (
        "camera_distance",
        "cannot: camera_distance: unsupported: CameraEntity borrows render state (render thread)",
    ),
    (
        "compute_fog_color",
        "cannot: compute_fog_color: unsupported: FogType borrows render state (render thread)",
    ),
    ("mouse_button_pre", "generated"),
    ("mouse_button_post", "generated"),
    (
        "player_respawned",
        "cannot: player_respawned: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "render_fog",
        "cannot: render_fog: unsupported: FogType borrows render state (render thread); render_fog: unsupported: FogData borrows render state (render thread)",
    ),
    (
        "screen_key_released_pre",
        "cannot: screen_key_released_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "music",
        "adapter: music: adapter: an optional music needs its table form",
    ),
    (
        "music_layers",
        "adapter: music_layers: adapter: a music_layer list needs its table form",
    ),
    ("datapack_sync", "generated"),
    ("recipes_received", "generated"),
    (
        "input_key",
        "cannot: input_key: unsupported: a borrowed client key event cannot enter the Lua state",
    ),
    (
        "mouse_scrolling",
        "adapter: mouse_scrolling: adapter: a 2D int array needs its table form; mouse_scrolling: adapter: an array needs its table form",
    ),
    (
        "screen_mouse_dragged_pre",
        "cannot: screen_mouse_dragged_pre: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    ("mod_container_viewers", "generated"),
    (
        "living_knockback",
        "cannot: living_knockback: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("experience_orb_can_merge", "generated"),
    ("experience_orb_merge", "generated"),
    ("menu_slot_rules", "generated"),
    (
        "living_entity_use_item_tick",
        "adapter: living_entity_use_item_tick: adapter: a hand needs its name",
    ),
    ("on_use_tick", "generated"),
    (
        "living_entity_use_item_stop",
        "adapter: living_entity_use_item_stop: adapter: a hand needs its name",
    ),
    ("release_using", "generated"),
    (
        "loot_function",
        "cannot: loot_function: unsupported: NativeLootContext is borrowed loot state; loot_function: unsupported: LootStack is borrowed loot state",
    ),
    (
        "loot_condition",
        "cannot: loot_condition: unsupported: NativeLootContext is borrowed loot state",
    ),
    ("level_load", "generated"),
    ("level_unload", "generated"),
    ("client_level_load", "generated"),
    ("client_level_unload", "generated"),
    (
        "hears_projectile_impacts",
        "cannot: hears_projectile_impacts: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "projectile_impact",
        "cannot: projectile_impact: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("entity_mount", "generated"),
    (
        "interaction_key",
        "adapter: interaction_key: adapter: a hand needs its name",
    ),
    (
        "gui_layer_pre",
        "cannot: gui_layer_pre: unsupported: HudBatch borrows render state (render thread)",
    ),
    (
        "gui_layer_post",
        "cannot: gui_layer_post: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "map_decorations",
        "adapter: map_decorations: adapter: native_map_info needs its table form; map_decorations: adapter: a native_map_decoration list needs its table form",
    ),
    ("player_start_tracking", "generated"),
    ("player_stop_tracking", "generated"),
    (
        "block_entity_energy",
        "adapter: block_entity_energy: adapter: a replace out needs its table form",
    ),
    (
        "data_map_types",
        "adapter: data_map_types: adapter: a native_data_map_type list needs its table form",
    ),
    ("living_breathe", "generated"),
    (
        "blocks",
        "adapter: blocks: adapter: a block list needs its table form",
    ),
    (
        "items",
        "adapter: items: adapter: a item list needs its table form",
    ),
    ("entity_tick_pre", "generated"),
    (
        "entity_tick_post",
        "cannot: entity_tick_post: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "entity_tick_listened",
        "cannot: entity_tick_listened: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("item_fluid_tank", "generated"),
    (
        "living_equipment_change",
        "cannot: living_equipment_change: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "living_equipment_listened",
        "cannot: living_equipment_listened: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "player_submit",
        "cannot: player_submit: unsupported: NativeMobRender borrows render state (render thread); player_submit: unsupported: NativePlayerRender borrows render state (render thread); player_submit: unsupported: BlockEntityDraw borrows render state (render thread)",
    ),
    (
        "mob_layer",
        "cannot: mob_layer: unsupported: NativeMobRender borrows render state (render thread); mob_layer: unsupported: BlockEntityDraw borrows render state (render thread)",
    ),
    ("player_render_listened", "generated"),
    ("sign_changed", "generated"),
    ("standalone_models", "generated"),
    (
        "player_container_open",
        "cannot: player_container_open: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("player_container_close", "generated"),
    ("player_toggle_flight", "generated"),
    ("anvil_damaged", "generated"),
    ("arrow_loose", "generated"),
    ("beacon_state_change", "generated"),
    ("bell_ring", "generated"),
    ("block_break_progress_update", "generated"),
    ("block_burn", "generated"),
    ("block_cook", "generated"),
    ("block_damage_abort", "generated"),
    ("block_dispense_armor", "generated"),
    ("block_exp", "generated"),
    ("block_fade", "generated"),
    ("block_failed_dispense", "generated"),
    ("block_form", "generated"),
    ("block_ignite", "generated"),
    ("block_lock_check", "generated"),
    ("block_receive_game", "generated"),
    ("block_shear_entity", "generated"),
    ("brewing_complete", "generated"),
    ("brewing_fuel", "generated"),
    ("brewing_start", "generated"),
    (
        "broadcast_message",
        "adapter: broadcast_message: adapter: rich text needs its plain form",
    ),
    ("campfire_start", "generated"),
    ("cauldron_level_changed", "generated"),
    ("chorus_fruit_teleport", "generated"),
    ("compost_item", "generated"),
    ("crafter_craft", "generated"),
    ("creeper_ignite", "generated"),
    ("creeper_power", "generated"),
    ("critical_hit", "generated"),
    ("difficulty_changed", "generated"),
    ("dragon_egg_form", "generated"),
    ("enchant_item", "generated"),
    ("ender_pearl_teleport", "generated"),
    ("entities_load", "generated"),
    ("entities_unload", "generated"),
    ("entity_break_door", "generated"),
    ("entity_breed", "generated"),
    ("entity_change_block", "generated"),
    ("entity_drop_item", "generated"),
    ("entity_enter_block", "generated"),
    ("entity_enter_love_mode", "generated"),
    ("entity_portal", "generated"),
    ("entity_pose_change", "generated"),
    ("entity_shoot_bow", "generated"),
    ("entity_tame", "generated"),
    ("entity_teleport", "generated"),
    ("entity_toggle_glide", "generated"),
    ("entity_transform", "generated"),
    ("entity_unleash", "generated"),
    ("exp_bottle", "generated"),
    ("experience_orb_pickup_listened", "generated"),
    ("experience_orb_pickup", "generated"),
    ("mending_repair", "generated"),
    ("explosion_prime", "generated"),
    (
        "fire_odds",
        "adapter: fire_odds: adapter: a replace out needs its table form",
    ),
    ("smoke_source", "generated"),
    ("firework_explode", "generated"),
    ("fluid_level_change", "generated"),
    ("framerate_limit", "generated"),
    ("furnace_burn", "generated"),
    ("furnace_extract", "generated"),
    ("furnace_smelt", "generated"),
    ("furnace_start_smelt", "generated"),
    ("game_event", "generated"),
    ("hopper_inventory_search", "generated"),
    ("hopper_pickup_item", "generated"),
    (
        "ingredient",
        "adapter: ingredient: adapter: a compound needs the nbt table",
    ),
    ("inventory_block_start", "generated"),
    ("inventory_move_item", "generated"),
    (
        "item_crafted",
        "cannot: item_crafted: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("item_despawn", "generated"),
    (
        "item_energy_buffer",
        "adapter: item_energy_buffer: adapter: a replace out needs its table form",
    ),
    ("item_expire", "generated"),
    ("item_spawn", "generated"),
    ("leaves_decay", "generated"),
    ("lightning_strike", "generated"),
    (
        "menu_quick_move",
        "adapter: menu_quick_move: adapter: a mod_block_entity needs its table form; menu_quick_move: adapter: a mutable stack needs its table form",
    ),
    ("moisture_change", "generated"),
    (
        "anvil_craft_post",
        "adapter: anvil_craft_post: adapter: a anvil_craft needs its table form",
    ),
    (
        "anvil_craft_pre",
        "adapter: anvil_craft_pre: adapter: a anvil_craft needs its table form",
    ),
    ("armor_stand_manipulate", "generated"),
    ("bucket_empty", "generated"),
    ("container_opened", "generated"),
    ("creative_slot", "generated"),
    ("food_level_changed", "generated"),
    ("hanging_break", "generated"),
    ("hanging_place", "generated"),
    ("item_damaged", "generated"),
    (
        "player_changed_dimension",
        "cannot: player_changed_dimension: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("player_clone", "generated"),
    ("player_commands_sent", "generated"),
    ("player_destroy_item", "generated"),
    ("player_fish", "generated"),
    ("player_flower_pot_manipulate", "generated"),
    ("player_game_mode_changed", "generated"),
    ("player_insert_lectern_book", "generated"),
    ("player_item_frame_change", "generated"),
    ("player_item_mend", "generated"),
    ("player_lectern_page_change", "generated"),
    (
        "player_load_from_file",
        "adapter: player_load_from_file: adapter: a path needs its text form",
    ),
    (
        "player_login",
        "adapter: player_login: adapter: rich text needs its plain form",
    ),
    (
        "player_save_to_file",
        "adapter: player_save_to_file: adapter: a path needs its text form",
    ),
    ("player_sprint", "generated"),
    ("player_swap_hands", "generated"),
    ("player_take_lectern_book", "generated"),
    ("player_teleported", "generated"),
    ("player_totem", "generated"),
    ("resource_pack_status", "generated"),
    ("spawn_change", "generated"),
    ("tnt_primed", "generated"),
    ("vehicle_create", "generated"),
    ("vehicle_damage", "generated"),
    ("vehicle_destroy", "generated"),
    ("vehicle_moved", "generated"),
    ("world_border_bounds_changed", "generated"),
    ("world_border_center_changed", "generated"),
    ("world_game_rule_change", "generated"),
    ("note_play", "generated"),
    ("player_advancement_criterion_grant", "generated"),
    ("player_advancement_done", "generated"),
    ("player_advancement_progress", "generated"),
    ("player_arm_swing", "generated"),
    ("player_attack_cooldown_reset", "generated"),
    (
        "player_bed_enter",
        "cannot: player_bed_enter: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("player_bed_leave", "generated"),
    ("player_bucket_entity", "generated"),
    ("player_bundle_item_select", "generated"),
    ("player_change_beacon_effect", "generated"),
    ("player_change_main_hand", "generated"),
    ("player_client_loaded_world", "generated"),
    ("player_deep_sleep", "generated"),
    ("player_edit_book", "generated"),
    ("player_egg_throw", "generated"),
    ("player_elytra_boost", "generated"),
    (
        "player_exp_change",
        "cannot: player_exp_change: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "player_exp_cooldown_change",
        "tick: a `ticks` parameter (level-tick host)",
    ),
    ("player_input", "generated"),
    (
        "player_item_cooldown",
        "tick: a `ticks` parameter (level-tick host)",
    ),
    ("player_item_held", "generated"),
    ("player_jump", "generated"),
    ("player_kicked", "generated"),
    ("player_launch_projectile", "generated"),
    ("player_level_change", "generated"),
    ("player_locale_change", "generated"),
    ("player_loom_pattern_select", "generated"),
    ("player_map_filled", "generated"),
    ("player_name_entity", "generated"),
    ("player_open_sign", "generated"),
    ("player_pick_item", "generated"),
    ("player_pickup_arrow", "generated"),
    ("player_portal", "generated"),
    ("player_ready_arrow", "generated"),
    ("player_recipe_book_click", "generated"),
    ("player_recipe_book_settings_change", "generated"),
    ("player_recipe_discovered", "generated"),
    ("player_riptide", "generated"),
    ("player_shear_entity", "generated"),
    (
        "player_shield_disable",
        "tick: a `ticks` parameter (level-tick host)",
    ),
    ("player_spawn_point_set", "generated"),
    ("player_stat_award", "generated"),
    ("player_stonecutter_recipe_select", "generated"),
    ("player_stop_spectating_entity", "generated"),
    ("player_stop_using_item", "generated"),
    ("player_swap_with_equipment_slot", "generated"),
    ("player_teleport_end_gateway", "generated"),
    ("player_toggle_entity_age_lock", "generated"),
    ("player_toggle_sneak", "generated"),
    ("player_trade", "generated"),
    ("player_unleash_entity", "generated"),
    ("player_velocity", "generated"),
    ("portal_create", "generated"),
    ("potion_splash", "generated"),
    ("prepare_anvil", "generated"),
    ("prepare_grindstone", "generated"),
    ("prepare_hopper_transfer", "generated"),
    ("prepare_item_craft", "generated"),
    ("prepare_item_enchant", "generated"),
    ("prepare_smithing", "generated"),
    (
        "projectile_launch",
        "cannot: projectile_launch: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("raid_spawn_wave", "generated"),
    ("raid_state_change", "generated"),
    ("sculk_bloom", "generated"),
    ("sleep_finished_time", "generated"),
    ("sponge_absorb", "generated"),
    (
        "tab_complete",
        "adapter: tab_complete: adapter: an array needs its table form; tab_complete: adapter: a suggestion list needs its table form",
    ),
    ("target_hit", "generated"),
    ("thunder_change", "generated"),
    ("time_skip", "generated"),
    ("trade_select", "generated"),
    (
        "trade_with_villager",
        "adapter: trade_with_villager: adapter: a merchant_offer needs its table form",
    ),
    ("vault_change_state", "generated"),
    ("vault_loot", "generated"),
    ("weather_change", "generated"),
    ("whitelist_state_update", "generated"),
    ("world_border_bounds_change_finish", "generated"),
    ("world_saved", "generated"),
    ("living_drown", "generated"),
    (
        "living_fall",
        "cannot: living_fall: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "living_heal",
        "cannot: living_heal: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("living_jump", "generated"),
    (
        "arrow_nock",
        "adapter: arrow_nock: adapter: a hand needs its name",
    ),
    ("baby_entity_spawn", "generated"),
    ("can_player_sleep", "generated"),
    (
        "compute_fov_modifier",
        "cannot: compute_fov_modifier: unsupported: FovPlayer borrows render state (render thread)",
    ),
    (
        "explosion_knockback",
        "adapter: explosion_knockback: adapter: a slice needs its table form",
    ),
    (
        "hand_drawn",
        "cannot: hand_drawn: unsupported: HandDrawn borrows render state (render thread)",
    ),
    (
        "ingredient_test",
        "adapter: ingredient_test: adapter: a compound needs the nbt table",
    ),
    ("item_container", "generated"),
    (
        "item_model_property",
        "cannot: item_model_property: unsupported: StackView borrows render state (render thread)",
    ),
    ("living_conversion_post", "generated"),
    ("living_conversion_pre", "generated"),
    ("living_death", "generated"),
    (
        "living_get_projectile",
        "adapter: living_get_projectile: adapter: a stack out needs its table form",
    ),
    (
        "living_incoming_damage_v2",
        "adapter: living_incoming_damage_v2: adapter: an optional array needs its table form",
    ),
    ("living_knockback_v2", "generated"),
    ("mob_split", "generated"),
    (
        "register_configs",
        "adapter: register_configs: adapter: a native_config list needs its table form",
    ),
    (
        "config_loading",
        "adapter: config_loading: adapter: a native_config_file needs its table form",
    ),
    (
        "config_reloading",
        "adapter: config_reloading: adapter: a native_config_file needs its table form",
    ),
    (
        "config_unloading",
        "adapter: config_unloading: adapter: a native_config_file needs its table form",
    ),
    ("movement_input", "generated"),
    (
        "note_block_change",
        "adapter: note_block_change: adapter: native_note_block_change needs its table form",
    ),
    (
        "potential_spawns",
        "adapter: potential_spawns: adapter: a spawn_entry list needs its table form",
    ),
    ("village_siege", "generated"),
    (
        "player_chat",
        "cannot: player_chat: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    ("player_xp_change", "generated"),
    (
        "recipe_book_categories",
        "adapter: recipe_book_categories: adapter: a recipe_book_category list needs its table form",
    ),
    (
        "render_hand_pose",
        "adapter: render_hand_pose: adapter: a hand needs its name",
    ),
    ("render_item_in_frame", "generated"),
    ("render_name_tag", "generated"),
    (
        "screen_closing",
        "cannot: screen_closing: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "screen_init_post",
        "cannot: screen_init_post: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    (
        "screen_opening",
        "cannot: screen_opening: unsupported: NativeScreenView borrows render state (render thread)",
    ),
    ("tooltip_color", "generated"),
    ("camera_pose", "generated"),
    (
        "client_chat_received",
        "adapter: client_chat_received: adapter: rich text needs its plain form",
    ),
    (
        "client_packet",
        "adapter: client_packet: adapter: a packet needs its table form",
    ),
    (
        "menu_extra_data",
        "adapter: menu_extra_data: adapter: a replace out needs its table form",
    ),
    (
        "merchant_offer_traded",
        "adapter: merchant_offer_traded: adapter: a merchant_offer needs its table form",
    ),
    (
        "add_server_reload_listeners",
        "adapter: add_server_reload_listeners: adapter: a native_reload_listener list needs its table form",
    ),
    (
        "server_reload_listener_apply",
        "adapter: server_reload_listener_apply: adapter: a native_reload_listener needs its table form; server_reload_listener_apply: adapter: a slice needs its table form",
    ),
    ("client_start", "generated"),
    ("client_disconnecting", "generated"),
    ("client_stopping", "generated"),
    ("menu_background", "generated"),
    (
        "gui_render_post",
        "cannot: gui_render_post: unsupported: HudBatch borrows render state (render thread)",
    ),
    (
        "anvil_update",
        "adapter: anvil_update: adapter: a mutable stack needs its table form",
    ),
    (
        "block_clone_item",
        "adapter: block_clone_item: adapter: a block state needs a host to name it",
    ),
    ("block_entity_accepts_fluid", "generated"),
    ("block_place", "generated"),
    ("block_redstone", "generated"),
    (
        "break_speed",
        "adapter: break_speed: adapter: a block state needs a host to name it",
    ),
    ("chat_suggestion_applied", "generated"),
    (
        "chunk_generate",
        "cannot: chunk_generate: unsupported: NativeProtoChunk borrows the generating chunk (worldgen); chunk_generate: unsupported: NativeGenerate borrows the generating chunk (worldgen)",
    ),
    ("client_attack", "generated"),
    ("client_hurt", "generated"),
    ("client_player_network_logging_in", "generated"),
    ("client_tick_pre", "generated"),
    ("client_tick_post", "generated"),
    (
        "console_line",
        "adapter: console_line: adapter: rich text needs its plain form",
    ),
    (
        "container_accepts",
        "adapter: container_accepts: adapter: a mod_block_entity needs its table form; container_accepts: adapter: an optional direction needs its name",
    ),
    ("creative_slot_v2", "generated"),
    ("custom_block_fluids", "generated"),
    (
        "custom_fluid_tanks",
        "adapter: custom_fluid_tanks: adapter: a replace out needs its table form",
    ),
    (
        "custom_fluid_fill",
        "adapter: custom_fluid_fill: adapter: an optional direction needs its name; custom_fluid_fill: adapter: a native_fluid_stack needs its table form; custom_fluid_fill: adapter: a native_fluid_action needs its table form",
    ),
    (
        "custom_fluid_drain",
        "adapter: custom_fluid_drain: adapter: a replace out needs its table form",
    ),
    (
        "custom_payload_from_client",
        "adapter: custom_payload_from_client: adapter: bytes need their table form",
    ),
    ("entity_change_block_v2", "generated"),
    (
        "entity_data_to",
        "adapter: entity_data_to: adapter: a tuple needs its table form",
    ),
    ("entity_interact_specific", "generated"),
    ("entity_outline", "generated"),
    ("fishing_hook_landed", "generated"),
    (
        "handshake",
        "adapter: handshake: adapter: an address needs its text form; handshake: adapter: rich text needs its plain form",
    ),
    (
        "interact_entity_v2",
        "adapter: interact_entity_v2: adapter: a hand needs its name",
    ),
    ("inventory_click", "generated"),
    ("inventory_drag", "generated"),
    (
        "item_after_vertical_collision",
        "adapter: item_after_vertical_collision: adapter: a mutable stack needs its table form",
    ),
    ("item_dropped_v2", "generated"),
    ("item_interact_mob", "generated"),
    ("item_picked_up_v2", "generated"),
    ("living_change_target", "generated"),
    ("living_damage_pre", "generated"),
    ("living_damage_post", "generated"),
    ("living_incoming_damage_v3", "generated"),
    (
        "lod_column",
        "cannot: lod_column: unsupported: NativeLodColumn borrows the generating chunk (worldgen)",
    ),
    ("log_line", "generated"),
    (
        "menu_input_closed",
        "adapter: menu_input_closed: adapter: a stack out needs its table form",
    ),
    (
        "menu_links",
        "adapter: menu_links: adapter: a block list needs its table form",
    ),
    (
        "mob_ai_register",
        "cannot: mob_ai_register: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "mob_died",
        "cannot: mob_died: unsupported: the hand-written hook keeps this name (removing it is #44)",
    ),
    (
        "layer_definitions",
        "adapter: layer_definitions: adapter: a tuple needs its table form",
    ),
    ("permission_check", "generated"),
    ("player_register_channels", "generated"),
    ("player_command_level", "generated"),
    ("player_commands_sent_v2", "generated"),
    (
        "player_fish_v2",
        "adapter: player_fish_v2: adapter: a hand needs its name",
    ),
    ("player_hurt_by_player", "generated"),
    ("player_launch_projectile_v2", "generated"),
    ("player_logging_out", "generated"),
    ("player_move", "generated"),
    (
        "player_pre_login",
        "adapter: player_pre_login: adapter: an address needs its text form; player_pre_login: adapter: a tuple needs its table form",
    ),
    (
        "player_profile",
        "adapter: player_profile: adapter: a property list needs its table form",
    ),
    ("player_respawn_position", "generated"),
    ("player_spawn_location", "generated"),
    ("player_teleport", "generated"),
    ("player_tick_pre", "generated"),
    ("projectile_world_border", "generated"),
    (
        "recipe_sources",
        "adapter: recipe_sources: adapter: a block list needs its table form",
    ),
    (
        "register_goals",
        "adapter: register_goals: adapter: a live mob needs its table form",
    ),
    ("resource_pack_status_v2", "generated"),
    (
        "right_click_block_v2",
        "adapter: right_click_block_v2: adapter: a hand needs its name",
    ),
    ("screen_mouse_dragged_pre_v2", "generated"),
    ("screen_mouse_scrolled_pre_v2", "generated"),
    (
        "screen_text_field_char_typed_pre",
        "cannot: screen_text_field_char_typed_pre: unsupported: NativeScreenView borrows render state (render thread); screen_text_field_char_typed_pre: unsupported: NativeTextField borrows render state (render thread)",
    ),
    (
        "screen_transparent_background",
        "cannot: screen_transparent_background: unsupported: HudBatch borrows render state (render thread)",
    ),
    (
        "server_packet",
        "adapter: server_packet: adapter: a packet needs its table form",
    ),
    (
        "server_packets",
        "adapter: server_packets: adapter: a slice needs its table form",
    ),
    (
        "server_tick_timed",
        "adapter: server_tick_timed: adapter: a slice needs its table form",
    ),
    (
        "sound_play",
        "adapter: sound_play: adapter: sound_playback needs its table form",
    ),
    ("stateful_block_entity", "generated"),
    (
        "block_entity_from_state",
        "adapter: block_entity_from_state: adapter: a replace out needs its table form",
    ),
    (
        "tab_complete_v2",
        "adapter: tab_complete_v2: adapter: an array needs its table form; tab_complete_v2: adapter: a suggestion list needs its table form",
    ),
    ("toast_add", "generated"),
    (
        "tooltip_render_pre",
        "cannot: tooltip_render_pre: unsupported: HudBatch borrows render state (render thread)",
    ),
    ("trample", "generated"),
    ("vehicle_moved_v2", "generated"),
    (
        "wind_burst",
        "adapter: wind_burst: adapter: a block list needs its table form",
    ),
    (
        "world_submit",
        "cannot: world_submit: unsupported: NativeWorldDraw borrows render state (render thread)",
    ),
    (
        "world_hold",
        "cannot: world_hold: unsupported: WorldHold borrows render state (render thread)",
    ),
    (
        "world_primitives",
        "cannot: world_primitives: unsupported: WorldPrimitives borrows render state (render thread)",
    ),
];

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod spike_tests;

/// The generated hooks, for `horizon.on`'s name check.
pub const GENERATED_HOOK_NAMES: &[&str] = &[
    "tick",
    "server_about_to_start",
    "server_stopping",
    "level_tick_pre",
    "level_tick_post",
    "block_changed",
    "key",
    "wheel",
    "settings",
    "settings_click",
    "screen_click",
    "screen_tooltip",
    "menu_clicked",
    "menu_closed",
    "block_spread",
    "fill_bucket",
    "on_equip",
    "is_signal_source",
    "left_click_block",
    "entity_leave_level",
    "should_apply_effect_tick_this_tick",
    "apply_effect_tick",
    "on_effect_added",
    "mob_effect_added",
    "chunk_load",
    "chunk_unload",
    "has_analog_output",
    "component_types",
    "tooltip_component",
    "can_survive",
    "may_place_on",
    "is_valid_bonemeal_target",
    "is_bonemeal_success",
    "client_resources_reloaded",
    "use_duration",
    "can_eat",
    "perform_console_command",
    "server_data_loaded",
    "compute_fov",
    "mouse_button_pre",
    "mouse_button_post",
    "datapack_sync",
    "recipes_received",
    "mod_container_viewers",
    "experience_orb_can_merge",
    "experience_orb_merge",
    "menu_slot_rules",
    "on_use_tick",
    "release_using",
    "level_load",
    "level_unload",
    "client_level_load",
    "client_level_unload",
    "entity_mount",
    "player_start_tracking",
    "player_stop_tracking",
    "living_breathe",
    "entity_tick_pre",
    "item_fluid_tank",
    "player_render_listened",
    "sign_changed",
    "standalone_models",
    "player_container_close",
    "player_toggle_flight",
    "anvil_damaged",
    "arrow_loose",
    "beacon_state_change",
    "bell_ring",
    "block_break_progress_update",
    "block_burn",
    "block_cook",
    "block_damage_abort",
    "block_dispense_armor",
    "block_exp",
    "block_fade",
    "block_failed_dispense",
    "block_form",
    "block_ignite",
    "block_lock_check",
    "block_receive_game",
    "block_shear_entity",
    "brewing_complete",
    "brewing_fuel",
    "brewing_start",
    "campfire_start",
    "cauldron_level_changed",
    "chorus_fruit_teleport",
    "compost_item",
    "crafter_craft",
    "creeper_ignite",
    "creeper_power",
    "critical_hit",
    "difficulty_changed",
    "dragon_egg_form",
    "enchant_item",
    "ender_pearl_teleport",
    "entities_load",
    "entities_unload",
    "entity_break_door",
    "entity_breed",
    "entity_change_block",
    "entity_drop_item",
    "entity_enter_block",
    "entity_enter_love_mode",
    "entity_portal",
    "entity_pose_change",
    "entity_shoot_bow",
    "entity_tame",
    "entity_teleport",
    "entity_toggle_glide",
    "entity_transform",
    "entity_unleash",
    "exp_bottle",
    "experience_orb_pickup_listened",
    "experience_orb_pickup",
    "mending_repair",
    "explosion_prime",
    "smoke_source",
    "firework_explode",
    "fluid_level_change",
    "framerate_limit",
    "furnace_burn",
    "furnace_extract",
    "furnace_smelt",
    "furnace_start_smelt",
    "game_event",
    "hopper_inventory_search",
    "hopper_pickup_item",
    "inventory_block_start",
    "inventory_move_item",
    "item_despawn",
    "item_expire",
    "item_spawn",
    "leaves_decay",
    "lightning_strike",
    "moisture_change",
    "armor_stand_manipulate",
    "bucket_empty",
    "container_opened",
    "creative_slot",
    "food_level_changed",
    "hanging_break",
    "hanging_place",
    "item_damaged",
    "player_clone",
    "player_commands_sent",
    "player_destroy_item",
    "player_fish",
    "player_flower_pot_manipulate",
    "player_game_mode_changed",
    "player_insert_lectern_book",
    "player_item_frame_change",
    "player_item_mend",
    "player_lectern_page_change",
    "player_sprint",
    "player_swap_hands",
    "player_take_lectern_book",
    "player_teleported",
    "player_totem",
    "resource_pack_status",
    "spawn_change",
    "tnt_primed",
    "vehicle_create",
    "vehicle_damage",
    "vehicle_destroy",
    "vehicle_moved",
    "world_border_bounds_changed",
    "world_border_center_changed",
    "world_game_rule_change",
    "note_play",
    "player_advancement_criterion_grant",
    "player_advancement_done",
    "player_advancement_progress",
    "player_arm_swing",
    "player_attack_cooldown_reset",
    "player_bed_leave",
    "player_bucket_entity",
    "player_bundle_item_select",
    "player_change_beacon_effect",
    "player_change_main_hand",
    "player_client_loaded_world",
    "player_deep_sleep",
    "player_edit_book",
    "player_egg_throw",
    "player_elytra_boost",
    "player_input",
    "player_item_held",
    "player_jump",
    "player_kicked",
    "player_launch_projectile",
    "player_level_change",
    "player_locale_change",
    "player_loom_pattern_select",
    "player_map_filled",
    "player_name_entity",
    "player_open_sign",
    "player_pick_item",
    "player_pickup_arrow",
    "player_portal",
    "player_ready_arrow",
    "player_recipe_book_click",
    "player_recipe_book_settings_change",
    "player_recipe_discovered",
    "player_riptide",
    "player_shear_entity",
    "player_spawn_point_set",
    "player_stat_award",
    "player_stonecutter_recipe_select",
    "player_stop_spectating_entity",
    "player_stop_using_item",
    "player_swap_with_equipment_slot",
    "player_teleport_end_gateway",
    "player_toggle_entity_age_lock",
    "player_toggle_sneak",
    "player_trade",
    "player_unleash_entity",
    "player_velocity",
    "portal_create",
    "potion_splash",
    "prepare_anvil",
    "prepare_grindstone",
    "prepare_hopper_transfer",
    "prepare_item_craft",
    "prepare_item_enchant",
    "prepare_smithing",
    "raid_spawn_wave",
    "raid_state_change",
    "sculk_bloom",
    "sleep_finished_time",
    "sponge_absorb",
    "target_hit",
    "thunder_change",
    "time_skip",
    "trade_select",
    "vault_change_state",
    "vault_loot",
    "weather_change",
    "whitelist_state_update",
    "world_border_bounds_change_finish",
    "world_saved",
    "living_drown",
    "living_jump",
    "baby_entity_spawn",
    "can_player_sleep",
    "item_container",
    "living_conversion_post",
    "living_conversion_pre",
    "living_death",
    "living_knockback_v2",
    "mob_split",
    "movement_input",
    "village_siege",
    "player_xp_change",
    "render_item_in_frame",
    "render_name_tag",
    "tooltip_color",
    "camera_pose",
    "client_start",
    "client_disconnecting",
    "client_stopping",
    "menu_background",
    "block_entity_accepts_fluid",
    "block_place",
    "block_redstone",
    "chat_suggestion_applied",
    "client_attack",
    "client_hurt",
    "client_player_network_logging_in",
    "client_tick_pre",
    "client_tick_post",
    "creative_slot_v2",
    "custom_block_fluids",
    "entity_change_block_v2",
    "entity_interact_specific",
    "entity_outline",
    "fishing_hook_landed",
    "inventory_click",
    "inventory_drag",
    "item_dropped_v2",
    "item_interact_mob",
    "item_picked_up_v2",
    "living_change_target",
    "living_damage_pre",
    "living_damage_post",
    "living_incoming_damage_v3",
    "log_line",
    "permission_check",
    "player_register_channels",
    "player_command_level",
    "player_commands_sent_v2",
    "player_hurt_by_player",
    "player_launch_projectile_v2",
    "player_logging_out",
    "player_move",
    "player_respawn_position",
    "player_spawn_location",
    "player_teleport",
    "player_tick_pre",
    "projectile_world_border",
    "resource_pack_status_v2",
    "screen_mouse_dragged_pre_v2",
    "screen_mouse_scrolled_pre_v2",
    "stateful_block_entity",
    "toast_add",
    "trample",
    "vehicle_moved_v2",
];

/// The generated hooks whose `kind` filter runs: `horizon.on` rejects a
/// kind for every other generated hook.
pub const GENERATED_KIND_HOOK_NAMES: &[&str] = &[
    "entity_leave_level",
    "apply_effect_tick",
    "on_effect_added",
    "mob_effect_added",
    "on_use_tick",
    "release_using",
    "entity_mount",
    "player_start_tracking",
    "player_stop_tracking",
    "living_breathe",
    "entity_tick_pre",
    "block_shear_entity",
    "creeper_ignite",
    "creeper_power",
    "entity_break_door",
    "entity_change_block",
    "entity_drop_item",
    "entity_enter_block",
    "entity_enter_love_mode",
    "entity_portal",
    "entity_shoot_bow",
    "entity_tame",
    "entity_teleport",
    "entity_transform",
    "entity_unleash",
    "firework_explode",
    "hopper_inventory_search",
    "item_despawn",
    "item_expire",
    "item_spawn",
    "hanging_break",
    "hanging_place",
    "player_item_frame_change",
    "vehicle_create",
    "vehicle_damage",
    "vehicle_destroy",
    "vehicle_moved",
    "player_bucket_entity",
    "player_name_entity",
    "player_pick_item",
    "player_pickup_arrow",
    "player_shear_entity",
    "player_stop_spectating_entity",
    "player_toggle_entity_age_lock",
    "player_trade",
    "player_unleash_entity",
    "living_drown",
    "living_jump",
    "baby_entity_spawn",
    "living_conversion_post",
    "living_conversion_pre",
    "living_death",
    "living_knockback_v2",
    "mob_split",
    "render_item_in_frame",
    "render_name_tag",
    "client_attack",
    "entity_change_block_v2",
    "entity_interact_specific",
    "entity_outline",
    "item_interact_mob",
    "living_change_target",
    "living_damage_pre",
    "living_damage_post",
    "living_incoming_damage_v3",
    "projectile_world_border",
    "trample",
    "vehicle_moved_v2",
];

/// The `Hooks` methods: one macro invocation per hook (the table).
macro_rules! impl_generated_hooks {
    () => {
    gen_listen!(tick, game_mut, horizon_client::mod_api::TickEvent, (none), []);
    gen_listen!(server_about_to_start, game_mut, horizon_client::mod_api::ServerAboutToStartEvent, (none), [dedicated: copy]);
    gen_listen!(server_stopping, game_mut, horizon_client::mod_api::ServerStoppingEvent, (none), [dedicated: copy]);
    gen_listen!(level_tick_pre, game_mut, horizon_client::mod_api::LevelTickPreEvent<'_>, (none), [dimension: cloned]);
    gen_listen!(level_tick_post, game_mut, horizon_client::mod_api::LevelTickPostEvent<'_>, (none), [dimension: cloned]);
    gen_listen!(block_changed, level, horizon_client::mod_api::BlockChangedEvent, (none), [dimension: dim, pos: pos_struct, old: state_level, new: state_level]);
    gen_listen!(key, game_mut, horizon_client::mod_api::KeyEvent<'_>, (none), [key: cloned, pressed: copy]);
    gen_cancel!(wheel, game_mut, horizon_client::mod_api::WheelEvent, (none), [steps: copy]);
    gen_collect!(settings, game_mut, horizon_client::mod_api::SettingsEvent<'_>, (none), [mod_id: cloned, buttons: str_seq => mut_strvec]);
    gen_listen!(settings_click, game_mut, horizon_client::mod_api::SettingsClickEvent<'_>, (none), [mod_id: cloned, button: copy]);
    gen_replace!(screen_click, game_ref, horizon_client::mod_api::ScreenClickEvent<'_>, (none), [block: cloned, data: int_seq, at: pair, right: copy], button : value, Option<i32>);
    gen_collect!(screen_tooltip, game_ref, horizon_client::mod_api::ScreenTooltipEvent<'_>, (none), [block: cloned, data: int_seq, at: pair, lines: str_seq => mut_strvec]);
    gen_listen!(menu_clicked, game_mut, horizon_client::mod_api::MenuClickedEvent, (none), [uuid: copy, window: copy, slot: copy, button: copy, mode: copy]);
    gen_listen!(menu_closed, game_mut, horizon_client::mod_api::MenuClosedEvent, (none), [uuid: copy, window: copy]);
    gen_cancel!(block_spread, level, horizon_client::mod_api::BlockSpreadEvent<'_>, (none), [dimension: dim, kind: cloned, from: pos_struct, to: pos_struct]);
    gen_cancel!(fill_bucket, game_mut, horizon_client::mod_api::FillBucketEvent<'_>, (none), [stack: item, pos: pos]);
    gen_listen!(on_equip, game_mut, horizon_client::mod_api::OnEquipEvent<'_>, (none), [slot: cloned, new: opt_item]);
    gen_listen!(is_signal_source, level, horizon_client::mod_api::IsSignalSourceEvent, (none), [state: state_level, source: copy => mut_scalar]);
    gen_cancel!(left_click_block, game_mut, horizon_client::mod_api::LeftClickBlockEvent, (none), [pos: pos, face: pos, state: state_game]);
    gen_listen!(entity_leave_level, game_mut, horizon_client::mod_api::EntityLeaveLevelEvent<'_>, (field kind), [dimension: cloned, entity: copy, kind: cloned]);
    gen_listen!(should_apply_effect_tick_this_tick, game_ref, horizon_client::mod_api::ShouldApplyEffectTickThisTickEvent<'_>, (none), [effect: cloned, tick_count: copy, amplifier: copy, apply: copy => mut_scalar]);
    gen_listen!(apply_effect_tick, game_ref, horizon_client::mod_api::ApplyEffectTickEvent<'_>, (opt_entity entity), [effect: cloned, dimension: cloned, entity: copy, amplifier: copy, keep: copy => mut_scalar]);
    gen_listen!(on_effect_added, game_mut, horizon_client::mod_api::OnEffectAddedEvent<'_>, (opt_entity entity), [effect: cloned, entity: copy, amplifier: copy]);
    gen_listen!(mob_effect_added, game_mut, horizon_client::mod_api::MobEffectAddedEvent<'_>, (opt_entity entity), [effect: cloned, entity: copy, amplifier: copy]);
    gen_listen!(chunk_load, game_mut, horizon_client::mod_api::ChunkLoadEvent<'_>, (none), [dimension: cloned, chunk: chunk, new: copy]);
    gen_listen!(chunk_unload, game_mut, horizon_client::mod_api::ChunkUnloadEvent<'_>, (none), [dimension: cloned, chunk: chunk]);
    gen_collect!(has_analog_output, level, horizon_client::mod_api::HasAnalogOutputEvent, (none), [state: state_level, output: copy => mut_scalar]);
    gen_collect!(component_types, none, horizon_client::mod_api::ComponentTypesEvent<'_>, (none), [types: str_seq => mut_strvec]);
    gen_replace!(tooltip_component, game_ref, horizon_client::mod_api::TooltipComponentEvent<'_>, (none), [stack: item], size : pair, Option<[f32; 2]>);
    gen_replace!(can_survive, level, horizon_client::mod_api::CanSurviveEvent, (none), [dimension: dim, pos: pos_struct, state: state_level], survives : value, Option<bool>);
    gen_replace!(may_place_on, level, horizon_client::mod_api::MayPlaceOnEvent, (none), [dimension: dim, state: state_level, ground: state_level, ground_pos: pos_struct], may : value, Option<bool>);
    gen_replace!(is_valid_bonemeal_target, level, horizon_client::mod_api::IsValidBonemealTargetEvent, (none), [dimension: dim, pos: pos_struct, state: state_level], valid : value, Option<bool>);
    gen_collect!(is_bonemeal_success, level_mut, horizon_client::mod_api::IsBonemealSuccessEvent, (none), [dimension: dim, pos: pos_struct, success: copy => mut_scalar, block: block_from_state]);
    gen_listen!(client_resources_reloaded, game_mut, horizon_client::mod_api::ClientResourcesReloadedEvent, (none), []);
    gen_listen!(use_duration, game_ref, horizon_client::mod_api::UseDurationEvent<'_>, (none), [stack: item, duration: copy => mut_scalar]);
    gen_listen!(can_eat, game_ref, horizon_client::mod_api::CanEatEvent<'_>, (none), [stack: item, can_eat: copy => mut_scalar]);
    gen_cancel!(perform_console_command, game_mut, horizon_client::mod_api::PerformConsoleCommandEvent<'_>, (none), [command: cloned]);
    gen_listen!(server_data_loaded, game_mut, horizon_client::mod_api::ServerDataLoadedEvent, (none), [reload: copy]);
    gen_listen!(compute_fov, game_ref, horizon_client::mod_api::ComputeFovEvent, (none), [partial: copy, camera: vec3, degrees: copy => mut_scalar]);
    gen_cancel!(mouse_button_pre, game_mut, horizon_client::mod_api::MouseButtonPreEvent, (none), [button: copy, pressed: copy, modifiers: copy]);
    gen_listen!(mouse_button_post, game_mut, horizon_client::mod_api::MouseButtonPostEvent, (none), [button: copy, pressed: copy, modifiers: copy]);
    gen_collect!(datapack_sync, game_mut, horizon_client::mod_api::DatapackSyncEvent<'_>, (none), [recipe_types: str_seq => mut_strvec]);
    gen_listen!(recipes_received, game_mut, horizon_client::mod_api::RecipesReceivedEvent<'_>, (none), [recipe_types: str_seq]);
    gen_listen!(mod_container_viewers, game_mut, horizon_client::mod_api::ModContainerViewersEvent<'_>, (none), [dimension: cloned, pos: pos, viewers: copy, previous: copy]);
    gen_listen!(experience_orb_can_merge, game_mut, horizon_client::mod_api::ExperienceOrbCanMergeEvent<'_>, (none), [dimension: cloned, into: copy, from: copy, into_value: copy, from_value: copy, can_merge: copy => mut_scalar]);
    gen_cancel!(experience_orb_merge, game_mut, horizon_client::mod_api::ExperienceOrbMergeEvent<'_>, (none), [dimension: cloned, into: copy, from: copy, from_value: copy]);
    gen_listen!(menu_slot_rules, none, horizon_client::mod_api::MenuSlotRulesEvent<'_>, (none), [menu: cloned, slot: copy, stack: item, may_place: copy => mut_scalar, max_stack: copy => mut_scalar]);
    gen_listen!(on_use_tick, game_mut, horizon_client::mod_api::OnUseTickEvent<'_>, (opt_entity entity), [stack: item, entity: copy, remaining: copy]);
    gen_listen!(release_using, game_mut, horizon_client::mod_api::ReleaseUsingEvent<'_>, (opt_entity entity), [stack: item, entity: copy, time_left: copy, used: copy => mut_scalar]);
    gen_listen!(level_load, game_mut, horizon_client::mod_api::LevelLoadEvent<'_>, (none), [dimension: cloned]);
    gen_listen!(level_unload, game_mut, horizon_client::mod_api::LevelUnloadEvent<'_>, (none), [dimension: cloned]);
    gen_listen!(client_level_load, game_mut, horizon_client::mod_api::ClientLevelLoadEvent<'_>, (none), [dimension: cloned]);
    gen_listen!(client_level_unload, game_mut, horizon_client::mod_api::ClientLevelUnloadEvent<'_>, (none), [dimension: cloned]);
    gen_cancel!(entity_mount, game_mut, horizon_client::mod_api::EntityMountEvent<'_>, (field kind), [dimension: cloned, entity: copy, kind: cloned, vehicle: copy, vehicle_kind: cloned, mounting: copy]);
    gen_listen!(player_start_tracking, game_mut, horizon_client::mod_api::PlayerStartTrackingEvent<'_>, (field kind), [player: player, entity: copy, uuid: copy, kind: cloned]);
    gen_listen!(player_stop_tracking, game_mut, horizon_client::mod_api::PlayerStopTrackingEvent<'_>, (field kind), [player: player, entity: copy, uuid: copy, kind: cloned]);
    gen_listen!(living_breathe, none, horizon_client::mod_api::LivingBreatheEvent<'_>, (field kind), [entity: copy, kind: cloned, can_breathe: copy => mut_scalar, consume: copy => mut_scalar, refill: copy => mut_scalar]);
    gen_cancel!(entity_tick_pre, game_mut, horizon_client::mod_api::EntityTickPreEvent<'_>, (field kind), [dimension: cloned, entity: copy, kind: cloned]);
    gen_replace!(item_fluid_tank, game_ref, horizon_client::mod_api::ItemFluidTankEvent<'_>, (none), [item: cloned], capacity : value, Option<i32>);
    gen_collect!(player_render_listened, game_ref, horizon_client::mod_api::PlayerRenderListenedEvent, (none), [listened: copy => mut_scalar]);
    gen_cancel!(sign_changed, game_mut, horizon_client::mod_api::SignChangedEvent<'_>, (none), [player: player, dimension: cloned, pos: pos, front: copy, lines: str_seq => mut_strvec]);
    gen_collect!(standalone_models, none, horizon_client::mod_api::StandaloneModelsEvent<'_>, (none), [models: str_seq => mut_strvec]);
    gen_listen!(player_container_close, game_mut, horizon_client::mod_api::PlayerContainerCloseEvent<'_>, (none), [player: player, dimension: cloned, menu: cloned, pos: opt_pos]);
    gen_cancel!(player_toggle_flight, game_mut, horizon_client::mod_api::PlayerToggleFlightEvent<'_>, (none), [player: player, flying: copy]);
    gen_listen!(anvil_damaged, game_mut, horizon_client::mod_api::AnvilDamagedEvent<'_>, (none), [dimension: cloned, pos: pos, from: cloned, to: cloned]);
    gen_cancel!(arrow_loose, game_mut, horizon_client::mod_api::ArrowLooseEvent<'_>, (none), [stack: item, has_ammo: copy, charge: copy => mut_scalar]);
    gen_listen!(beacon_state_change, game_mut, horizon_client::mod_api::BeaconStateChangeEvent<'_>, (none), [dimension: cloned, pos: pos, active: copy]);
    gen_cancel!(bell_ring, level, horizon_client::mod_api::BellRingEvent, (none), [dimension: dim, player: opt_uuid, pos: pos_struct, block: block_at_pos]);
    gen_listen!(block_break_progress_update, game_mut, horizon_client::mod_api::BlockBreakProgressUpdateEvent<'_>, (none), [player: player, pos: pos, progress: copy]);
    gen_cancel!(block_burn, level, horizon_client::mod_api::BlockBurnEvent, (none), [dimension: dim, pos: pos_struct, block: block_at_pos]);
    gen_listen!(block_cook, game_ref, horizon_client::mod_api::BlockCookEvent<'_>, (none), [pos: pos, item: cloned, result: cloned]);
    gen_listen!(block_damage_abort, game_mut, horizon_client::mod_api::BlockDamageAbortEvent<'_>, (none), [player: player, pos: pos]);
    gen_listen!(block_dispense_armor, game_mut, horizon_client::mod_api::BlockDispenseArmorEvent<'_>, (none), [dimension: cloned, pos: pos, item: cloned, target: cloned]);
    gen_listen!(block_exp, level, horizon_client::mod_api::BlockExpEvent<'_>, (none), [dimension: dim, pos: pos_struct, block: cloned, experience: copy => mut_scalar]);
    gen_cancel!(block_fade, level, horizon_client::mod_api::BlockFadeEvent, (none), [dimension: dim, pos: pos_struct, from: state_level, to: state_level]);
    gen_listen!(block_failed_dispense, game_mut, horizon_client::mod_api::BlockFailedDispenseEvent<'_>, (none), [dimension: cloned, pos: pos, item: cloned]);
    gen_cancel!(block_form, level, horizon_client::mod_api::BlockFormEvent, (none), [dimension: dim, pos: pos_struct, from: state_level, to: state_level]);
    gen_cancel!(block_ignite, level, horizon_client::mod_api::BlockIgniteEvent<'_>, (none), [dimension: dim, player: opt_uuid, pos: pos_struct, cause: cloned]);
    gen_cancel!(block_lock_check, game_mut, horizon_client::mod_api::BlockLockCheckEvent<'_>, (none), [player: player, pos: pos]);
    gen_cancel!(block_receive_game, game_mut, horizon_client::mod_api::BlockReceiveGameEvent<'_>, (none), [dimension: cloned, pos: pos, block: cloned, event: cloned]);
    gen_listen!(block_shear_entity, game_mut, horizon_client::mod_api::BlockShearEntityEvent<'_>, (field kind), [dimension: cloned, pos: pos, entity: copy, kind: cloned]);
    gen_listen!(brewing_complete, game_ref, horizon_client::mod_api::BrewingCompleteEvent, (none), [pos: pos]);
    gen_listen!(brewing_fuel, game_ref, horizon_client::mod_api::BrewingFuelEvent<'_>, (none), [pos: pos, fuel: cloned]);
    gen_listen!(brewing_start, game_ref, horizon_client::mod_api::BrewingStartEvent<'_>, (none), [pos: pos, ingredient: cloned]);
    gen_listen!(campfire_start, game_ref, horizon_client::mod_api::CampfireStartEvent<'_>, (none), [pos: pos, item: cloned]);
    gen_cancel!(cauldron_level_changed, level, horizon_client::mod_api::CauldronLevelChangedEvent<'_>, (none), [dimension: dim, player: opt_uuid, pos: pos_struct, from_level: copy, to_level: copy, reason: cloned]);
    gen_cancel!(chorus_fruit_teleport, game_mut, horizon_client::mod_api::ChorusFruitTeleportEvent<'_>, (none), [player: player, from: vec3, target: mut_vec3 => mut_vec3]);
    gen_listen!(compost_item, game_ref, horizon_client::mod_api::CompostItemEvent<'_>, (none), [pos: pos, item: cloned]);
    gen_listen!(crafter_craft, game_ref, horizon_client::mod_api::CrafterCraftEvent<'_>, (none), [pos: pos, recipe: cloned]);
    gen_listen!(creeper_ignite, game_mut, horizon_client::mod_api::CreeperIgniteEvent<'_>, (entity entity), [entity: copy, player: copy, item: cloned]);
    gen_listen!(creeper_power, game_mut, horizon_client::mod_api::CreeperPowerEvent, (entity entity), [entity: copy, lightning: vec3]);
    gen_listen!(critical_hit, game_mut, horizon_client::mod_api::CriticalHitEvent<'_>, (none), [target_kind: cloned, vanilla_critical: copy, critical: copy => mut_scalar, damage_multiplier: copy => mut_scalar]);
    gen_listen!(difficulty_changed, game_mut, horizon_client::mod_api::DifficultyChangedEvent<'_>, (none), [from: cloned, to: cloned]);
    gen_cancel!(dragon_egg_form, level, horizon_client::mod_api::DragonEggFormEvent, (none), [dimension: dim, pos: pos_struct]);
    gen_listen!(enchant_item, game_mut, horizon_client::mod_api::EnchantItemEvent<'_>, (none), [player: player, pos: pos, item: item, levels: copy]);
    gen_cancel!(ender_pearl_teleport, game_mut, horizon_client::mod_api::EnderPearlTeleportEvent<'_>, (none), [player: player, pearl: copy, from: vec3, target: mut_vec3 => mut_vec3, damage: copy => mut_scalar]);
    gen_listen!(entities_load, game_mut, horizon_client::mod_api::EntitiesLoadEvent<'_>, (none), [dimension: cloned, chunk: chunk, count: copy]);
    gen_listen!(entities_unload, game_mut, horizon_client::mod_api::EntitiesUnloadEvent<'_>, (none), [dimension: cloned, chunk: chunk, count: copy]);
    gen_listen!(entity_break_door, game_mut, horizon_client::mod_api::EntityBreakDoorEvent<'_>, (field kind), [entity: copy, kind: cloned, pos: pos]);
    gen_listen!(entity_breed, game_mut, horizon_client::mod_api::EntityBreedEvent, (none), [parent: copy, partner: copy, child: copy]);
    gen_listen!(entity_change_block, game_mut, horizon_client::mod_api::EntityChangeBlockEvent<'_>, (field kind), [entity: copy, kind: cloned, pos: pos, from: cloned, to: cloned]);
    gen_listen!(entity_drop_item, game_mut, horizon_client::mod_api::EntityDropItemEvent<'_>, (entity entity), [entity: copy, item: cloned, count: copy, pos: vec3]);
    gen_listen!(entity_enter_block, game_mut, horizon_client::mod_api::EntityEnterBlockEvent<'_>, (field kind), [entity: copy, kind: cloned, pos: pos, block: cloned]);
    gen_listen!(entity_enter_love_mode, game_mut, horizon_client::mod_api::EntityEnterLoveModeEvent<'_>, (field kind), [entity: copy, kind: cloned, player: opt_uuid]);
    gen_listen!(entity_portal, game_mut, horizon_client::mod_api::EntityPortalEvent<'_>, (entity entity), [dimension: cloned, entity: copy, pos: pos]);
    gen_listen!(entity_pose_change, game_mut, horizon_client::mod_api::EntityPoseChangeEvent<'_>, (none), [player: player, from: cloned, to: cloned]);
    gen_listen!(entity_shoot_bow, game_mut, horizon_client::mod_api::EntityShootBowEvent<'_>, (entity projectile), [player: player, projectile: copy, force: copy]);
    gen_listen!(entity_tame, game_mut, horizon_client::mod_api::EntityTameEvent<'_>, (field kind), [entity: copy, kind: cloned, owner: opt_uuid]);
    gen_listen!(entity_teleport, game_mut, horizon_client::mod_api::EntityTeleportEvent<'_>, (entity entity), [dimension: cloned, entity: copy, from: vec3, to: vec3]);
    gen_listen!(entity_toggle_glide, game_mut, horizon_client::mod_api::EntityToggleGlideEvent<'_>, (none), [player: player, gliding: copy]);
    gen_listen!(entity_transform, game_mut, horizon_client::mod_api::EntityTransformEvent<'_>, (field kind), [entity: copy, kind: cloned, transformed: copy, transformed_kind: cloned]);
    gen_listen!(entity_unleash, game_mut, horizon_client::mod_api::EntityUnleashEvent<'_>, (field kind), [entity: copy, kind: cloned]);
    gen_listen!(exp_bottle, game_mut, horizon_client::mod_api::ExpBottleEvent, (none), [pos: vec3, experience: copy]);
    gen_collect!(experience_orb_pickup_listened, game_ref, horizon_client::mod_api::ExperienceOrbPickupListenedEvent, (none), [listened: copy => mut_scalar]);
    gen_cancel!(experience_orb_pickup, game_mut, horizon_client::mod_api::ExperienceOrbPickupEvent<'_>, (none), [player: copy, dimension: cloned, orb: copy, value: copy => mut_scalar, count: copy => mut_scalar]);
    gen_listen!(mending_repair, game_mut, horizon_client::mod_api::MendingRepairEvent<'_>, (none), [player: copy, stack: item, experience: copy, repair: copy => mut_scalar]);
    gen_listen!(explosion_prime, game_mut, horizon_client::mod_api::ExplosionPrimeEvent, (none), [center: vec3, radius: copy]);
    gen_replace!(smoke_source, level, horizon_client::mod_api::SmokeSourceEvent, (none), [dimension: dim, state: state_level], smoke : value, Option<bool>);
    gen_listen!(firework_explode, game_mut, horizon_client::mod_api::FireworkExplodeEvent, (entity entity), [entity: copy, pos: vec3, bursts: copy]);
    gen_cancel!(fluid_level_change, level, horizon_client::mod_api::FluidLevelChangeEvent<'_>, (none), [dimension: dim, pos: pos_struct, fluid: cloned, from: copy, to: copy]);
    gen_listen!(framerate_limit, game_ref, horizon_client::mod_api::FramerateLimitEvent, (none), [focused: copy, minimized: copy, in_world: copy, limit: copy => mut_scalar]);
    gen_listen!(furnace_burn, game_ref, horizon_client::mod_api::FurnaceBurnEvent<'_>, (none), [pos: pos, fuel: cloned]);
    gen_listen!(furnace_extract, game_mut, horizon_client::mod_api::FurnaceExtractEvent<'_>, (none), [player: player, pos: pos, item: cloned, count: copy]);
    gen_listen!(furnace_smelt, game_ref, horizon_client::mod_api::FurnaceSmeltEvent<'_>, (none), [pos: pos, item: cloned, result: cloned]);
    gen_listen!(furnace_start_smelt, game_ref, horizon_client::mod_api::FurnaceStartSmeltEvent<'_>, (none), [pos: pos, item: cloned]);
    gen_listen!(game_event, game_mut, horizon_client::mod_api::GameEventEvent<'_>, (none), [event: cloned, pos: vec3]);
    gen_listen!(hopper_inventory_search, game_ref, horizon_client::mod_api::HopperInventorySearchEvent, (opt_entity entity), [hopper: pos, container: opt_pos, entity: copy]);
    gen_cancel!(hopper_pickup_item, level, horizon_client::mod_api::HopperPickupItemEvent<'_>, (none), [dimension: dim, pos: pos_struct, item: cloned, count: copy]);
    gen_cancel!(inventory_block_start, game_mut, horizon_client::mod_api::InventoryBlockStartEvent<'_>, (none), [player: player, pos: pos]);
    gen_cancel!(inventory_move_item, game_ref, horizon_client::mod_api::InventoryMoveItemEvent<'_>, (none), [source: pos, destination: pos, item: cloned]);
    gen_listen!(item_despawn, game_mut, horizon_client::mod_api::ItemDespawnEvent<'_>, (entity entity), [entity: copy, item: cloned, count: copy, pos: vec3]);
    gen_listen!(item_expire, game_mut, horizon_client::mod_api::ItemExpireEvent<'_>, (entity entity), [entity: copy, stack: item, extra_life: copy => mut_scalar]);
    gen_listen!(item_spawn, game_mut, horizon_client::mod_api::ItemSpawnEvent<'_>, (entity entity), [entity: copy, item: cloned, count: copy, pos: vec3]);
    gen_cancel!(leaves_decay, level, horizon_client::mod_api::LeavesDecayEvent, (none), [dimension: dim, pos: pos_struct]);
    gen_cancel!(lightning_strike, game_mut, horizon_client::mod_api::LightningStrikeEvent<'_>, (none), [player: opt_uuid, pos: pos, cause: cloned]);
    gen_cancel!(moisture_change, level, horizon_client::mod_api::MoistureChangeEvent, (none), [dimension: dim, pos: pos_struct, from: copy, to: copy]);
    gen_cancel!(armor_stand_manipulate, game_mut, horizon_client::mod_api::ArmorStandManipulateEvent<'_>, (none), [player: player, stand_id: copy, slot: cloned]);
    gen_cancel!(bucket_empty, game_mut, horizon_client::mod_api::BucketEmptyEvent<'_>, (none), [player: player, pos: pos, fluid: cloned]);
    gen_cancel!(container_opened, game_mut, horizon_client::mod_api::ContainerOpenedEvent<'_>, (none), [player: player, kind: cloned, pos: opt_pos]);
    gen_cancel!(creative_slot, game_mut, horizon_client::mod_api::CreativeSlotEvent<'_>, (none), [player: player, slot: copy, item: cloned, count: copy]);
    gen_cancel!(food_level_changed, game_mut, horizon_client::mod_api::FoodLevelChangedEvent<'_>, (none), [player: player, from: copy, to: copy => mut_scalar]);
    gen_cancel!(hanging_break, game_mut, horizon_client::mod_api::HangingBreakEvent<'_>, (field kind), [player: opt_uuid, kind: cloned, pos: pos]);
    gen_cancel!(hanging_place, game_mut, horizon_client::mod_api::HangingPlaceEvent<'_>, (field kind), [player: player, kind: cloned, pos: pos]);
    gen_cancel!(item_damaged, game_mut, horizon_client::mod_api::ItemDamagedEvent<'_>, (none), [player: player, item: cloned, amount: copy => mut_scalar]);
    gen_listen!(player_clone, game_mut, horizon_client::mod_api::PlayerCloneEvent<'_>, (none), [player: player, was_death: copy]);
    gen_collect!(player_commands_sent, game_mut, horizon_client::mod_api::PlayerCommandsSentEvent<'_>, (none), [player: player, names: str_seq => mut_strvec]);
    gen_listen!(player_destroy_item, game_mut, horizon_client::mod_api::PlayerDestroyItemEvent<'_>, (none), [player: player, item: cloned, slot: cloned]);
    gen_cancel!(player_fish, game_mut, horizon_client::mod_api::PlayerFishEvent<'_>, (none), [player: player, state: copy]);
    gen_listen!(player_flower_pot_manipulate, game_mut, horizon_client::mod_api::PlayerFlowerPotManipulateEvent<'_>, (none), [player: player, pos: pos, plant: cloned]);
    gen_cancel!(player_game_mode_changed, game_mut, horizon_client::mod_api::PlayerGameModeChangedEvent<'_>, (none), [player: player, from: copy, to: copy]);
    gen_listen!(player_insert_lectern_book, game_mut, horizon_client::mod_api::PlayerInsertLecternBookEvent<'_>, (none), [player: player, pos: pos, book: cloned]);
    gen_listen!(player_item_frame_change, game_mut, horizon_client::mod_api::PlayerItemFrameChangeEvent<'_>, (entity entity_id), [player: player, entity_id: copy, action: cloned, item: cloned, rotation: copy]);
    gen_listen!(player_item_mend, game_mut, horizon_client::mod_api::PlayerItemMendEvent<'_>, (none), [player: player, item: cloned, repair: copy, experience: copy]);
    gen_listen!(player_lectern_page_change, game_mut, horizon_client::mod_api::PlayerLecternPageChangeEvent<'_>, (none), [player: player, pos: pos, page: copy]);
    gen_listen!(player_sprint, game_mut, horizon_client::mod_api::PlayerSprintEvent<'_>, (none), [player: player, sprinting: copy]);
    gen_cancel!(player_swap_hands, game_mut, horizon_client::mod_api::PlayerSwapHandsEvent<'_>, (none), [player: player]);
    gen_listen!(player_take_lectern_book, game_mut, horizon_client::mod_api::PlayerTakeLecternBookEvent<'_>, (none), [player: player, pos: pos, book: cloned]);
    gen_listen!(player_teleported, game_mut, horizon_client::mod_api::PlayerTeleportedEvent<'_>, (none), [player: player, from: vec3, to: vec3, dimension: cloned]);
    gen_listen!(player_totem, game_mut, horizon_client::mod_api::PlayerTotemEvent<'_>, (none), [player: player]);
    gen_listen!(resource_pack_status, game_mut, horizon_client::mod_api::ResourcePackStatusEvent<'_>, (none), [player: player, status: copy]);
    gen_listen!(spawn_change, game_mut, horizon_client::mod_api::SpawnChangeEvent, (none), [pos: pos, angle: pair]);
    gen_cancel!(tnt_primed, game_mut, horizon_client::mod_api::TntPrimedEvent, (none), [player: opt_uuid, pos: pos]);
    gen_listen!(vehicle_create, game_mut, horizon_client::mod_api::VehicleCreateEvent<'_>, (field kind), [kind: cloned, pos: vec3]);
    gen_cancel!(vehicle_damage, game_mut, horizon_client::mod_api::VehicleDamageEvent<'_>, (field kind), [kind: cloned, damage: copy => mut_scalar]);
    gen_listen!(vehicle_destroy, game_mut, horizon_client::mod_api::VehicleDestroyEvent<'_>, (field kind), [kind: cloned, pos: vec3]);
    gen_listen!(vehicle_moved, game_mut, horizon_client::mod_api::VehicleMovedEvent<'_>, (field kind), [player: opt_uuid, kind: cloned, from: vec3, to: vec3]);
    gen_listen!(world_border_bounds_changed, game_mut, horizon_client::mod_api::WorldBorderBoundsChangedEvent, (none), []);
    gen_listen!(world_border_center_changed, game_mut, horizon_client::mod_api::WorldBorderCenterChangedEvent, (none), []);
    gen_listen!(world_game_rule_change, game_mut, horizon_client::mod_api::WorldGameRuleChangeEvent<'_>, (none), [player: player, rule: cloned, value: cloned]);
    gen_cancel!(note_play, level, horizon_client::mod_api::NotePlayEvent<'_>, (none), [dimension: dim, player: opt_uuid, pos: pos_struct, instrument: cloned, note: copy]);
    gen_listen!(player_advancement_criterion_grant, game_mut, horizon_client::mod_api::PlayerAdvancementCriterionGrantEvent<'_>, (none), [player: player, advancement: cloned, criterion: cloned]);
    gen_listen!(player_advancement_done, game_mut, horizon_client::mod_api::PlayerAdvancementDoneEvent<'_>, (none), [player: player, advancement: cloned]);
    gen_listen!(player_advancement_progress, game_mut, horizon_client::mod_api::PlayerAdvancementProgressEvent<'_>, (none), [player: player, advancement: cloned, criterion: cloned, granted: copy]);
    gen_listen!(player_arm_swing, game_mut, horizon_client::mod_api::PlayerArmSwingEvent<'_>, (none), [player: player, hand: cloned]);
    gen_listen!(player_attack_cooldown_reset, game_mut, horizon_client::mod_api::PlayerAttackCooldownResetEvent<'_>, (none), [player: player]);
    gen_listen!(player_bed_leave, game_mut, horizon_client::mod_api::PlayerBedLeaveEvent<'_>, (none), [player: player, pos: pos]);
    gen_listen!(player_bucket_entity, game_mut, horizon_client::mod_api::PlayerBucketEntityEvent<'_>, (entity entity_id), [player: player, entity_id: copy, entity: cloned]);
    gen_listen!(player_bundle_item_select, game_mut, horizon_client::mod_api::PlayerBundleItemSelectEvent<'_>, (none), [player: player, slot: copy, index: copy]);
    gen_listen!(player_change_beacon_effect, game_mut, horizon_client::mod_api::PlayerChangeBeaconEffectEvent<'_>, (none), [player: player, pos: pos, primary: cloned, secondary: cloned]);
    gen_listen!(player_change_main_hand, game_mut, horizon_client::mod_api::PlayerChangeMainHandEvent<'_>, (none), [player: player, left: copy]);
    gen_listen!(player_client_loaded_world, game_mut, horizon_client::mod_api::PlayerClientLoadedWorldEvent<'_>, (none), [player: player]);
    gen_listen!(player_deep_sleep, game_mut, horizon_client::mod_api::PlayerDeepSleepEvent<'_>, (none), [player: player]);
    gen_cancel!(player_edit_book, game_mut, horizon_client::mod_api::PlayerEditBookEvent<'_>, (none), [player: player, pages: copy, signing: copy]);
    gen_listen!(player_egg_throw, game_mut, horizon_client::mod_api::PlayerEggThrowEvent<'_>, (none), [player: player]);
    gen_listen!(player_elytra_boost, game_mut, horizon_client::mod_api::PlayerElytraBoostEvent<'_>, (none), [player: player]);
    gen_listen!(player_input, game_mut, horizon_client::mod_api::PlayerInputEvent<'_>, (none), [player: player, flags: copy]);
    gen_cancel!(player_item_held, game_mut, horizon_client::mod_api::PlayerItemHeldEvent<'_>, (none), [player: player, from_slot: copy, to_slot: copy]);
    gen_listen!(player_jump, game_mut, horizon_client::mod_api::PlayerJumpEvent<'_>, (none), [player: player]);
    gen_listen!(player_kicked, game_mut, horizon_client::mod_api::PlayerKickedEvent<'_>, (none), [player: player, reason: cloned]);
    gen_listen!(player_launch_projectile, game_mut, horizon_client::mod_api::PlayerLaunchProjectileEvent<'_>, (none), [player: player, item: cloned]);
    gen_listen!(player_level_change, game_mut, horizon_client::mod_api::PlayerLevelChangeEvent<'_>, (none), [player: player, from: copy, to: copy]);
    gen_listen!(player_locale_change, game_mut, horizon_client::mod_api::PlayerLocaleChangeEvent<'_>, (none), [player: player, locale: cloned]);
    gen_listen!(player_loom_pattern_select, game_mut, horizon_client::mod_api::PlayerLoomPatternSelectEvent<'_>, (none), [player: player, pattern: copy]);
    gen_listen!(player_map_filled, game_mut, horizon_client::mod_api::PlayerMapFilledEvent<'_>, (none), [player: player, map_id: copy]);
    gen_listen!(player_name_entity, game_mut, horizon_client::mod_api::PlayerNameEntityEvent<'_>, (entity entity_id), [player: player, entity_id: copy, entity_name: cloned]);
    gen_listen!(player_open_sign, game_mut, horizon_client::mod_api::PlayerOpenSignEvent<'_>, (none), [player: player, pos: pos, front: copy]);
    gen_cancel!(player_pick_item, game_mut, horizon_client::mod_api::PlayerPickItemEvent<'_>, (entity entity_id), [player: player, from_entity: copy, pos: pos, entity_id: copy]);
    gen_listen!(player_pickup_arrow, game_mut, horizon_client::mod_api::PlayerPickupArrowEvent<'_>, (entity entity_id), [player: player, entity_id: copy, item: cloned]);
    gen_cancel!(player_portal, game_mut, horizon_client::mod_api::PlayerPortalEvent<'_>, (none), [player: player, from_dimension: cloned, to_dimension: cloned, cause: cloned]);
    gen_listen!(player_ready_arrow, game_mut, horizon_client::mod_api::PlayerReadyArrowEvent<'_>, (none), [player: player, arrow: cloned]);
    gen_listen!(player_recipe_book_click, game_mut, horizon_client::mod_api::PlayerRecipeBookClickEvent<'_>, (none), [player: player, recipe: cloned, all: copy]);
    gen_listen!(player_recipe_book_settings_change, game_mut, horizon_client::mod_api::PlayerRecipeBookSettingsChangeEvent<'_>, (none), [player: player, book: copy, open: copy, filtering: copy]);
    gen_cancel!(player_recipe_discovered, game_mut, horizon_client::mod_api::PlayerRecipeDiscoveredEvent<'_>, (none), [player: player, recipe: cloned]);
    gen_listen!(player_riptide, game_mut, horizon_client::mod_api::PlayerRiptideEvent<'_>, (none), [player: player]);
    gen_cancel!(player_shear_entity, game_mut, horizon_client::mod_api::PlayerShearEntityEvent<'_>, (entity entity_id), [player: player, entity_id: copy, off_hand: copy]);
    gen_cancel!(player_spawn_point_set, game_mut, horizon_client::mod_api::PlayerSpawnPointSetEvent<'_>, (none), [player: player, dimension: cloned, pos: pos, cause: cloned]);
    gen_cancel!(player_stat_award, game_mut, horizon_client::mod_api::PlayerStatAwardEvent<'_>, (none), [player: player, stat_type: cloned, stat: cloned, value: copy]);
    gen_listen!(player_stonecutter_recipe_select, game_mut, horizon_client::mod_api::PlayerStonecutterRecipeSelectEvent<'_>, (none), [player: player, recipe: copy]);
    gen_listen!(player_stop_spectating_entity, game_mut, horizon_client::mod_api::PlayerStopSpectatingEntityEvent<'_>, (entity entity_id), [player: player, entity_id: copy]);
    gen_listen!(player_stop_using_item, game_mut, horizon_client::mod_api::PlayerStopUsingItemEvent<'_>, (none), [player: player]);
    gen_listen!(player_swap_with_equipment_slot, game_mut, horizon_client::mod_api::PlayerSwapWithEquipmentSlotEvent<'_>, (none), [player: player, slot: cloned]);
    gen_cancel!(player_teleport_end_gateway, game_mut, horizon_client::mod_api::PlayerTeleportEndGatewayEvent<'_>, (none), [player: player, gateway: pos]);
    gen_listen!(player_toggle_entity_age_lock, game_mut, horizon_client::mod_api::PlayerToggleEntityAgeLockEvent<'_>, (entity entity_id), [player: player, entity_id: copy, locked: copy]);
    gen_listen!(player_toggle_sneak, game_mut, horizon_client::mod_api::PlayerToggleSneakEvent<'_>, (none), [player: player, sneaking: copy]);
    gen_listen!(player_trade, game_mut, horizon_client::mod_api::PlayerTradeEvent<'_>, (entity entity_id), [player: player, entity_id: copy]);
    gen_listen!(player_unleash_entity, game_mut, horizon_client::mod_api::PlayerUnleashEntityEvent<'_>, (entity entity_id), [player: player, entity_id: copy]);
    gen_listen!(player_velocity, game_mut, horizon_client::mod_api::PlayerVelocityEvent<'_>, (none), [player: player, velocity: vec3]);
    gen_listen!(portal_create, game_mut, horizon_client::mod_api::PortalCreateEvent<'_>, (none), [dimension: cloned, pos: pos]);
    gen_listen!(potion_splash, game_mut, horizon_client::mod_api::PotionSplashEvent<'_>, (none), [item: cloned, pos: vec3]);
    gen_replace!(prepare_anvil, game_mut, horizon_client::mod_api::PrepareAnvilEvent<'_>, (none), [player: player, pos: pos, cost: copy => mut_scalar], result : item, Option<ItemStack>);
    gen_replace!(prepare_grindstone, game_mut, horizon_client::mod_api::PrepareGrindstoneEvent<'_>, (none), [player: player, pos: pos], result : item, Option<ItemStack>);
    gen_listen!(prepare_hopper_transfer, game_ref, horizon_client::mod_api::PrepareHopperTransferEvent, (none), [pos: pos]);
    gen_replace!(prepare_item_craft, game_mut, horizon_client::mod_api::PrepareItemCraftEvent<'_>, (none), [player: player, pos: opt_pos], result : item, Option<ItemStack>);
    gen_listen!(prepare_item_enchant, game_mut, horizon_client::mod_api::PrepareItemEnchantEvent<'_>, (none), [player: player, pos: pos, costs: mut_pos => mut_pos]);
    gen_replace!(prepare_smithing, game_mut, horizon_client::mod_api::PrepareSmithingEvent<'_>, (none), [player: player, pos: pos], result : item, Option<ItemStack>);
    gen_listen!(raid_spawn_wave, game_mut, horizon_client::mod_api::RaidSpawnWaveEvent, (none), [raid: copy, pos: pos, wave: copy, raiders: copy]);
    gen_listen!(raid_state_change, game_mut, horizon_client::mod_api::RaidStateChangeEvent<'_>, (none), [raid: copy, pos: pos, state: cloned]);
    gen_listen!(sculk_bloom, game_mut, horizon_client::mod_api::SculkBloomEvent<'_>, (none), [dimension: cloned, pos: pos]);
    gen_listen!(sleep_finished_time, game_mut, horizon_client::mod_api::SleepFinishedTimeEvent<'_>, (none), [dimension: cloned, current: copy, new_time: copy => mut_scalar]);
    gen_listen!(sponge_absorb, level, horizon_client::mod_api::SpongeAbsorbEvent, (none), [dimension: dim, pos: pos_struct]);
    gen_listen!(target_hit, game_mut, horizon_client::mod_api::TargetHitEvent<'_>, (none), [dimension: cloned, pos: pos, strength: copy]);
    gen_cancel!(thunder_change, game_mut, horizon_client::mod_api::ThunderChangeEvent<'_>, (none), [dimension: cloned, thundering: copy]);
    gen_cancel!(time_skip, game_mut, horizon_client::mod_api::TimeSkipEvent<'_>, (none), [dimension: cloned, from: copy, to: copy, cause: cloned]);
    gen_listen!(trade_select, game_mut, horizon_client::mod_api::TradeSelectEvent<'_>, (none), [player: player, villager: copy, index: copy]);
    gen_listen!(vault_change_state, game_mut, horizon_client::mod_api::VaultChangeStateEvent<'_>, (none), [dimension: cloned, pos: pos, state: cloned]);
    gen_listen!(vault_loot, game_ref, horizon_client::mod_api::VaultLootEvent<'_>, (none), [pos: pos, item: cloned]);
    gen_cancel!(weather_change, game_mut, horizon_client::mod_api::WeatherChangeEvent<'_>, (none), [dimension: cloned, raining: copy, thundering: copy]);
    gen_listen!(whitelist_state_update, game_mut, horizon_client::mod_api::WhitelistStateUpdateEvent, (none), [enabled: copy]);
    gen_listen!(world_border_bounds_change_finish, game_mut, horizon_client::mod_api::WorldBorderBoundsChangeFinishEvent, (none), []);
    gen_listen!(world_saved, game_mut, horizon_client::mod_api::WorldSavedEvent, (none), [wait: copy]);
    gen_cancel!(living_drown, none, horizon_client::mod_api::LivingDrownEvent<'_>, (field kind), [entity: copy, kind: cloned, is_drowning: copy => mut_scalar, damage_amount: copy => mut_scalar, bubble_count: copy]);
    gen_listen!(living_jump, none, horizon_client::mod_api::LivingJumpEvent<'_>, (field kind), [entity: copy, kind: cloned]);
    gen_cancel!(baby_entity_spawn, game_mut, horizon_client::mod_api::BabyEntitySpawnEvent<'_>, (field kind), [parent: copy, partner: copy, player: copy, kind: cloned]);
    gen_listen!(can_player_sleep, game_mut, horizon_client::mod_api::CanPlayerSleepEvent<'_>, (none), [player: player, pos: pos, problem: mut_opt_string => mut_opt_string]);
    gen_replace!(item_container, game_ref, horizon_client::mod_api::ItemContainerEvent<'_>, (none), [item: cloned], slots : value, Option<u32>);
    gen_listen!(living_conversion_post, game_mut, horizon_client::mod_api::LivingConversionPostEvent<'_>, (field kind), [entity: copy, kind: cloned, converted: copy, converted_kind: cloned]);
    gen_cancel!(living_conversion_pre, game_mut, horizon_client::mod_api::LivingConversionPreEvent<'_>, (field kind), [entity: copy, kind: cloned, outcome: cloned]);
    gen_cancel!(living_death, game_mut, horizon_client::mod_api::LivingDeathEvent<'_>, (field kind), [entity: copy, kind: cloned, damage_type: cloned, attacker: copy]);
    gen_cancel!(living_knockback_v2, none, horizon_client::mod_api::LivingKnockbackV2Event<'_>, (field kind), [entity: copy, kind: cloned, attacker: copy, attacker_main: opt_item, attacker_off: opt_item, strength: copy => mut_scalar, x: copy => mut_scalar, z: copy => mut_scalar]);
    gen_cancel!(mob_split, game_mut, horizon_client::mod_api::MobSplitEvent<'_>, (field kind), [parent: copy, kind: cloned, size: copy, child_size: copy, count: copy => mut_scalar]);
    gen_listen!(movement_input, game_ref, horizon_client::mod_api::MovementInputEvent, (none), [forward: copy => mut_scalar, strafe: copy => mut_scalar, jump: copy => mut_scalar, sneak: copy => mut_scalar]);
    gen_cancel!(village_siege, game_mut, horizon_client::mod_api::VillageSiegeEvent<'_>, (none), [dimension: cloned, player: copy, attempted_pos: vec3]);
    gen_cancel!(player_xp_change, game_mut, horizon_client::mod_api::PlayerXpChangeEvent<'_>, (none), [player: player, amount: copy => mut_scalar]);
    gen_cancel!(render_item_in_frame, game_ref, horizon_client::mod_api::RenderItemInFrameEvent<'_>, (entity entity_id), [entity_id: copy, pos: pos, stack: item]);
    gen_listen!(render_name_tag, game_ref, horizon_client::mod_api::RenderNameTagEvent<'_>, (field kind), [entity_id: copy, kind: cloned, vanilla_visible: copy, visible: copy => mut_scalar, text: mut_opt_string => mut_opt_string]);
    gen_listen!(tooltip_color, game_ref, horizon_client::mod_api::TooltipColorEvent<'_>, (none), [stack: opt_item, background: copy => mut_scalar, border_top: copy => mut_scalar, border_bottom: copy => mut_scalar]);
    gen_listen!(camera_pose, game_ref, horizon_client::mod_api::CameraPoseEvent<'_>, (none), [partial: copy, position: mut_vec3 => mut_vec3, yaw: copy => mut_scalar, pitch: copy => mut_scalar]);
    gen_listen!(client_start, game_mut, horizon_client::mod_api::ClientStartEvent, (none), []);
    gen_listen!(client_disconnecting, game_ref, horizon_client::mod_api::ClientDisconnectingEvent, (none), []);
    gen_listen!(client_stopping, game_ref, horizon_client::mod_api::ClientStoppingEvent, (none), []);
    gen_listen!(menu_background, game_ref, horizon_client::mod_api::MenuBackgroundEvent<'_>, (none), [radius: copy_ref => mut_scalar_ref]);
    gen_cancel!(block_entity_accepts_fluid, game_ref, horizon_client::mod_api::BlockEntityAcceptsFluidEvent<'_>, (none), [block: cloned, tank: copy, fluid: cloned]);
    gen_cancel!(block_place, game_mut, horizon_client::mod_api::BlockPlaceEvent<'_>, (none), [player: player, stack: item, pos: pos, state: state_game]);
    gen_listen!(block_redstone, level, horizon_client::mod_api::BlockRedstoneEvent, (none), [dimension: dim, pos: pos_struct, old: copy, new: copy => mut_scalar]);
    gen_listen!(chat_suggestion_applied, game_ref, horizon_client::mod_api::ChatSuggestionAppliedEvent<'_>, (none), [original: cloned, start: copy, end: copy, suggestion: cloned, input: cloned]);
    gen_listen!(client_attack, game_mut, horizon_client::mod_api::ClientAttackEvent, (entity target), [target: copy, player: copy]);
    gen_listen!(client_hurt, game_mut, horizon_client::mod_api::ClientHurtEvent, (none), [attacker: copy, player: copy]);
    gen_listen!(client_player_network_logging_in, game_mut, horizon_client::mod_api::ClientPlayerNetworkLoggingInEvent, (none), []);
    gen_listen!(client_tick_pre, game_mut, horizon_client::mod_api::ClientTickPreEvent, (none), []);
    gen_listen!(client_tick_post, game_mut, horizon_client::mod_api::ClientTickPostEvent, (none), []);
    gen_cancel!(creative_slot_v2, game_mut, horizon_client::mod_api::CreativeSlotV2Event<'_>, (none), [player: player, slot: copy, stack: opt_item]);
    gen_listen!(custom_block_fluids, game_ref, horizon_client::mod_api::CustomBlockFluidsEvent<'_>, (none), [block: cloned, custom: copy => mut_scalar]);
    gen_cancel!(entity_change_block_v2, game_mut, horizon_client::mod_api::EntityChangeBlockV2Event<'_>, (field kind), [player: opt_uuid, entity: copy, kind: cloned, pos: pos, from: cloned, to: cloned]);
    gen_cancel!(entity_interact_specific, game_mut, horizon_client::mod_api::EntityInteractSpecificEvent<'_>, (field kind), [entity: copy, kind: cloned, stack: opt_item, location: vec3]);
    gen_listen!(entity_outline, game_ref, horizon_client::mod_api::EntityOutlineEvent<'_>, (field kind), [entity_id: copy, kind: cloned, outline: copy_ref => mut_scalar_ref]);
    gen_listen!(fishing_hook_landed, game_mut, horizon_client::mod_api::FishingHookLandedEvent<'_>, (none), [player: copy, hook: copy, dimension: cloned, block: pos, face: pos]);
    gen_cancel!(inventory_click, game_mut, horizon_client::mod_api::InventoryClickEvent<'_>, (none), [player: player, window: copy, slot: copy, button: copy, mode: copy]);
    gen_cancel!(inventory_drag, game_mut, horizon_client::mod_api::InventoryDragEvent<'_>, (none), [player: player, window: copy, kind: copy, slots: int_seq]);
    gen_cancel!(item_dropped_v2, game_mut, horizon_client::mod_api::ItemDroppedV2Event<'_>, (none), [player: copy, stack: item]);
    gen_replace!(item_interact_mob, game_mut, horizon_client::mod_api::ItemInteractMobEvent<'_>, (field kind), [entity: copy, kind: cloned, stack: opt_item], swing : value, Option<bool>);
    gen_cancel!(item_picked_up_v2, game_mut, horizon_client::mod_api::ItemPickedUpV2Event<'_>, (none), [player: copy, stack: item, thrower: opt_uuid, position: vec3]);
    gen_cancel!(living_change_target, none, horizon_client::mod_api::LivingChangeTargetEvent<'_>, (field kind), [dimension: cloned, entity: copy, kind: cloned, target: opt_uuid, target_mob: copy]);
    gen_listen!(living_damage_pre, none, horizon_client::mod_api::LivingDamagePreEvent<'_>, (field kind), [entity: copy, kind: cloned, damage_type: cloned, causing: copy, direct: copy, amount: copy => mut_scalar]);
    gen_listen!(living_damage_post, none, horizon_client::mod_api::LivingDamagePostEvent<'_>, (field kind), [entity: copy, kind: cloned, damage_type: cloned, causing: copy, direct: copy, amount: copy]);
    gen_cancel!(living_incoming_damage_v3, none, horizon_client::mod_api::LivingIncomingDamageV3Event<'_>, (field kind), [victim: copy, victim_uuid: opt_uuid, kind: cloned, attacker: copy, attacker_uuid: opt_uuid, direct: copy, damage_type: cloned, amount: copy => mut_scalar]);
    gen_listen!(log_line, none, horizon_client::mod_api::LogLineEvent<'_>, (none), [level: cloned, thread: cloned, message: cloned, line: cloned]);
    gen_replace!(permission_check, game_ref, horizon_client::mod_api::PermissionCheckEvent<'_>, (none), [player: copy, node: cloned, default_level: copy], result : value, Option<bool>);
    gen_listen!(player_register_channels, game_mut, horizon_client::mod_api::PlayerRegisterChannelsEvent<'_>, (none), [player: player, channels: str_seq, register: copy]);
    gen_listen!(player_command_level, game_mut, horizon_client::mod_api::PlayerCommandLevelEvent<'_>, (none), [player: player, command: cloned, level: copy => mut_scalar]);
    gen_collect!(player_commands_sent_v2, game_mut, horizon_client::mod_api::PlayerCommandsSentV2Event<'_>, (none), [player: player, names: str_seq => mut_strvec, names_hidden: str_seq => mut_strvec]);
    gen_cancel!(player_hurt_by_player, none, horizon_client::mod_api::PlayerHurtByPlayerEvent<'_>, (none), [attacker: copy, attacker_entity: copy, victim: copy, victim_entity: copy, source: cloned, damage_type: cloned, amount: copy => mut_scalar]);
    gen_cancel!(player_launch_projectile_v2, game_mut, horizon_client::mod_api::PlayerLaunchProjectileV2Event<'_>, (none), [player: player, stack: item]);
    gen_listen!(player_logging_out, game_mut, horizon_client::mod_api::PlayerLoggingOutEvent<'_>, (none), [player: player]);
    gen_cancel!(player_move, none, horizon_client::mod_api::PlayerMoveEvent<'_>, (none), [player: copy, name: cloned, from: vec3, to: mut_vec3 => mut_vec3, yaw: copy, pitch: copy, on_ground: copy]);
    gen_listen!(player_respawn_position, game_mut, horizon_client::mod_api::PlayerRespawnPositionEvent<'_>, (none), [player: player, dimension: mut_string => mut_string, pos: mut_vec3 => mut_vec3, yaw: copy => mut_scalar, pitch: copy => mut_scalar]);
    gen_listen!(player_spawn_location, game_mut, horizon_client::mod_api::PlayerSpawnLocationEvent<'_>, (none), [player: player, dimension: mut_string => mut_string, position: mut_vec3 => mut_vec3, rotation: mut_pair => mut_pair]);
    gen_cancel!(player_teleport, none, horizon_client::mod_api::PlayerTeleportEvent<'_>, (none), [player: copy, name: cloned, cause: cloned, from_dimension: cloned, from: vec3, to_dimension: cloned, to: mut_vec3 => mut_vec3, relative: copy => mut_scalar]);
    gen_listen!(player_tick_pre, game_mut, horizon_client::mod_api::PlayerTickPreEvent, (none), []);
    gen_listen!(projectile_world_border, none, horizon_client::mod_api::ProjectileWorldBorderEvent<'_>, (field kind), [dimension: cloned, entity: copy, kind: cloned, pos: mut_vec3 => mut_vec3, velocity: mut_vec3 => mut_vec3, collide: copy => mut_scalar]);
    gen_listen!(resource_pack_status_v2, game_mut, horizon_client::mod_api::ResourcePackStatusV2Event<'_>, (none), [player: player, pack: copy, status: copy]);
    gen_cancel!(screen_mouse_dragged_pre_v2, game_mut, horizon_client::mod_api::ScreenMouseDraggedPreV2Event<'_>, (none), [screen: cloned, mouse: pair, button: copy, delta: pair, modifiers: copy]);
    gen_cancel!(screen_mouse_scrolled_pre_v2, game_mut, horizon_client::mod_api::ScreenMouseScrolledPreV2Event<'_>, (none), [screen: cloned, mouse: pair, scroll: pair64, modifiers: copy]);
    gen_listen!(stateful_block_entity, game_ref, horizon_client::mod_api::StatefulBlockEntityEvent<'_>, (none), [block: cloned, stateful: copy => mut_scalar]);
    gen_cancel!(toast_add, game_mut, horizon_client::mod_api::ToastAddEvent<'_>, (none), [kind: cloned, id: cloned]);
    gen_cancel!(trample, none, horizon_client::mod_api::TrampleEvent<'_>, (field kind), [dimension: cloned, entity: copy, kind: cloned, pos: pos, fall_distance: copy]);
    gen_cancel!(vehicle_moved_v2, game_mut, horizon_client::mod_api::VehicleMovedV2Event<'_>, (field kind), [player: opt_uuid, dimension: cloned, vehicle: copy, kind: cloned, from: vec3, to: mut_vec3 => mut_vec3]);
    };
}

#[cfg(test)]
mod generated_tests {
    use super::test_support::*;
    use super::{GENERATED_HOOK_NAMES, GENERATED_KIND_HOOK_NAMES, HOOK_CLASSIFICATION};
    use horizon_client::mod_api::*;

    #[test]
    fn generated_classification_names_every_hook_once() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in HOOK_CLASSIFICATION {
            assert!(seen.insert(*name), "{name} classified twice");
        }
        for name in GENERATED_HOOK_NAMES {
            assert!(seen.contains(name), "{name} generated but not classified");
        }
        for name in GENERATED_KIND_HOOK_NAMES {
            assert!(
                GENERATED_HOOK_NAMES.contains(name),
                "{name} kind-filtered but not generated"
            );
        }
    }

    #[test]
    fn generated_tick() {
        let (m, id, _dir) = loaded("gen_tick", "horizon.on(\"tick\", function(event)\n\nend)");
        let mods = boxed(m);
        dispatch_tick(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_server_about_to_start() {
        let (m, id, _dir) = loaded(
            "gen_server_about_to_start",
            "horizon.on(\"server_about_to_start\", function(event)\n    assert(event.dedicated == true, \"dedicated\")\nend)",
        );
        let mods = boxed(m);
        dispatch_server_about_to_start(&mut FakeHost::empty(), &mods, true);
        no_error(&id);
    }

    #[test]
    fn generated_server_stopping() {
        let (m, id, _dir) = loaded(
            "gen_server_stopping",
            "horizon.on(\"server_stopping\", function(event)\n    assert(event.dedicated == true, \"dedicated\")\nend)",
        );
        let mods = boxed(m);
        dispatch_server_stopping(&mut FakeHost::empty(), &mods, true);
        no_error(&id);
    }

    #[test]
    fn generated_level_tick_post() {
        let (m, id, _dir) = loaded(
            "gen_level_tick_post",
            "horizon.on(\"level_tick_post\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\nend)",
        );
        let mods = boxed(m);
        dispatch_level_tick_post(&mut FakeHost::empty(), &mods, "test:dimension");
        no_error(&id);
    }

    #[test]
    fn generated_block_changed() {
        let (m, id, _dir) = loaded(
            "gen_block_changed",
            "horizon.on(\"block_changed\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.old == \"test:state10\", \"old\")\n    assert(event.new == \"test:state11\", \"new\")\nend)",
        );
        let mods = boxed(m);
        dispatch_block_changed(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            BlockStateId(10),
            BlockStateId(11),
        );
        no_error(&id);
    }

    #[test]
    fn generated_key() {
        let (m, id, _dir) = loaded(
            "gen_key",
            "horizon.on(\"key\", function(event)\n    assert(event.key == \"test:key\", \"key\")\n    assert(event.pressed == false, \"pressed\")\nend)",
        );
        let mods = boxed(m);
        dispatch_key(&mut FakeHost::empty(), &mods, "test:key", false);
        no_error(&id);
    }

    #[test]
    fn generated_wheel() {
        let (m, id, _dir) = loaded(
            "gen_wheel",
            "horizon.on(\"wheel\", function(event)\n    assert(event.steps == 1.5, \"steps\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut claimed = false;
        dispatch_wheel(&mut FakeHost::empty(), &mods, 1.5, &mut claimed);
        assert!(claimed, "claimed not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_settings() {
        let (m, id, _dir) = loaded(
            "gen_settings",
            "horizon.on(\"settings\", function(event)\n    assert(event.mod_id == \"test:mod_id\", \"mod_id\")\n    assert(event.buttons[1] == \"test:seed\", \"buttons\")\n    event.buttons[1] = \"test:edited\"; table.insert(event.buttons, \"test:added\")\nend)",
        );
        let mods = boxed(m);
        let mut buttons = vec!["test:seed".to_owned()];
        dispatch_settings(&mut FakeHost::empty(), &mods, "test:mod_id", &mut buttons);
        assert_eq!(
            buttons,
            vec!["test:edited".to_owned(), "test:added".to_owned()]
        );
        no_error(&id);
    }

    #[test]
    fn generated_settings_click() {
        let (m, id, _dir) = loaded(
            "gen_settings_click",
            "horizon.on(\"settings_click\", function(event)\n    assert(event.mod_id == \"test:mod_id\", \"mod_id\")\n    assert(event.button == 8, \"button\")\nend)",
        );
        let mods = boxed(m);
        dispatch_settings_click(&mut FakeHost::empty(), &mods, "test:mod_id", 8);
        no_error(&id);
    }

    #[test]
    fn generated_menu_clicked() {
        let (m, id, _dir) = loaded(
            "gen_menu_clicked",
            "horizon.on(\"menu_clicked\", function(event)\n    assert(event.uuid == 7, \"uuid\")\n    assert(event.window == 8, \"window\")\n    assert(event.slot == 9, \"slot\")\n    assert(event.button == 10, \"button\")\n    assert(event.mode == 11, \"mode\")\nend)",
        );
        let mods = boxed(m);
        dispatch_menu_clicked(&mut FakeHost::empty(), &mods, 7, 8, 9, 10, 11);
        no_error(&id);
    }

    #[test]
    fn generated_menu_closed() {
        let (m, id, _dir) = loaded(
            "gen_menu_closed",
            "horizon.on(\"menu_closed\", function(event)\n    assert(event.uuid == 7, \"uuid\")\n    assert(event.window == 8, \"window\")\nend)",
        );
        let mods = boxed(m);
        dispatch_menu_closed(&mut FakeHost::empty(), &mods, 7, 8);
        no_error(&id);
    }

    #[test]
    fn generated_block_spread() {
        let (m, id, _dir) = loaded(
            "gen_block_spread",
            "horizon.on(\"block_spread\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.kind == \"test:kind\", \"kind\")\n    assert(event.from.x == 3, \"from\")\n    assert(event.from.y == 4, \"from\")\n    assert(event.from.z == 5, \"from\")\n    assert(event.to.x == 4, \"to\")\n    assert(event.to.y == 5, \"to\")\n    assert(event.to.z == 6, \"to\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_block_spread(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            "test:kind",
            BlockPos::new(3, 4, 5),
            BlockPos::new(4, 5, 6),
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_fill_bucket() {
        let (m, id, _dir) = loaded(
            "gen_fill_bucket",
            "horizon.on(\"fill_bucket\", function(event)\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_fill_bucket(
            &mut FakeHost::empty(),
            &mods,
            &ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
            [2, 3, 4],
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_on_equip() {
        let (m, id, _dir) = loaded(
            "gen_on_equip",
            "horizon.on(\"on_equip\", function(event)\n    assert(event.slot == \"test:slot\", \"slot\")\n    assert(event.new.id == \"test:new\", \"new\")\nend)",
        );
        let mods = boxed(m);
        dispatch_on_equip(
            &mut FakeHost::empty(),
            &mods,
            "test:slot",
            Some(&ItemStack {
                item: "test:new".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
        );
        no_error(&id);
    }

    #[test]
    fn generated_is_signal_source() {
        let (m, id, _dir) = loaded(
            "gen_is_signal_source",
            "horizon.on(\"is_signal_source\", function(event)\n    assert(event.state == \"test:state8\", \"state\")\n    assert(event.source == false, \"source\")\n    event.source = true\nend)",
        );
        let mods = boxed(m);
        let mut source = false;
        dispatch_is_signal_source(&FakeLevel, &mods, BlockStateId(8), &mut source);
        assert!(source);
        no_error(&id);
    }

    #[test]
    fn generated_left_click_block() {
        let (m, id, _dir) = loaded(
            "gen_left_click_block",
            "horizon.on(\"left_click_block\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.face.x == 2, \"face\")\n    assert(event.face.y == 3, \"face\")\n    assert(event.face.z == 4, \"face\")\n    assert(event.state == \"test:state10\", \"state\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_left_click_block(
            &mut FakeHost::empty(),
            &mods,
            [1, 2, 3],
            [2, 3, 4],
            BlockStateId(10),
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_entity_leave_level() {
        let (m, id, _dir) = loaded(
            "gen_entity_leave_level",
            "horizon.on(\"entity_leave_level\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_leave_level(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                "minecraft:skeleton",
            );
        }
        no_error(&id);
        dispatch_entity_leave_level(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            "minecraft:zombie",
        );
        no_error(&id);
    }

    #[test]
    fn generated_should_apply_effect_tick_this_tick() {
        let (m, id, _dir) = loaded(
            "gen_should_apply_effect_tick_this_tick",
            "horizon.on(\"should_apply_effect_tick_this_tick\", function(event)\n    assert(event.effect == \"test:effect\", \"effect\")\n    assert(event.tick_count == 8, \"tick_count\")\n    assert(event.amplifier == 9, \"amplifier\")\n    assert(event.apply == false, \"apply\")\n    event.apply = true\nend)",
        );
        let mods = boxed(m);
        let mut apply = false;
        dispatch_should_apply_effect_tick_this_tick(
            &FakeHost::empty(),
            &mods,
            "test:effect",
            8,
            9,
            &mut apply,
        );
        assert!(apply);
        no_error(&id);
    }

    #[test]
    fn generated_apply_effect_tick() {
        let (m, id, _dir) = loaded(
            "gen_apply_effect_tick",
            "horizon.on(\"apply_effect_tick\", function(event)\n    assert(event.effect == \"test:effect\", \"effect\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.amplifier == 10, \"amplifier\")\n    assert(event.keep == true, \"keep\")\n    event.keep = false\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut keep = true;
            dispatch_apply_effect_tick(
                &FakeHost::empty(),
                &mods,
                "test:effect",
                "test:dimension",
                Some(13),
                10,
                &mut keep,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            let mut keep = true;
            dispatch_apply_effect_tick(
                &FakeHost::empty(),
                &mods,
                "test:effect",
                "test:dimension",
                None,
                10,
                &mut keep,
            );
        }
        no_error(&id);
        let mut keep = true;
        dispatch_apply_effect_tick(
            &FakeHost::empty(),
            &mods,
            "test:effect",
            "test:dimension",
            Some(11),
            10,
            &mut keep,
        );
        assert!(!keep);
        no_error(&id);
    }

    #[test]
    fn generated_on_effect_added() {
        let (m, id, _dir) = loaded(
            "gen_on_effect_added",
            "horizon.on(\"on_effect_added\", function(event)\n    assert(event.effect == \"test:effect\", \"effect\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.amplifier == 9, \"amplifier\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_on_effect_added(&mut FakeHost::empty(), &mods, "test:effect", Some(13), 9);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_on_effect_added(&mut FakeHost::empty(), &mods, "test:effect", None, 9);
        }
        no_error(&id);
        dispatch_on_effect_added(&mut FakeHost::empty(), &mods, "test:effect", Some(11), 9);
        no_error(&id);
    }

    #[test]
    fn generated_mob_effect_added() {
        let (m, id, _dir) = loaded(
            "gen_mob_effect_added",
            "horizon.on(\"mob_effect_added\", function(event)\n    assert(event.effect == \"test:effect\", \"effect\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.amplifier == 9, \"amplifier\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_mob_effect_added(&mut FakeHost::empty(), &mods, "test:effect", Some(13), 9);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_mob_effect_added(&mut FakeHost::empty(), &mods, "test:effect", None, 9);
        }
        no_error(&id);
        dispatch_mob_effect_added(&mut FakeHost::empty(), &mods, "test:effect", Some(11), 9);
        no_error(&id);
    }

    #[test]
    fn generated_chunk_unload() {
        let (m, id, _dir) = loaded(
            "gen_chunk_unload",
            "horizon.on(\"chunk_unload\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.chunk.x == 4, \"chunk\")\n    assert(event.chunk.z == -6, \"chunk\")\nend)",
        );
        let mods = boxed(m);
        dispatch_chunk_unload(&mut FakeHost::empty(), &mods, "test:dimension", [4, -6]);
        no_error(&id);
    }

    #[test]
    fn generated_has_analog_output() {
        let (m, id, _dir) = loaded(
            "gen_has_analog_output",
            "horizon.on(\"has_analog_output\", function(event)\n    assert(event.state == \"test:state8\", \"state\")\n    assert(event.output == false, \"output\")\n    event.output = true\nend)",
        );
        let mods = boxed(m);
        let mut output = false;
        dispatch_has_analog_output(&FakeLevel, &mods, BlockStateId(8), &mut output);
        assert!(output);
        no_error(&id);
    }

    #[test]
    fn generated_can_survive() {
        let (m, id, _dir) = loaded(
            "gen_can_survive",
            "horizon.on(\"can_survive\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.state == \"test:state10\", \"state\")\n    assert(event.survives == nil, \"replace seed\")\n    event.survives = true\nend)",
        );
        let mods = boxed(m);
        let mut survives: Option<bool> = None;
        dispatch_can_survive(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            BlockStateId(10),
            &mut survives,
        );
        assert_eq!(survives, Some(true));
        no_error(&id);
    }

    #[test]
    fn generated_may_place_on() {
        let (m, id, _dir) = loaded(
            "gen_may_place_on",
            "horizon.on(\"may_place_on\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.state == \"test:state9\", \"state\")\n    assert(event.ground == \"test:state10\", \"ground\")\n    assert(event.ground_pos.x == 4, \"ground_pos\")\n    assert(event.ground_pos.y == 5, \"ground_pos\")\n    assert(event.ground_pos.z == 6, \"ground_pos\")\n    assert(event.may == nil, \"replace seed\")\n    event.may = true\nend)",
        );
        let mods = boxed(m);
        let mut may: Option<bool> = None;
        dispatch_may_place_on(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockStateId(9),
            BlockStateId(10),
            BlockPos::new(4, 5, 6),
            &mut may,
        );
        assert_eq!(may, Some(true));
        no_error(&id);
    }

    #[test]
    fn generated_is_valid_bonemeal_target() {
        let (m, id, _dir) = loaded(
            "gen_is_valid_bonemeal_target",
            "horizon.on(\"is_valid_bonemeal_target\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.state == \"test:state10\", \"state\")\n    assert(event.valid == nil, \"replace seed\")\n    event.valid = true\nend)",
        );
        let mods = boxed(m);
        let mut valid: Option<bool> = None;
        dispatch_is_valid_bonemeal_target(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            BlockStateId(10),
            &mut valid,
        );
        assert_eq!(valid, Some(true));
        no_error(&id);
    }

    #[test]
    fn generated_client_resources_reloaded() {
        let (m, id, _dir) = loaded(
            "gen_client_resources_reloaded",
            "horizon.on(\"client_resources_reloaded\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_resources_reloaded(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_use_duration() {
        let (m, id, _dir) = loaded(
            "gen_use_duration",
            "horizon.on(\"use_duration\", function(event)\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.duration == 8, \"duration\")\n    event.duration = 51\nend)",
        );
        let mods = boxed(m);
        let mut duration = 8;
        dispatch_use_duration(
            &FakeHost::empty(),
            &mods,
            &ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
            &mut duration,
        );
        assert_eq!(duration, 51);
        no_error(&id);
    }

    #[test]
    fn generated_can_eat() {
        let (m, id, _dir) = loaded(
            "gen_can_eat",
            "horizon.on(\"can_eat\", function(event)\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.can_eat == false, \"can_eat\")\n    event.can_eat = true\nend)",
        );
        let mods = boxed(m);
        let mut can_eat = false;
        dispatch_can_eat(
            &FakeHost::empty(),
            &mods,
            &ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
            &mut can_eat,
        );
        assert!(can_eat);
        no_error(&id);
    }

    #[test]
    fn generated_perform_console_command() {
        let (m, id, _dir) = loaded(
            "gen_perform_console_command",
            "horizon.on(\"perform_console_command\", function(event)\n    assert(event.command == \"test:command\", \"command\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut ran = false;
        dispatch_perform_console_command(&mut FakeHost::empty(), &mods, "test:command", &mut ran);
        assert!(ran, "ran not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_server_data_loaded() {
        let (m, id, _dir) = loaded(
            "gen_server_data_loaded",
            "horizon.on(\"server_data_loaded\", function(event)\n    assert(event.reload == true, \"reload\")\nend)",
        );
        let mods = boxed(m);
        dispatch_server_data_loaded(&mut FakeHost::empty(), &mods, true);
        no_error(&id);
    }

    #[test]
    fn generated_compute_fov() {
        let (m, id, _dir) = loaded(
            "gen_compute_fov",
            "horizon.on(\"compute_fov\", function(event)\n    assert(event.partial == 1.5, \"partial\")\n    assert(event.camera.x == 2, \"camera\")\n    assert(event.camera.y == 2.0, \"camera\")\n    assert(event.camera.z == 3.0, \"camera\")\n    assert(event.degrees == 3.5, \"degrees\")\n    event.degrees = 17.5\nend)",
        );
        let mods = boxed(m);
        let mut degrees = 3.5;
        dispatch_compute_fov(
            &FakeHost::empty(),
            &mods,
            1.5,
            [2.0, 2.0, 3.0],
            &mut degrees,
        );
        assert_eq!(degrees, 17.5);
        no_error(&id);
    }

    #[test]
    fn generated_mouse_button_pre() {
        let (m, id, _dir) = loaded(
            "gen_mouse_button_pre",
            "horizon.on(\"mouse_button_pre\", function(event)\n    assert(event.button == 7, \"button\")\n    assert(event.pressed == false, \"pressed\")\n    assert(event.modifiers == 9, \"modifiers\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_mouse_button_pre(&mut FakeHost::empty(), &mods, 7, false, 9, &mut canceled);
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_mouse_button_post() {
        let (m, id, _dir) = loaded(
            "gen_mouse_button_post",
            "horizon.on(\"mouse_button_post\", function(event)\n    assert(event.button == 7, \"button\")\n    assert(event.pressed == false, \"pressed\")\n    assert(event.modifiers == 9, \"modifiers\")\nend)",
        );
        let mods = boxed(m);
        dispatch_mouse_button_post(&mut FakeHost::empty(), &mods, 7, false, 9);
        no_error(&id);
    }

    #[test]
    fn generated_recipes_received() {
        let (m, id, _dir) = loaded(
            "gen_recipes_received",
            "horizon.on(\"recipes_received\", function(event)\n    assert(event.recipe_types[1] == \"test:seed\", \"recipe_types\")\nend)",
        );
        let mods = boxed(m);
        dispatch_recipes_received(&mut FakeHost::empty(), &mods, &["test:seed".to_owned()]);
        no_error(&id);
    }

    #[test]
    fn generated_mod_container_viewers() {
        let (m, id, _dir) = loaded(
            "gen_mod_container_viewers",
            "horizon.on(\"mod_container_viewers\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.viewers == 9, \"viewers\")\n    assert(event.previous == 10, \"previous\")\nend)",
        );
        let mods = boxed(m);
        dispatch_mod_container_viewers(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            9,
            10,
        );
        no_error(&id);
    }

    #[test]
    fn generated_experience_orb_can_merge() {
        let (m, id, _dir) = loaded(
            "gen_experience_orb_can_merge",
            "horizon.on(\"experience_orb_can_merge\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.into == 8, \"into\")\n    assert(event.from == 9, \"from\")\n    assert(event.into_value == 10, \"into_value\")\n    assert(event.from_value == 11, \"from_value\")\n    assert(event.can_merge == false, \"can_merge\")\n    event.can_merge = true\nend)",
        );
        let mods = boxed(m);
        let mut can_merge = false;
        dispatch_experience_orb_can_merge(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            9,
            10,
            11,
            &mut can_merge,
        );
        assert!(can_merge);
        no_error(&id);
    }

    #[test]
    fn generated_experience_orb_merge() {
        let (m, id, _dir) = loaded(
            "gen_experience_orb_merge",
            "horizon.on(\"experience_orb_merge\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.into == 8, \"into\")\n    assert(event.from == 9, \"from\")\n    assert(event.from_value == 10, \"from_value\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_experience_orb_merge(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            9,
            10,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_menu_slot_rules() {
        let (m, id, _dir) = loaded(
            "gen_menu_slot_rules",
            "horizon.on(\"menu_slot_rules\", function(event)\n    assert(event.menu == \"test:menu\", \"menu\")\n    assert(event.slot == 8, \"slot\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.may_place == false, \"may_place\")\n    event.may_place = true\n    assert(event.max_stack == 11, \"max_stack\")\n    event.max_stack = 54\nend)",
        );
        let mods = boxed(m);
        let mut may_place = false;
        let mut max_stack = 11;
        dispatch_menu_slot_rules(
            &mods,
            "test:menu",
            8,
            &ItemStack {
                item: "test:stack".into(),
                count: 3,
                damage: 2,
                extra: None,
            },
            &mut may_place,
            &mut max_stack,
        );
        assert!(may_place);
        assert_eq!(max_stack, 54);
        no_error(&id);
    }

    #[test]
    fn generated_on_use_tick() {
        let (m, id, _dir) = loaded(
            "gen_on_use_tick",
            "horizon.on(\"on_use_tick\", function(event)\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.remaining == 9, \"remaining\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_on_use_tick(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                Some(13),
                9,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_on_use_tick(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                None,
                9,
            );
        }
        no_error(&id);
        dispatch_on_use_tick(
            &mut FakeHost::empty(),
            &mods,
            &ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
            Some(11),
            9,
        );
        no_error(&id);
    }

    #[test]
    fn generated_release_using() {
        let (m, id, _dir) = loaded(
            "gen_release_using",
            "horizon.on(\"release_using\", function(event)\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.time_left == 9, \"time_left\")\n    assert(event.used == false, \"used\")\n    event.used = true\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut used = false;
            dispatch_release_using(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                Some(13),
                9,
                &mut used,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            let mut used = false;
            dispatch_release_using(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                None,
                9,
                &mut used,
            );
        }
        no_error(&id);
        let mut used = false;
        dispatch_release_using(
            &mut FakeHost::empty(),
            &mods,
            &ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            },
            Some(11),
            9,
            &mut used,
        );
        assert!(used);
        no_error(&id);
    }

    #[test]
    fn generated_level_load() {
        let (m, id, _dir) = loaded(
            "gen_level_load",
            "horizon.on(\"level_load\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\nend)",
        );
        let mods = boxed(m);
        dispatch_level_load(&mut FakeHost::empty(), &mods, "test:dimension");
        no_error(&id);
    }

    #[test]
    fn generated_level_unload() {
        let (m, id, _dir) = loaded(
            "gen_level_unload",
            "horizon.on(\"level_unload\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\nend)",
        );
        let mods = boxed(m);
        dispatch_level_unload(&mut FakeHost::empty(), &mods, "test:dimension");
        no_error(&id);
    }

    #[test]
    fn generated_client_level_load() {
        let (m, id, _dir) = loaded(
            "gen_client_level_load",
            "horizon.on(\"client_level_load\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\nend)",
        );
        let mods = boxed(m);
        dispatch_client_level_load(&mut FakeHost::empty(), &mods, "test:dimension");
        no_error(&id);
    }

    #[test]
    fn generated_client_level_unload() {
        let (m, id, _dir) = loaded(
            "gen_client_level_unload",
            "horizon.on(\"client_level_unload\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\nend)",
        );
        let mods = boxed(m);
        dispatch_client_level_unload(&mut FakeHost::empty(), &mods, "test:dimension");
        no_error(&id);
    }

    #[test]
    fn generated_entity_mount() {
        let (m, id, _dir) = loaded(
            "gen_entity_mount",
            "horizon.on(\"entity_mount\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.vehicle == 10, \"vehicle\")\n    assert(event.vehicle_kind == \"test:vehicle_kind\", \"vehicle_kind\")\n    assert(event.mounting == false, \"mounting\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut canceled = false;
            dispatch_entity_mount(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                "minecraft:skeleton",
                10,
                "test:vehicle_kind",
                false,
                &mut canceled,
            );
        }
        no_error(&id);
        let mut canceled = false;
        dispatch_entity_mount(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            "minecraft:zombie",
            10,
            "test:vehicle_kind",
            false,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_start_tracking() {
        let (m, id, _dir) = loaded(
            "gen_player_start_tracking",
            "horizon.on(\"player_start_tracking\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity == 9, \"entity\")\n    assert(event.uuid == 10, \"uuid\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_start_tracking(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                "minecraft:skeleton",
            );
        }
        no_error(&id);
        dispatch_player_start_tracking(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            10,
            "minecraft:zombie",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_stop_tracking() {
        let (m, id, _dir) = loaded(
            "gen_player_stop_tracking",
            "horizon.on(\"player_stop_tracking\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity == 9, \"entity\")\n    assert(event.uuid == 10, \"uuid\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_stop_tracking(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                "minecraft:skeleton",
            );
        }
        no_error(&id);
        dispatch_player_stop_tracking(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            10,
            "minecraft:zombie",
        );
        no_error(&id);
    }

    #[test]
    fn generated_living_breathe() {
        let (m, id, _dir) = loaded(
            "gen_living_breathe",
            "horizon.on(\"living_breathe\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.can_breathe == true, \"can_breathe\")\n    event.can_breathe = false\n    assert(event.consume == 10, \"consume\")\n    event.consume = 53\n    assert(event.refill == 11, \"refill\")\n    event.refill = 54\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut can_breathe = true;
            let mut consume = 10;
            let mut refill = 11;
            dispatch_living_breathe(
                &mods,
                7,
                "minecraft:skeleton",
                &mut can_breathe,
                &mut consume,
                &mut refill,
            );
        }
        no_error(&id);
        let mut can_breathe = true;
        let mut consume = 10;
        let mut refill = 11;
        dispatch_living_breathe(
            &mods,
            7,
            "minecraft:zombie",
            &mut can_breathe,
            &mut consume,
            &mut refill,
        );
        assert!(!can_breathe);
        assert_eq!(consume, 53);
        assert_eq!(refill, 54);
        no_error(&id);
    }

    #[test]
    fn generated_entity_tick_pre() {
        let (m, id, _dir) = loaded(
            "gen_entity_tick_pre",
            "horizon.on(\"entity_tick_pre\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut canceled = false;
            dispatch_entity_tick_pre(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                "minecraft:skeleton",
                &mut canceled,
            );
        }
        no_error(&id);
        let mut canceled = false;
        dispatch_entity_tick_pre(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            "minecraft:zombie",
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_render_listened() {
        let (m, id, _dir) = loaded(
            "gen_player_render_listened",
            "horizon.on(\"player_render_listened\", function(event)\n    assert(event.listened == true, \"listened\")\n    event.listened = false\nend)",
        );
        let mods = boxed(m);
        let mut listened = true;
        dispatch_player_render_listened(&FakeHost::empty(), &mods, &mut listened);
        assert!(!listened);
        no_error(&id);
    }

    #[test]
    fn generated_sign_changed() {
        let (m, id, _dir) = loaded(
            "gen_sign_changed",
            "horizon.on(\"sign_changed\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    assert(event.front == true, \"front\")\n    assert(event.lines[1] == \"test:seed\", \"lines\")\n    event.lines[1] = \"test:edited\"; table.insert(event.lines, \"test:added\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut lines = vec!["test:seed".to_owned()];
        let mut cancel = false;
        dispatch_sign_changed(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:dimension",
            [4, 5, 6],
            true,
            &mut lines,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(
            lines,
            vec!["test:edited".to_owned(), "test:added".to_owned()]
        );
        no_error(&id);
    }

    #[test]
    fn generated_standalone_models() {
        let (m, id, _dir) = loaded(
            "gen_standalone_models",
            "horizon.on(\"standalone_models\", function(event)\n    assert(event.models[1] == \"test:seed\", \"models\")\n    event.models[1] = \"test:edited\"; table.insert(event.models, \"test:added\")\nend)",
        );
        let mods = boxed(m);
        let mut models = vec!["test:seed".to_owned()];
        dispatch_standalone_models(&mods, &mut models);
        assert_eq!(
            models,
            vec!["test:edited".to_owned(), "test:added".to_owned()]
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_toggle_flight() {
        let (m, id, _dir) = loaded(
            "gen_player_toggle_flight",
            "horizon.on(\"player_toggle_flight\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.flying == true, \"flying\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_toggle_flight(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            true,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_beacon_state_change() {
        let (m, id, _dir) = loaded(
            "gen_beacon_state_change",
            "horizon.on(\"beacon_state_change\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.active == true, \"active\")\nend)",
        );
        let mods = boxed(m);
        dispatch_beacon_state_change(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            true,
        );
        no_error(&id);
    }

    #[test]
    fn generated_block_break_progress_update() {
        let (m, id, _dir) = loaded(
            "gen_block_break_progress_update",
            "horizon.on(\"block_break_progress_update\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.progress == 10, \"progress\")\nend)",
        );
        let mods = boxed(m);
        dispatch_block_break_progress_update(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            10,
        );
        no_error(&id);
    }

    #[test]
    fn generated_block_damage_abort() {
        let (m, id, _dir) = loaded(
            "gen_block_damage_abort",
            "horizon.on(\"block_damage_abort\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_block_damage_abort(&mut FakeHost::empty(), &mods, 7, "test:name", [3, 4, 5]);
        no_error(&id);
    }

    #[test]
    fn generated_block_dispense_armor() {
        let (m, id, _dir) = loaded(
            "gen_block_dispense_armor",
            "horizon.on(\"block_dispense_armor\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.target == \"test:target\", \"target\")\nend)",
        );
        let mods = boxed(m);
        dispatch_block_dispense_armor(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            "test:item",
            "test:target",
        );
        no_error(&id);
    }

    #[test]
    fn generated_block_exp() {
        let (m, id, _dir) = loaded(
            "gen_block_exp",
            "horizon.on(\"block_exp\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.block == \"test:block\", \"block\")\n    assert(event.experience == 10, \"experience\")\n    event.experience = 53\nend)",
        );
        let mods = boxed(m);
        let mut experience = 10;
        dispatch_block_exp(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            "test:block",
            &mut experience,
        );
        assert_eq!(experience, 53);
        no_error(&id);
    }

    #[test]
    fn generated_block_fade() {
        let (m, id, _dir) = loaded(
            "gen_block_fade",
            "horizon.on(\"block_fade\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.from == \"test:state10\", \"from\")\n    assert(event.to == \"test:state11\", \"to\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_block_fade(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            BlockStateId(10),
            BlockStateId(11),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_block_failed_dispense() {
        let (m, id, _dir) = loaded(
            "gen_block_failed_dispense",
            "horizon.on(\"block_failed_dispense\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\nend)",
        );
        let mods = boxed(m);
        dispatch_block_failed_dispense(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            "test:item",
        );
        no_error(&id);
    }

    #[test]
    fn generated_block_form() {
        let (m, id, _dir) = loaded(
            "gen_block_form",
            "horizon.on(\"block_form\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.from == \"test:state10\", \"from\")\n    assert(event.to == \"test:state11\", \"to\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_block_form(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            BlockStateId(10),
            BlockStateId(11),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_block_ignite() {
        let (m, id, _dir) = loaded(
            "gen_block_ignite",
            "horizon.on(\"block_ignite\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.player == \"00000000-0000-0000-0000-000000000008\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.cause == \"test:cause\", \"cause\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_block_ignite(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            Some(8),
            BlockPos::new(3, 4, 5),
            "test:cause",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_block_receive_game() {
        let (m, id, _dir) = loaded(
            "gen_block_receive_game",
            "horizon.on(\"block_receive_game\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.block == \"test:block\", \"block\")\n    assert(event.event == \"test:event\", \"event\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_block_receive_game(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            "test:block",
            "test:event",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_block_shear_entity() {
        let (m, id, _dir) = loaded(
            "gen_block_shear_entity",
            "horizon.on(\"block_shear_entity\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.entity == 9, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_block_shear_entity(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                9,
                "minecraft:skeleton",
            );
        }
        no_error(&id);
        dispatch_block_shear_entity(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            9,
            "minecraft:zombie",
        );
        no_error(&id);
    }

    #[test]
    fn generated_brewing_complete() {
        let (m, id, _dir) = loaded(
            "gen_brewing_complete",
            "horizon.on(\"brewing_complete\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_brewing_complete(&FakeHost::empty(), &mods, [1, 2, 3]);
        no_error(&id);
    }

    #[test]
    fn generated_brewing_fuel() {
        let (m, id, _dir) = loaded(
            "gen_brewing_fuel",
            "horizon.on(\"brewing_fuel\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.fuel == \"test:fuel\", \"fuel\")\nend)",
        );
        let mods = boxed(m);
        dispatch_brewing_fuel(&FakeHost::empty(), &mods, [1, 2, 3], "test:fuel");
        no_error(&id);
    }

    #[test]
    fn generated_brewing_start() {
        let (m, id, _dir) = loaded(
            "gen_brewing_start",
            "horizon.on(\"brewing_start\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.ingredient == \"test:ingredient\", \"ingredient\")\nend)",
        );
        let mods = boxed(m);
        dispatch_brewing_start(&FakeHost::empty(), &mods, [1, 2, 3], "test:ingredient");
        no_error(&id);
    }

    #[test]
    fn generated_campfire_start() {
        let (m, id, _dir) = loaded(
            "gen_campfire_start",
            "horizon.on(\"campfire_start\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\nend)",
        );
        let mods = boxed(m);
        dispatch_campfire_start(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        no_error(&id);
    }

    #[test]
    fn generated_cauldron_level_changed() {
        let (m, id, _dir) = loaded(
            "gen_cauldron_level_changed",
            "horizon.on(\"cauldron_level_changed\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.player == \"00000000-0000-0000-0000-000000000008\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.from_level == 10, \"from_level\")\n    assert(event.to_level == 11, \"to_level\")\n    assert(event.reason == \"test:reason\", \"reason\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_cauldron_level_changed(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            Some(8),
            BlockPos::new(3, 4, 5),
            10,
            11,
            "test:reason",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_chorus_fruit_teleport() {
        let (m, id, _dir) = loaded(
            "gen_chorus_fruit_teleport",
            "horizon.on(\"chorus_fruit_teleport\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from.x == 3, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.target.x == 4, \"target\")\n    assert(event.target.y == 2.0, \"target\")\n    assert(event.target.z == 3.0, \"target\")\n    event.target = { x = 14.0, y = 2.0, z = 3.0 }\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut target = [4.0, 2.0, 3.0];
        let mut cancel = false;
        dispatch_chorus_fruit_teleport(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3.0, 2.0, 3.0],
            &mut target,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(target, [14.0, 2.0, 3.0]);
        no_error(&id);
    }

    #[test]
    fn generated_compost_item() {
        let (m, id, _dir) = loaded(
            "gen_compost_item",
            "horizon.on(\"compost_item\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\nend)",
        );
        let mods = boxed(m);
        dispatch_compost_item(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        no_error(&id);
    }

    #[test]
    fn generated_crafter_craft() {
        let (m, id, _dir) = loaded(
            "gen_crafter_craft",
            "horizon.on(\"crafter_craft\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.recipe == \"test:recipe\", \"recipe\")\nend)",
        );
        let mods = boxed(m);
        dispatch_crafter_craft(&FakeHost::empty(), &mods, [1, 2, 3], "test:recipe");
        no_error(&id);
    }

    #[test]
    fn generated_creeper_ignite() {
        let (m, id, _dir) = loaded(
            "gen_creeper_ignite",
            "horizon.on(\"creeper_ignite\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.player == 8, \"player\")\n    assert(event.item == \"test:item\", \"item\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_creeper_ignite(&mut FakeHost::empty(), &mods, 13, 8, "test:item");
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_creeper_ignite(&mut FakeHost::empty(), &mods, 99, 8, "test:item");
        }
        no_error(&id);
        dispatch_creeper_ignite(&mut FakeHost::empty(), &mods, 11, 8, "test:item");
        no_error(&id);
    }

    #[test]
    fn generated_creeper_power() {
        let (m, id, _dir) = loaded(
            "gen_creeper_power",
            "horizon.on(\"creeper_power\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.lightning.x == 2, \"lightning\")\n    assert(event.lightning.y == 2.0, \"lightning\")\n    assert(event.lightning.z == 3.0, \"lightning\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_creeper_power(&mut FakeHost::empty(), &mods, 13, [2.0, 2.0, 3.0]);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_creeper_power(&mut FakeHost::empty(), &mods, 99, [2.0, 2.0, 3.0]);
        }
        no_error(&id);
        dispatch_creeper_power(&mut FakeHost::empty(), &mods, 11, [2.0, 2.0, 3.0]);
        no_error(&id);
    }

    #[test]
    fn generated_critical_hit() {
        let (m, id, _dir) = loaded(
            "gen_critical_hit",
            "horizon.on(\"critical_hit\", function(event)\n    assert(event.target_kind == \"test:target_kind\", \"target_kind\")\n    assert(event.vanilla_critical == false, \"vanilla_critical\")\n    assert(event.critical == true, \"critical\")\n    event.critical = false\n    assert(event.damage_multiplier == 4.5, \"damage_multiplier\")\n    event.damage_multiplier = 18.5\nend)",
        );
        let mods = boxed(m);
        let mut critical = true;
        let mut damage_multiplier = 4.5;
        dispatch_critical_hit(
            &mut FakeHost::empty(),
            &mods,
            "test:target_kind",
            false,
            &mut critical,
            &mut damage_multiplier,
        );
        assert!(!critical);
        assert_eq!(damage_multiplier, 18.5);
        no_error(&id);
    }

    #[test]
    fn generated_difficulty_changed() {
        let (m, id, _dir) = loaded(
            "gen_difficulty_changed",
            "horizon.on(\"difficulty_changed\", function(event)\n    assert(event.from == \"test:from\", \"from\")\n    assert(event.to == \"test:to\", \"to\")\nend)",
        );
        let mods = boxed(m);
        dispatch_difficulty_changed(&mut FakeHost::empty(), &mods, "test:from", "test:to");
        no_error(&id);
    }

    #[test]
    fn generated_dragon_egg_form() {
        let (m, id, _dir) = loaded(
            "gen_dragon_egg_form",
            "horizon.on(\"dragon_egg_form\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_dragon_egg_form(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_enchant_item() {
        let (m, id, _dir) = loaded(
            "gen_enchant_item",
            "horizon.on(\"enchant_item\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.item.id == \"test:item\", \"item\")\n    assert(event.levels == 11, \"levels\")\nend)",
        );
        let mods = boxed(m);
        dispatch_enchant_item(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &ItemStack {
                item: "test:item".into(),
                count: 4,
                damage: 3,
                extra: None,
            },
            11,
        );
        no_error(&id);
    }

    #[test]
    fn generated_ender_pearl_teleport() {
        let (m, id, _dir) = loaded(
            "gen_ender_pearl_teleport",
            "horizon.on(\"ender_pearl_teleport\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pearl == 9, \"pearl\")\n    assert(event.from.x == 4, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.target.x == 5, \"target\")\n    assert(event.target.y == 2.0, \"target\")\n    assert(event.target.z == 3.0, \"target\")\n    event.target = { x = 15.0, y = 2.0, z = 3.0 }\n    assert(event.damage == 6.5, \"damage\")\n    event.damage = 20.5\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut target = [5.0, 2.0, 3.0];
        let mut damage = 6.5;
        let mut cancel = false;
        dispatch_ender_pearl_teleport(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            [4.0, 2.0, 3.0],
            &mut target,
            &mut damage,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(target, [15.0, 2.0, 3.0]);
        assert_eq!(damage, 20.5);
        no_error(&id);
    }

    #[test]
    fn generated_entities_load() {
        let (m, id, _dir) = loaded(
            "gen_entities_load",
            "horizon.on(\"entities_load\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.chunk.x == 4, \"chunk\")\n    assert(event.chunk.z == -6, \"chunk\")\n    assert(event.count == 9, \"count\")\nend)",
        );
        let mods = boxed(m);
        dispatch_entities_load(&mut FakeHost::empty(), &mods, "test:dimension", [4, -6], 9);
        no_error(&id);
    }

    #[test]
    fn generated_entities_unload() {
        let (m, id, _dir) = loaded(
            "gen_entities_unload",
            "horizon.on(\"entities_unload\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.chunk.x == 4, \"chunk\")\n    assert(event.chunk.z == -6, \"chunk\")\n    assert(event.count == 9, \"count\")\nend)",
        );
        let mods = boxed(m);
        dispatch_entities_unload(&mut FakeHost::empty(), &mods, "test:dimension", [4, -6], 9);
        no_error(&id);
    }

    #[test]
    fn generated_entity_break_door() {
        let (m, id, _dir) = loaded(
            "gen_entity_break_door",
            "horizon.on(\"entity_break_door\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_break_door(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                [3, 4, 5],
            );
        }
        no_error(&id);
        dispatch_entity_break_door(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            [3, 4, 5],
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_breed() {
        let (m, id, _dir) = loaded(
            "gen_entity_breed",
            "horizon.on(\"entity_breed\", function(event)\n    assert(event.parent == 7, \"parent\")\n    assert(event.partner == 8, \"partner\")\n    assert(event.child == 9, \"child\")\nend)",
        );
        let mods = boxed(m);
        dispatch_entity_breed(&mut FakeHost::empty(), &mods, 7, 8, Some(9));
        no_error(&id);
    }

    #[test]
    fn generated_entity_change_block() {
        let (m, id, _dir) = loaded(
            "gen_entity_change_block",
            "horizon.on(\"entity_change_block\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.from == \"test:from\", \"from\")\n    assert(event.to == \"test:to\", \"to\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_change_block(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                [3, 4, 5],
                "test:from",
                "test:to",
            );
        }
        no_error(&id);
        dispatch_entity_change_block(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            [3, 4, 5],
            "test:from",
            "test:to",
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_drop_item() {
        let (m, id, _dir) = loaded(
            "gen_entity_drop_item",
            "horizon.on(\"entity_drop_item\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.count == 9, \"count\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_drop_item(
                &mut FakeHost::empty(),
                &mods,
                13,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_entity_drop_item(
                &mut FakeHost::empty(),
                &mods,
                99,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_entity_drop_item(
            &mut FakeHost::empty(),
            &mods,
            11,
            "test:item",
            9,
            [4.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_enter_block() {
        let (m, id, _dir) = loaded(
            "gen_entity_enter_block",
            "horizon.on(\"entity_enter_block\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.block == \"test:block\", \"block\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_enter_block(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                [3, 4, 5],
                "test:block",
            );
        }
        no_error(&id);
        dispatch_entity_enter_block(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            [3, 4, 5],
            "test:block",
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_enter_love_mode() {
        let (m, id, _dir) = loaded(
            "gen_entity_enter_love_mode",
            "horizon.on(\"entity_enter_love_mode\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.player == \"00000000-0000-0000-0000-000000000009\", \"player\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_enter_love_mode(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                Some(9),
            );
        }
        no_error(&id);
        dispatch_entity_enter_love_mode(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            Some(9),
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_portal() {
        let (m, id, _dir) = loaded(
            "gen_entity_portal",
            "horizon.on(\"entity_portal\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_portal(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                13,
                [3, 4, 5],
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_entity_portal(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                99,
                [3, 4, 5],
            );
        }
        no_error(&id);
        dispatch_entity_portal(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            11,
            [3, 4, 5],
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_pose_change() {
        let (m, id, _dir) = loaded(
            "gen_entity_pose_change",
            "horizon.on(\"entity_pose_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from == \"test:from\", \"from\")\n    assert(event.to == \"test:to\", \"to\")\nend)",
        );
        let mods = boxed(m);
        dispatch_entity_pose_change(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:from",
            "test:to",
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_shoot_bow() {
        let (m, id, _dir) = loaded(
            "gen_entity_shoot_bow",
            "horizon.on(\"entity_shoot_bow\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.projectile == 11, \"kind\")\n    assert(event.force == 4.5, \"force\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_shoot_bow(&mut FakeHost::empty(), &mods, 7, "test:name", 13, 4.5);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_entity_shoot_bow(&mut FakeHost::empty(), &mods, 7, "test:name", 99, 4.5);
        }
        no_error(&id);
        dispatch_entity_shoot_bow(&mut FakeHost::empty(), &mods, 7, "test:name", 11, 4.5);
        no_error(&id);
    }

    #[test]
    fn generated_entity_tame() {
        let (m, id, _dir) = loaded(
            "gen_entity_tame",
            "horizon.on(\"entity_tame\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.owner == \"00000000-0000-0000-0000-000000000009\", \"owner\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_tame(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                Some(9),
            );
        }
        no_error(&id);
        dispatch_entity_tame(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            Some(9),
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_teleport() {
        let (m, id, _dir) = loaded(
            "gen_entity_teleport",
            "horizon.on(\"entity_teleport\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 11, \"kind\")\n    assert(event.from.x == 3, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.to.x == 4, \"to\")\n    assert(event.to.y == 2.0, \"to\")\n    assert(event.to.z == 3.0, \"to\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_teleport(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                13,
                [3.0, 2.0, 3.0],
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_entity_teleport(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                99,
                [3.0, 2.0, 3.0],
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_entity_teleport(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            11,
            [3.0, 2.0, 3.0],
            [4.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_toggle_glide() {
        let (m, id, _dir) = loaded(
            "gen_entity_toggle_glide",
            "horizon.on(\"entity_toggle_glide\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.gliding == true, \"gliding\")\nend)",
        );
        let mods = boxed(m);
        dispatch_entity_toggle_glide(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        no_error(&id);
    }

    #[test]
    fn generated_entity_transform() {
        let (m, id, _dir) = loaded(
            "gen_entity_transform",
            "horizon.on(\"entity_transform\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.transformed == 9, \"transformed\")\n    assert(event.transformed_kind == \"test:transformed_kind\", \"transformed_kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_transform(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                9,
                "test:transformed_kind",
            );
        }
        no_error(&id);
        dispatch_entity_transform(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            9,
            "test:transformed_kind",
        );
        no_error(&id);
    }

    #[test]
    fn generated_entity_unleash() {
        let (m, id, _dir) = loaded(
            "gen_entity_unleash",
            "horizon.on(\"entity_unleash\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_entity_unleash(&mut FakeHost::empty(), &mods, 7, "minecraft:skeleton");
        }
        no_error(&id);
        dispatch_entity_unleash(&mut FakeHost::empty(), &mods, 7, "minecraft:zombie");
        no_error(&id);
    }

    #[test]
    fn generated_exp_bottle() {
        let (m, id, _dir) = loaded(
            "gen_exp_bottle",
            "horizon.on(\"exp_bottle\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\n    assert(event.experience == 8, \"experience\")\nend)",
        );
        let mods = boxed(m);
        dispatch_exp_bottle(&mut FakeHost::empty(), &mods, [1.0, 2.0, 3.0], 8);
        no_error(&id);
    }

    #[test]
    fn generated_experience_orb_pickup_listened() {
        let (m, id, _dir) = loaded(
            "gen_experience_orb_pickup_listened",
            "horizon.on(\"experience_orb_pickup_listened\", function(event)\n    assert(event.listened == true, \"listened\")\n    event.listened = false\nend)",
        );
        let mods = boxed(m);
        let mut listened = true;
        dispatch_experience_orb_pickup_listened(&FakeHost::empty(), &mods, &mut listened);
        assert!(!listened);
        no_error(&id);
    }

    #[test]
    fn generated_experience_orb_pickup() {
        let (m, id, _dir) = loaded(
            "gen_experience_orb_pickup",
            "horizon.on(\"experience_orb_pickup\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.orb == 9, \"orb\")\n    assert(event.value == 10, \"value\")\n    event.value = 53\n    assert(event.count == 11, \"count\")\n    event.count = 54\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut value = 10;
        let mut count = 11;
        let mut canceled = false;
        dispatch_experience_orb_pickup(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:dimension",
            9,
            &mut value,
            &mut count,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        assert_eq!(value, 53);
        assert_eq!(count, 54);
        no_error(&id);
    }

    #[test]
    fn generated_mending_repair() {
        let (m, id, _dir) = loaded(
            "gen_mending_repair",
            "horizon.on(\"mending_repair\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.experience == 9, \"experience\")\n    assert(event.repair == 10, \"repair\")\n    event.repair = 53\nend)",
        );
        let mods = boxed(m);
        let mut repair = 10;
        dispatch_mending_repair(
            &mut FakeHost::empty(),
            &mods,
            7,
            &ItemStack {
                item: "test:stack".into(),
                count: 2,
                damage: 1,
                extra: None,
            },
            9,
            &mut repair,
        );
        assert_eq!(repair, 53);
        no_error(&id);
    }

    #[test]
    fn generated_explosion_prime() {
        let (m, id, _dir) = loaded(
            "gen_explosion_prime",
            "horizon.on(\"explosion_prime\", function(event)\n    assert(event.center.x == 1, \"center\")\n    assert(event.center.y == 2.0, \"center\")\n    assert(event.center.z == 3.0, \"center\")\n    assert(event.radius == 2.5, \"radius\")\nend)",
        );
        let mods = boxed(m);
        dispatch_explosion_prime(&mut FakeHost::empty(), &mods, [1.0, 2.0, 3.0], 2.5);
        no_error(&id);
    }

    #[test]
    fn generated_smoke_source() {
        let (m, id, _dir) = loaded(
            "gen_smoke_source",
            "horizon.on(\"smoke_source\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.state == \"test:state9\", \"state\")\n    assert(event.smoke == nil, \"replace seed\")\n    event.smoke = true\nend)",
        );
        let mods = boxed(m);
        let mut smoke: Option<bool> = None;
        dispatch_smoke_source(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockStateId(9),
            &mut smoke,
        );
        assert_eq!(smoke, Some(true));
        no_error(&id);
    }

    #[test]
    fn generated_firework_explode() {
        let (m, id, _dir) = loaded(
            "gen_firework_explode",
            "horizon.on(\"firework_explode\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\n    assert(event.bursts == 9, \"bursts\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_firework_explode(&mut FakeHost::empty(), &mods, 13, [2.0, 2.0, 3.0], 9);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_firework_explode(&mut FakeHost::empty(), &mods, 99, [2.0, 2.0, 3.0], 9);
        }
        no_error(&id);
        dispatch_firework_explode(&mut FakeHost::empty(), &mods, 11, [2.0, 2.0, 3.0], 9);
        no_error(&id);
    }

    #[test]
    fn generated_fluid_level_change() {
        let (m, id, _dir) = loaded(
            "gen_fluid_level_change",
            "horizon.on(\"fluid_level_change\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.fluid == \"test:fluid\", \"fluid\")\n    assert(event.from == 10, \"from\")\n    assert(event.to == 11, \"to\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_fluid_level_change(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            "test:fluid",
            10,
            11,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_framerate_limit() {
        let (m, id, _dir) = loaded(
            "gen_framerate_limit",
            "horizon.on(\"framerate_limit\", function(event)\n    assert(event.focused == true, \"focused\")\n    assert(event.minimized == false, \"minimized\")\n    assert(event.in_world == true, \"in_world\")\n    assert(event.limit == 10, \"limit\")\n    event.limit = 53\nend)",
        );
        let mods = boxed(m);
        let mut limit = 10;
        dispatch_framerate_limit(&FakeHost::empty(), &mods, true, false, true, &mut limit);
        assert_eq!(limit, 53);
        no_error(&id);
    }

    #[test]
    fn generated_furnace_burn() {
        let (m, id, _dir) = loaded(
            "gen_furnace_burn",
            "horizon.on(\"furnace_burn\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.fuel == \"test:fuel\", \"fuel\")\nend)",
        );
        let mods = boxed(m);
        dispatch_furnace_burn(&FakeHost::empty(), &mods, [1, 2, 3], "test:fuel");
        no_error(&id);
    }

    #[test]
    fn generated_furnace_extract() {
        let (m, id, _dir) = loaded(
            "gen_furnace_extract",
            "horizon.on(\"furnace_extract\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.count == 11, \"count\")\nend)",
        );
        let mods = boxed(m);
        dispatch_furnace_extract(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            "test:item",
            11,
        );
        no_error(&id);
    }

    #[test]
    fn generated_furnace_smelt() {
        let (m, id, _dir) = loaded(
            "gen_furnace_smelt",
            "horizon.on(\"furnace_smelt\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.result == \"test:result\", \"result\")\nend)",
        );
        let mods = boxed(m);
        dispatch_furnace_smelt(
            &FakeHost::empty(),
            &mods,
            [1, 2, 3],
            "test:item",
            "test:result",
        );
        no_error(&id);
    }

    #[test]
    fn generated_furnace_start_smelt() {
        let (m, id, _dir) = loaded(
            "gen_furnace_start_smelt",
            "horizon.on(\"furnace_start_smelt\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\nend)",
        );
        let mods = boxed(m);
        dispatch_furnace_start_smelt(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        no_error(&id);
    }

    #[test]
    fn generated_game_event() {
        let (m, id, _dir) = loaded(
            "gen_game_event",
            "horizon.on(\"game_event\", function(event)\n    assert(event.event == \"test:event\", \"event\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_game_event(&mut FakeHost::empty(), &mods, "test:event", [2.0, 2.0, 3.0]);
        no_error(&id);
    }

    #[test]
    fn generated_hopper_inventory_search() {
        let (m, id, _dir) = loaded(
            "gen_hopper_inventory_search",
            "horizon.on(\"hopper_inventory_search\", function(event)\n    assert(event.hopper.x == 1, \"hopper\")\n    assert(event.hopper.y == 2, \"hopper\")\n    assert(event.hopper.z == 3, \"hopper\")\n    assert(event.container.x == 2, \"container\")\n    assert(event.container.y == 3, \"container\")\n    assert(event.container.z == 4, \"container\")\n    assert(event.entity == 11, \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_hopper_inventory_search(
                &FakeHost::empty(),
                &mods,
                [1, 2, 3],
                Some([2, 3, 4]),
                Some(13),
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_hopper_inventory_search(
                &FakeHost::empty(),
                &mods,
                [1, 2, 3],
                Some([2, 3, 4]),
                None,
            );
        }
        no_error(&id);
        dispatch_hopper_inventory_search(
            &FakeHost::empty(),
            &mods,
            [1, 2, 3],
            Some([2, 3, 4]),
            Some(11),
        );
        no_error(&id);
    }

    #[test]
    fn generated_hopper_pickup_item() {
        let (m, id, _dir) = loaded(
            "gen_hopper_pickup_item",
            "horizon.on(\"hopper_pickup_item\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.count == 10, \"count\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_hopper_pickup_item(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            "test:item",
            10,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_inventory_block_start() {
        let (m, id, _dir) = loaded(
            "gen_inventory_block_start",
            "horizon.on(\"inventory_block_start\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_inventory_block_start(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_inventory_move_item() {
        let (m, id, _dir) = loaded(
            "gen_inventory_move_item",
            "horizon.on(\"inventory_move_item\", function(event)\n    assert(event.source.x == 1, \"source\")\n    assert(event.source.y == 2, \"source\")\n    assert(event.source.z == 3, \"source\")\n    assert(event.destination.x == 2, \"destination\")\n    assert(event.destination.y == 3, \"destination\")\n    assert(event.destination.z == 4, \"destination\")\n    assert(event.item == \"test:item\", \"item\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_inventory_move_item(
            &FakeHost::empty(),
            &mods,
            [1, 2, 3],
            [2, 3, 4],
            "test:item",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_item_despawn() {
        let (m, id, _dir) = loaded(
            "gen_item_despawn",
            "horizon.on(\"item_despawn\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.count == 9, \"count\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_item_despawn(
                &mut FakeHost::empty(),
                &mods,
                13,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_item_despawn(
                &mut FakeHost::empty(),
                &mods,
                99,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_item_despawn(
            &mut FakeHost::empty(),
            &mods,
            11,
            "test:item",
            9,
            [4.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_item_expire() {
        let (m, id, _dir) = loaded(
            "gen_item_expire",
            "horizon.on(\"item_expire\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.extra_life == 9, \"extra_life\")\n    event.extra_life = 52\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut extra_life = 9;
            dispatch_item_expire(
                &mut FakeHost::empty(),
                &mods,
                13,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 2,
                    damage: 1,
                    extra: None,
                },
                &mut extra_life,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            let mut extra_life = 9;
            dispatch_item_expire(
                &mut FakeHost::empty(),
                &mods,
                99,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 2,
                    damage: 1,
                    extra: None,
                },
                &mut extra_life,
            );
        }
        no_error(&id);
        let mut extra_life = 9;
        dispatch_item_expire(
            &mut FakeHost::empty(),
            &mods,
            11,
            &ItemStack {
                item: "test:stack".into(),
                count: 2,
                damage: 1,
                extra: None,
            },
            &mut extra_life,
        );
        assert_eq!(extra_life, 52);
        no_error(&id);
    }

    #[test]
    fn generated_item_spawn() {
        let (m, id, _dir) = loaded(
            "gen_item_spawn",
            "horizon.on(\"item_spawn\", function(event)\n    assert(event.entity == 11, \"kind\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.count == 9, \"count\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_item_spawn(
                &mut FakeHost::empty(),
                &mods,
                13,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_item_spawn(
                &mut FakeHost::empty(),
                &mods,
                99,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_item_spawn(
            &mut FakeHost::empty(),
            &mods,
            11,
            "test:item",
            9,
            [4.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_leaves_decay() {
        let (m, id, _dir) = loaded(
            "gen_leaves_decay",
            "horizon.on(\"leaves_decay\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_leaves_decay(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_lightning_strike() {
        let (m, id, _dir) = loaded(
            "gen_lightning_strike",
            "horizon.on(\"lightning_strike\", function(event)\n    assert(event.player == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.cause == \"test:cause\", \"cause\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_lightning_strike(
            &mut FakeHost::empty(),
            &mods,
            Some(7),
            [2, 3, 4],
            "test:cause",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_moisture_change() {
        let (m, id, _dir) = loaded(
            "gen_moisture_change",
            "horizon.on(\"moisture_change\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.from == 9, \"from\")\n    assert(event.to == 10, \"to\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_moisture_change(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            9,
            10,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_armor_stand_manipulate() {
        let (m, id, _dir) = loaded(
            "gen_armor_stand_manipulate",
            "horizon.on(\"armor_stand_manipulate\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.stand_id == 9, \"stand_id\")\n    assert(event.slot == \"test:slot\", \"slot\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_armor_stand_manipulate(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            "test:slot",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_bucket_empty() {
        let (m, id, _dir) = loaded(
            "gen_bucket_empty",
            "horizon.on(\"bucket_empty\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.fluid == \"test:fluid\", \"fluid\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_bucket_empty(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            "test:fluid",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_container_opened() {
        let (m, id, _dir) = loaded(
            "gen_container_opened",
            "horizon.on(\"container_opened\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.kind == \"test:kind\", \"kind\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_container_opened(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:kind",
            Some([4, 5, 6]),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_creative_slot() {
        let (m, id, _dir) = loaded(
            "gen_creative_slot",
            "horizon.on(\"creative_slot\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.slot == 9, \"slot\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.count == 11, \"count\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_creative_slot(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            "test:item",
            11,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_food_level_changed() {
        let (m, id, _dir) = loaded(
            "gen_food_level_changed",
            "horizon.on(\"food_level_changed\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from == 9, \"from\")\n    assert(event.to == 10, \"to\")\n    event.to = 53\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut to = 10;
        let mut cancel = false;
        dispatch_food_level_changed(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            &mut to,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(to, 53);
        no_error(&id);
    }

    #[test]
    fn generated_hanging_break() {
        let (m, id, _dir) = loaded(
            "gen_hanging_break",
            "horizon.on(\"hanging_break\", function(event)\n    assert(event.player == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_hanging_break(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                "minecraft:skeleton",
                [3, 4, 5],
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_hanging_break(
            &mut FakeHost::empty(),
            &mods,
            Some(7),
            "minecraft:zombie",
            [3, 4, 5],
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_hanging_place() {
        let (m, id, _dir) = loaded(
            "gen_hanging_place",
            "horizon.on(\"hanging_place\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_hanging_place(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "minecraft:skeleton",
                [4, 5, 6],
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_hanging_place(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "minecraft:zombie",
            [4, 5, 6],
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_item_damaged() {
        let (m, id, _dir) = loaded(
            "gen_item_damaged",
            "horizon.on(\"item_damaged\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.amount == 10, \"amount\")\n    event.amount = 53\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut amount = 10;
        let mut cancel = false;
        dispatch_item_damaged(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:item",
            &mut amount,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(amount, 53);
        no_error(&id);
    }

    #[test]
    fn generated_player_clone() {
        let (m, id, _dir) = loaded(
            "gen_player_clone",
            "horizon.on(\"player_clone\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.was_death == true, \"was_death\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_clone(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        no_error(&id);
    }

    #[test]
    fn generated_player_commands_sent() {
        let (m, id, _dir) = loaded(
            "gen_player_commands_sent",
            "horizon.on(\"player_commands_sent\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.names[1] == \"test:seed\", \"names\")\n    event.names[1] = \"test:edited\"; table.insert(event.names, \"test:added\")\nend)",
        );
        let mods = boxed(m);
        let mut names = vec!["test:seed".to_owned()];
        dispatch_player_commands_sent(&mut FakeHost::empty(), &mods, 7, "test:name", &mut names);
        assert_eq!(
            names,
            vec!["test:edited".to_owned(), "test:added".to_owned()]
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_destroy_item() {
        let (m, id, _dir) = loaded(
            "gen_player_destroy_item",
            "horizon.on(\"player_destroy_item\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.slot == \"test:slot\", \"slot\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_destroy_item(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:item",
            "test:slot",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_fish() {
        let (m, id, _dir) = loaded(
            "gen_player_fish",
            "horizon.on(\"player_fish\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.state == 9, \"state\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_fish(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_flower_pot_manipulate() {
        let (m, id, _dir) = loaded(
            "gen_player_flower_pot_manipulate",
            "horizon.on(\"player_flower_pot_manipulate\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.plant == \"test:plant\", \"plant\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_flower_pot_manipulate(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            "test:plant",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_game_mode_changed() {
        let (m, id, _dir) = loaded(
            "gen_player_game_mode_changed",
            "horizon.on(\"player_game_mode_changed\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from == 9, \"from\")\n    assert(event.to == 10, \"to\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_game_mode_changed(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            10,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_insert_lectern_book() {
        let (m, id, _dir) = loaded(
            "gen_player_insert_lectern_book",
            "horizon.on(\"player_insert_lectern_book\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.book == \"test:book\", \"book\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_insert_lectern_book(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            "test:book",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_item_frame_change() {
        let (m, id, _dir) = loaded(
            "gen_player_item_frame_change",
            "horizon.on(\"player_item_frame_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.action == \"test:action\", \"action\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.rotation == 12, \"rotation\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_item_frame_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
                "test:action",
                "test:item",
                12,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_item_frame_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
                "test:action",
                "test:item",
                12,
            );
        }
        no_error(&id);
        dispatch_player_item_frame_change(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            11,
            "test:action",
            "test:item",
            12,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_item_mend() {
        let (m, id, _dir) = loaded(
            "gen_player_item_mend",
            "horizon.on(\"player_item_mend\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.repair == 10, \"repair\")\n    assert(event.experience == 11, \"experience\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_item_mend(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:item",
            10,
            11,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_lectern_page_change() {
        let (m, id, _dir) = loaded(
            "gen_player_lectern_page_change",
            "horizon.on(\"player_lectern_page_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.page == 10, \"page\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_lectern_page_change(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            10,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_sprint() {
        let (m, id, _dir) = loaded(
            "gen_player_sprint",
            "horizon.on(\"player_sprint\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.sprinting == true, \"sprinting\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_sprint(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        no_error(&id);
    }

    #[test]
    fn generated_player_swap_hands() {
        let (m, id, _dir) = loaded(
            "gen_player_swap_hands",
            "horizon.on(\"player_swap_hands\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_swap_hands(&mut FakeHost::empty(), &mods, 7, "test:name", &mut cancel);
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_take_lectern_book() {
        let (m, id, _dir) = loaded(
            "gen_player_take_lectern_book",
            "horizon.on(\"player_take_lectern_book\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.book == \"test:book\", \"book\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_take_lectern_book(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            "test:book",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_teleported() {
        let (m, id, _dir) = loaded(
            "gen_player_teleported",
            "horizon.on(\"player_teleported\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from.x == 3, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.to.x == 4, \"to\")\n    assert(event.to.y == 2.0, \"to\")\n    assert(event.to.z == 3.0, \"to\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_teleported(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3.0, 2.0, 3.0],
            [4.0, 2.0, 3.0],
            "test:dimension",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_totem() {
        let (m, id, _dir) = loaded(
            "gen_player_totem",
            "horizon.on(\"player_totem\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_totem(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_resource_pack_status() {
        let (m, id, _dir) = loaded(
            "gen_resource_pack_status",
            "horizon.on(\"resource_pack_status\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.status == 9, \"status\")\nend)",
        );
        let mods = boxed(m);
        dispatch_resource_pack_status(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        no_error(&id);
    }

    #[test]
    fn generated_spawn_change() {
        let (m, id, _dir) = loaded(
            "gen_spawn_change",
            "horizon.on(\"spawn_change\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.angle.x == 2.5, \"angle\")\n    assert(event.angle.y == 3.5, \"angle\")\nend)",
        );
        let mods = boxed(m);
        dispatch_spawn_change(&mut FakeHost::empty(), &mods, [1, 2, 3], [2.5, 3.5]);
        no_error(&id);
    }

    #[test]
    fn generated_tnt_primed() {
        let (m, id, _dir) = loaded(
            "gen_tnt_primed",
            "horizon.on(\"tnt_primed\", function(event)\n    assert(event.player == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_tnt_primed(
            &mut FakeHost::empty(),
            &mods,
            Some(7),
            [2, 3, 4],
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_vehicle_create() {
        let (m, id, _dir) = loaded(
            "gen_vehicle_create",
            "horizon.on(\"vehicle_create\", function(event)\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_vehicle_create(
                &mut FakeHost::empty(),
                &mods,
                "minecraft:skeleton",
                [2.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_vehicle_create(
            &mut FakeHost::empty(),
            &mods,
            "minecraft:zombie",
            [2.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_vehicle_damage() {
        let (m, id, _dir) = loaded(
            "gen_vehicle_damage",
            "horizon.on(\"vehicle_damage\", function(event)\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.damage == 2.5, \"damage\")\n    event.damage = 16.5\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut damage = 2.5;
            let mut cancel = false;
            dispatch_vehicle_damage(
                &mut FakeHost::empty(),
                &mods,
                "minecraft:skeleton",
                &mut damage,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut damage = 2.5;
        let mut cancel = false;
        dispatch_vehicle_damage(
            &mut FakeHost::empty(),
            &mods,
            "minecraft:zombie",
            &mut damage,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(damage, 16.5);
        no_error(&id);
    }

    #[test]
    fn generated_vehicle_destroy() {
        let (m, id, _dir) = loaded(
            "gen_vehicle_destroy",
            "horizon.on(\"vehicle_destroy\", function(event)\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_vehicle_destroy(
                &mut FakeHost::empty(),
                &mods,
                "minecraft:skeleton",
                [2.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_vehicle_destroy(
            &mut FakeHost::empty(),
            &mods,
            "minecraft:zombie",
            [2.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_vehicle_moved() {
        let (m, id, _dir) = loaded(
            "gen_vehicle_moved",
            "horizon.on(\"vehicle_moved\", function(event)\n    assert(event.player == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.from.x == 3, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.to.x == 4, \"to\")\n    assert(event.to.y == 2.0, \"to\")\n    assert(event.to.z == 3.0, \"to\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_vehicle_moved(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                "minecraft:skeleton",
                [3.0, 2.0, 3.0],
                [4.0, 2.0, 3.0],
            );
        }
        no_error(&id);
        dispatch_vehicle_moved(
            &mut FakeHost::empty(),
            &mods,
            Some(7),
            "minecraft:zombie",
            [3.0, 2.0, 3.0],
            [4.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_world_border_bounds_changed() {
        let (m, id, _dir) = loaded(
            "gen_world_border_bounds_changed",
            "horizon.on(\"world_border_bounds_changed\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_world_border_bounds_changed(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_world_border_center_changed() {
        let (m, id, _dir) = loaded(
            "gen_world_border_center_changed",
            "horizon.on(\"world_border_center_changed\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_world_border_center_changed(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_world_game_rule_change() {
        let (m, id, _dir) = loaded(
            "gen_world_game_rule_change",
            "horizon.on(\"world_game_rule_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.rule == \"test:rule\", \"rule\")\n    assert(event.value == \"test:value\", \"value\")\nend)",
        );
        let mods = boxed(m);
        dispatch_world_game_rule_change(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:rule",
            "test:value",
        );
        no_error(&id);
    }

    #[test]
    fn generated_note_play() {
        let (m, id, _dir) = loaded(
            "gen_note_play",
            "horizon.on(\"note_play\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.player == \"00000000-0000-0000-0000-000000000008\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.instrument == \"test:instrument\", \"instrument\")\n    assert(event.note == 11, \"note\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_note_play(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            Some(8),
            BlockPos::new(3, 4, 5),
            "test:instrument",
            11,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_advancement_criterion_grant() {
        let (m, id, _dir) = loaded(
            "gen_player_advancement_criterion_grant",
            "horizon.on(\"player_advancement_criterion_grant\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.advancement == \"test:advancement\", \"advancement\")\n    assert(event.criterion == \"test:criterion\", \"criterion\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_advancement_criterion_grant(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:advancement",
            "test:criterion",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_advancement_done() {
        let (m, id, _dir) = loaded(
            "gen_player_advancement_done",
            "horizon.on(\"player_advancement_done\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.advancement == \"test:advancement\", \"advancement\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_advancement_done(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:advancement",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_advancement_progress() {
        let (m, id, _dir) = loaded(
            "gen_player_advancement_progress",
            "horizon.on(\"player_advancement_progress\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.advancement == \"test:advancement\", \"advancement\")\n    assert(event.criterion == \"test:criterion\", \"criterion\")\n    assert(event.granted == true, \"granted\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_advancement_progress(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:advancement",
            "test:criterion",
            true,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_arm_swing() {
        let (m, id, _dir) = loaded(
            "gen_player_arm_swing",
            "horizon.on(\"player_arm_swing\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.hand == \"test:hand\", \"hand\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_arm_swing(&mut FakeHost::empty(), &mods, 7, "test:name", "test:hand");
        no_error(&id);
    }

    #[test]
    fn generated_player_attack_cooldown_reset() {
        let (m, id, _dir) = loaded(
            "gen_player_attack_cooldown_reset",
            "horizon.on(\"player_attack_cooldown_reset\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_attack_cooldown_reset(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_bed_leave() {
        let (m, id, _dir) = loaded(
            "gen_player_bed_leave",
            "horizon.on(\"player_bed_leave\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_bed_leave(&mut FakeHost::empty(), &mods, 7, "test:name", [3, 4, 5]);
        no_error(&id);
    }

    #[test]
    fn generated_player_bucket_entity() {
        let (m, id, _dir) = loaded(
            "gen_player_bucket_entity",
            "horizon.on(\"player_bucket_entity\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.entity == \"test:entity\", \"entity\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_bucket_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
                "test:entity",
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_bucket_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
                "test:entity",
            );
        }
        no_error(&id);
        dispatch_player_bucket_entity(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            11,
            "test:entity",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_bundle_item_select() {
        let (m, id, _dir) = loaded(
            "gen_player_bundle_item_select",
            "horizon.on(\"player_bundle_item_select\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.slot == 9, \"slot\")\n    assert(event.index == 10, \"index\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_bundle_item_select(&mut FakeHost::empty(), &mods, 7, "test:name", 9, 10);
        no_error(&id);
    }

    #[test]
    fn generated_player_change_beacon_effect() {
        let (m, id, _dir) = loaded(
            "gen_player_change_beacon_effect",
            "horizon.on(\"player_change_beacon_effect\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.primary == \"test:primary\", \"primary\")\n    assert(event.secondary == \"test:secondary\", \"secondary\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_change_beacon_effect(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            "test:primary",
            "test:secondary",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_change_main_hand() {
        let (m, id, _dir) = loaded(
            "gen_player_change_main_hand",
            "horizon.on(\"player_change_main_hand\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.left == true, \"left\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_change_main_hand(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        no_error(&id);
    }

    #[test]
    fn generated_player_client_loaded_world() {
        let (m, id, _dir) = loaded(
            "gen_player_client_loaded_world",
            "horizon.on(\"player_client_loaded_world\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_client_loaded_world(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_deep_sleep() {
        let (m, id, _dir) = loaded(
            "gen_player_deep_sleep",
            "horizon.on(\"player_deep_sleep\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_deep_sleep(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_edit_book() {
        let (m, id, _dir) = loaded(
            "gen_player_edit_book",
            "horizon.on(\"player_edit_book\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pages == 9, \"pages\")\n    assert(event.signing == false, \"signing\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_edit_book(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            false,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_egg_throw() {
        let (m, id, _dir) = loaded(
            "gen_player_egg_throw",
            "horizon.on(\"player_egg_throw\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_egg_throw(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_elytra_boost() {
        let (m, id, _dir) = loaded(
            "gen_player_elytra_boost",
            "horizon.on(\"player_elytra_boost\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_elytra_boost(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_input() {
        let (m, id, _dir) = loaded(
            "gen_player_input",
            "horizon.on(\"player_input\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.flags == 9, \"flags\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_input(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        no_error(&id);
    }

    #[test]
    fn generated_player_item_held() {
        let (m, id, _dir) = loaded(
            "gen_player_item_held",
            "horizon.on(\"player_item_held\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from_slot == 9, \"from_slot\")\n    assert(event.to_slot == 10, \"to_slot\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_item_held(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            10,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_jump() {
        let (m, id, _dir) = loaded(
            "gen_player_jump",
            "horizon.on(\"player_jump\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_jump(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_kicked() {
        let (m, id, _dir) = loaded(
            "gen_player_kicked",
            "horizon.on(\"player_kicked\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.reason == \"test:reason\", \"reason\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_kicked(&mut FakeHost::empty(), &mods, 7, "test:name", "test:reason");
        no_error(&id);
    }

    #[test]
    fn generated_player_launch_projectile() {
        let (m, id, _dir) = loaded(
            "gen_player_launch_projectile",
            "horizon.on(\"player_launch_projectile\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.item == \"test:item\", \"item\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_launch_projectile(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:item",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_level_change() {
        let (m, id, _dir) = loaded(
            "gen_player_level_change",
            "horizon.on(\"player_level_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from == 9, \"from\")\n    assert(event.to == 10, \"to\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_level_change(&mut FakeHost::empty(), &mods, 7, "test:name", 9, 10);
        no_error(&id);
    }

    #[test]
    fn generated_player_locale_change() {
        let (m, id, _dir) = loaded(
            "gen_player_locale_change",
            "horizon.on(\"player_locale_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.locale == \"test:locale\", \"locale\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_locale_change(&mut FakeHost::empty(), &mods, 7, "test:name", "test:locale");
        no_error(&id);
    }

    #[test]
    fn generated_player_loom_pattern_select() {
        let (m, id, _dir) = loaded(
            "gen_player_loom_pattern_select",
            "horizon.on(\"player_loom_pattern_select\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pattern == 9, \"pattern\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_loom_pattern_select(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        no_error(&id);
    }

    #[test]
    fn generated_player_map_filled() {
        let (m, id, _dir) = loaded(
            "gen_player_map_filled",
            "horizon.on(\"player_map_filled\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.map_id == 9, \"map_id\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_map_filled(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        no_error(&id);
    }

    #[test]
    fn generated_player_name_entity() {
        let (m, id, _dir) = loaded(
            "gen_player_name_entity",
            "horizon.on(\"player_name_entity\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.entity_name == \"test:entity_name\", \"entity_name\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_name_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
                "test:entity_name",
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_name_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
                "test:entity_name",
            );
        }
        no_error(&id);
        dispatch_player_name_entity(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            11,
            "test:entity_name",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_open_sign() {
        let (m, id, _dir) = loaded(
            "gen_player_open_sign",
            "horizon.on(\"player_open_sign\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.front == false, \"front\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_open_sign(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            false,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_pick_item() {
        let (m, id, _dir) = loaded(
            "gen_player_pick_item",
            "horizon.on(\"player_pick_item\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from_entity == true, \"from_entity\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    assert(event.entity_id == 11, \"kind\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_player_pick_item(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                true,
                [4, 5, 6],
                13,
                &mut cancel,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_player_pick_item(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                true,
                [4, 5, 6],
                99,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_player_pick_item(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            true,
            [4, 5, 6],
            11,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_pickup_arrow() {
        let (m, id, _dir) = loaded(
            "gen_player_pickup_arrow",
            "horizon.on(\"player_pickup_arrow\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.item == \"test:item\", \"item\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_pickup_arrow(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
                "test:item",
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_pickup_arrow(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
                "test:item",
            );
        }
        no_error(&id);
        dispatch_player_pickup_arrow(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            11,
            "test:item",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_portal() {
        let (m, id, _dir) = loaded(
            "gen_player_portal",
            "horizon.on(\"player_portal\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.from_dimension == \"test:from_dimension\", \"from_dimension\")\n    assert(event.to_dimension == \"test:to_dimension\", \"to_dimension\")\n    assert(event.cause == \"test:cause\", \"cause\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_portal(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:from_dimension",
            "test:to_dimension",
            "test:cause",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_ready_arrow() {
        let (m, id, _dir) = loaded(
            "gen_player_ready_arrow",
            "horizon.on(\"player_ready_arrow\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.arrow == \"test:arrow\", \"arrow\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_ready_arrow(&mut FakeHost::empty(), &mods, 7, "test:name", "test:arrow");
        no_error(&id);
    }

    #[test]
    fn generated_player_recipe_book_click() {
        let (m, id, _dir) = loaded(
            "gen_player_recipe_book_click",
            "horizon.on(\"player_recipe_book_click\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.recipe == \"test:recipe\", \"recipe\")\n    assert(event.all == false, \"all\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_recipe_book_click(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:recipe",
            false,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_recipe_book_settings_change() {
        let (m, id, _dir) = loaded(
            "gen_player_recipe_book_settings_change",
            "horizon.on(\"player_recipe_book_settings_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.book == 9, \"book\")\n    assert(event.open == false, \"open\")\n    assert(event.filtering == true, \"filtering\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_recipe_book_settings_change(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            false,
            true,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_recipe_discovered() {
        let (m, id, _dir) = loaded(
            "gen_player_recipe_discovered",
            "horizon.on(\"player_recipe_discovered\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.recipe == \"test:recipe\", \"recipe\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_recipe_discovered(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:recipe",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_riptide() {
        let (m, id, _dir) = loaded(
            "gen_player_riptide",
            "horizon.on(\"player_riptide\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_riptide(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_shear_entity() {
        let (m, id, _dir) = loaded(
            "gen_player_shear_entity",
            "horizon.on(\"player_shear_entity\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.off_hand == false, \"off_hand\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_player_shear_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
                false,
                &mut cancel,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_player_shear_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
                false,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_player_shear_entity(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            11,
            false,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_spawn_point_set() {
        let (m, id, _dir) = loaded(
            "gen_player_spawn_point_set",
            "horizon.on(\"player_spawn_point_set\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    assert(event.cause == \"test:cause\", \"cause\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_spawn_point_set(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:dimension",
            [4, 5, 6],
            "test:cause",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_stat_award() {
        let (m, id, _dir) = loaded(
            "gen_player_stat_award",
            "horizon.on(\"player_stat_award\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.stat_type == \"test:stat_type\", \"stat_type\")\n    assert(event.stat == \"test:stat\", \"stat\")\n    assert(event.value == 11, \"value\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_stat_award(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:stat_type",
            "test:stat",
            11,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_stonecutter_recipe_select() {
        let (m, id, _dir) = loaded(
            "gen_player_stonecutter_recipe_select",
            "horizon.on(\"player_stonecutter_recipe_select\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.recipe == 9, \"recipe\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_stonecutter_recipe_select(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        no_error(&id);
    }

    #[test]
    fn generated_player_stop_spectating_entity() {
        let (m, id, _dir) = loaded(
            "gen_player_stop_spectating_entity",
            "horizon.on(\"player_stop_spectating_entity\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_stop_spectating_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_stop_spectating_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
            );
        }
        no_error(&id);
        dispatch_player_stop_spectating_entity(&mut FakeHost::empty(), &mods, 7, "test:name", 11);
        no_error(&id);
    }

    #[test]
    fn generated_player_stop_using_item() {
        let (m, id, _dir) = loaded(
            "gen_player_stop_using_item",
            "horizon.on(\"player_stop_using_item\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_stop_using_item(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_swap_with_equipment_slot() {
        let (m, id, _dir) = loaded(
            "gen_player_swap_with_equipment_slot",
            "horizon.on(\"player_swap_with_equipment_slot\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.slot == \"test:slot\", \"slot\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_swap_with_equipment_slot(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:slot",
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_teleport_end_gateway() {
        let (m, id, _dir) = loaded(
            "gen_player_teleport_end_gateway",
            "horizon.on(\"player_teleport_end_gateway\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.gateway.x == 3, \"gateway\")\n    assert(event.gateway.y == 4, \"gateway\")\n    assert(event.gateway.z == 5, \"gateway\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_teleport_end_gateway(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_toggle_entity_age_lock() {
        let (m, id, _dir) = loaded(
            "gen_player_toggle_entity_age_lock",
            "horizon.on(\"player_toggle_entity_age_lock\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.locked == false, \"locked\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_toggle_entity_age_lock(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                13,
                false,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_toggle_entity_age_lock(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                99,
                false,
            );
        }
        no_error(&id);
        dispatch_player_toggle_entity_age_lock(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            11,
            false,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_toggle_sneak() {
        let (m, id, _dir) = loaded(
            "gen_player_toggle_sneak",
            "horizon.on(\"player_toggle_sneak\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.sneaking == true, \"sneaking\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_toggle_sneak(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        no_error(&id);
    }

    #[test]
    fn generated_player_trade() {
        let (m, id, _dir) = loaded(
            "gen_player_trade",
            "horizon.on(\"player_trade\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_trade(&mut FakeHost::empty(), &mods, 7, "test:name", 13);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_trade(&mut FakeHost::empty(), &mods, 7, "test:name", 99);
        }
        no_error(&id);
        dispatch_player_trade(&mut FakeHost::empty(), &mods, 7, "test:name", 11);
        no_error(&id);
    }

    #[test]
    fn generated_player_unleash_entity() {
        let (m, id, _dir) = loaded(
            "gen_player_unleash_entity",
            "horizon.on(\"player_unleash_entity\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.entity_id == 11, \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_player_unleash_entity(&mut FakeHost::empty(), &mods, 7, "test:name", 13);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_player_unleash_entity(&mut FakeHost::empty(), &mods, 7, "test:name", 99);
        }
        no_error(&id);
        dispatch_player_unleash_entity(&mut FakeHost::empty(), &mods, 7, "test:name", 11);
        no_error(&id);
    }

    #[test]
    fn generated_player_velocity() {
        let (m, id, _dir) = loaded(
            "gen_player_velocity",
            "horizon.on(\"player_velocity\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.velocity.x == 3, \"velocity\")\n    assert(event.velocity.y == 2.0, \"velocity\")\n    assert(event.velocity.z == 3.0, \"velocity\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_velocity(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3.0, 2.0, 3.0],
        );
        no_error(&id);
    }

    #[test]
    fn generated_portal_create() {
        let (m, id, _dir) = loaded(
            "gen_portal_create",
            "horizon.on(\"portal_create\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_portal_create(&mut FakeHost::empty(), &mods, "test:dimension", [2, 3, 4]);
        no_error(&id);
    }

    #[test]
    fn generated_potion_splash() {
        let (m, id, _dir) = loaded(
            "gen_potion_splash",
            "horizon.on(\"potion_splash\", function(event)\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_potion_splash(&mut FakeHost::empty(), &mods, "test:item", [2.0, 2.0, 3.0]);
        no_error(&id);
    }

    #[test]
    fn generated_prepare_grindstone() {
        let (m, id, _dir) = loaded(
            "gen_prepare_grindstone",
            "horizon.on(\"prepare_grindstone\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.result == nil, \"replace seed\")\n    event.result = { id = \"test:result\", count = 1 }\nend)",
        );
        let mods = boxed(m);
        let mut result: Option<ItemStack> = None;
        dispatch_prepare_grindstone(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &mut result,
        );
        let replaced = result.expect("a replaced value");
        assert_eq!(&*replaced.item, "test:result");
        assert_eq!(replaced.count, 1);
        no_error(&id);
    }

    #[test]
    fn generated_prepare_hopper_transfer() {
        let (m, id, _dir) = loaded(
            "gen_prepare_hopper_transfer",
            "horizon.on(\"prepare_hopper_transfer\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_prepare_hopper_transfer(&FakeHost::empty(), &mods, [1, 2, 3]);
        no_error(&id);
    }

    #[test]
    fn generated_prepare_item_craft() {
        let (m, id, _dir) = loaded(
            "gen_prepare_item_craft",
            "horizon.on(\"prepare_item_craft\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.result == nil, \"replace seed\")\n    event.result = { id = \"test:result\", count = 1 }\nend)",
        );
        let mods = boxed(m);
        let mut result: Option<ItemStack> = None;
        dispatch_prepare_item_craft(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            Some([3, 4, 5]),
            &mut result,
        );
        let replaced = result.expect("a replaced value");
        assert_eq!(&*replaced.item, "test:result");
        assert_eq!(replaced.count, 1);
        no_error(&id);
    }

    #[test]
    fn generated_prepare_item_enchant() {
        let (m, id, _dir) = loaded(
            "gen_prepare_item_enchant",
            "horizon.on(\"prepare_item_enchant\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.costs.x == 4, \"costs\")\n    assert(event.costs.y == 5, \"costs\")\n    assert(event.costs.z == 6, \"costs\")\n    event.costs = { x = 14, y = 15, z = 16 }\nend)",
        );
        let mods = boxed(m);
        let mut costs = [4, 5, 6];
        dispatch_prepare_item_enchant(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &mut costs,
        );
        assert_eq!(costs, [14, 15, 16]);
        no_error(&id);
    }

    #[test]
    fn generated_prepare_smithing() {
        let (m, id, _dir) = loaded(
            "gen_prepare_smithing",
            "horizon.on(\"prepare_smithing\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.result == nil, \"replace seed\")\n    event.result = { id = \"test:result\", count = 1 }\nend)",
        );
        let mods = boxed(m);
        let mut result: Option<ItemStack> = None;
        dispatch_prepare_smithing(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &mut result,
        );
        let replaced = result.expect("a replaced value");
        assert_eq!(&*replaced.item, "test:result");
        assert_eq!(replaced.count, 1);
        no_error(&id);
    }

    #[test]
    fn generated_raid_spawn_wave() {
        let (m, id, _dir) = loaded(
            "gen_raid_spawn_wave",
            "horizon.on(\"raid_spawn_wave\", function(event)\n    assert(event.raid == 7, \"raid\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.wave == 9, \"wave\")\n    assert(event.raiders == 10, \"raiders\")\nend)",
        );
        let mods = boxed(m);
        dispatch_raid_spawn_wave(&mut FakeHost::empty(), &mods, 7, [2, 3, 4], 9, 10);
        no_error(&id);
    }

    #[test]
    fn generated_raid_state_change() {
        let (m, id, _dir) = loaded(
            "gen_raid_state_change",
            "horizon.on(\"raid_state_change\", function(event)\n    assert(event.raid == 7, \"raid\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.state == \"test:state\", \"state\")\nend)",
        );
        let mods = boxed(m);
        dispatch_raid_state_change(&mut FakeHost::empty(), &mods, 7, [2, 3, 4], "test:state");
        no_error(&id);
    }

    #[test]
    fn generated_sculk_bloom() {
        let (m, id, _dir) = loaded(
            "gen_sculk_bloom",
            "horizon.on(\"sculk_bloom\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_sculk_bloom(&mut FakeHost::empty(), &mods, "test:dimension", [2, 3, 4]);
        no_error(&id);
    }

    #[test]
    fn generated_sleep_finished_time() {
        let (m, id, _dir) = loaded(
            "gen_sleep_finished_time",
            "horizon.on(\"sleep_finished_time\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.current == 8, \"current\")\n    assert(event.new_time == 9, \"new_time\")\n    event.new_time = 52\nend)",
        );
        let mods = boxed(m);
        let mut new_time = 9;
        dispatch_sleep_finished_time(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            &mut new_time,
        );
        assert_eq!(new_time, 52);
        no_error(&id);
    }

    #[test]
    fn generated_sponge_absorb() {
        let (m, id, _dir) = loaded(
            "gen_sponge_absorb",
            "horizon.on(\"sponge_absorb\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\nend)",
        );
        let mods = boxed(m);
        dispatch_sponge_absorb(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
        );
        no_error(&id);
    }

    #[test]
    fn generated_target_hit() {
        let (m, id, _dir) = loaded(
            "gen_target_hit",
            "horizon.on(\"target_hit\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.strength == 9, \"strength\")\nend)",
        );
        let mods = boxed(m);
        dispatch_target_hit(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            9,
        );
        no_error(&id);
    }

    #[test]
    fn generated_thunder_change() {
        let (m, id, _dir) = loaded(
            "gen_thunder_change",
            "horizon.on(\"thunder_change\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.thundering == false, \"thundering\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_thunder_change(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            false,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_time_skip() {
        let (m, id, _dir) = loaded(
            "gen_time_skip",
            "horizon.on(\"time_skip\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.from == 8, \"from\")\n    assert(event.to == 9, \"to\")\n    assert(event.cause == \"test:cause\", \"cause\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_time_skip(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            9,
            "test:cause",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_trade_select() {
        let (m, id, _dir) = loaded(
            "gen_trade_select",
            "horizon.on(\"trade_select\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.villager == 9, \"villager\")\n    assert(event.index == 10, \"index\")\nend)",
        );
        let mods = boxed(m);
        dispatch_trade_select(&mut FakeHost::empty(), &mods, 7, "test:name", Some(9), 10);
        no_error(&id);
    }

    #[test]
    fn generated_vault_change_state() {
        let (m, id, _dir) = loaded(
            "gen_vault_change_state",
            "horizon.on(\"vault_change_state\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.state == \"test:state\", \"state\")\nend)",
        );
        let mods = boxed(m);
        dispatch_vault_change_state(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            [2, 3, 4],
            "test:state",
        );
        no_error(&id);
    }

    #[test]
    fn generated_vault_loot() {
        let (m, id, _dir) = loaded(
            "gen_vault_loot",
            "horizon.on(\"vault_loot\", function(event)\n    assert(event.pos.x == 1, \"pos\")\n    assert(event.pos.y == 2, \"pos\")\n    assert(event.pos.z == 3, \"pos\")\n    assert(event.item == \"test:item\", \"item\")\nend)",
        );
        let mods = boxed(m);
        dispatch_vault_loot(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        no_error(&id);
    }

    #[test]
    fn generated_weather_change() {
        let (m, id, _dir) = loaded(
            "gen_weather_change",
            "horizon.on(\"weather_change\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.raining == false, \"raining\")\n    assert(event.thundering == true, \"thundering\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_weather_change(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            false,
            true,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_whitelist_state_update() {
        let (m, id, _dir) = loaded(
            "gen_whitelist_state_update",
            "horizon.on(\"whitelist_state_update\", function(event)\n    assert(event.enabled == true, \"enabled\")\nend)",
        );
        let mods = boxed(m);
        dispatch_whitelist_state_update(&mut FakeHost::empty(), &mods, true);
        no_error(&id);
    }

    #[test]
    fn generated_world_border_bounds_change_finish() {
        let (m, id, _dir) = loaded(
            "gen_world_border_bounds_change_finish",
            "horizon.on(\"world_border_bounds_change_finish\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_world_border_bounds_change_finish(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_world_saved() {
        let (m, id, _dir) = loaded(
            "gen_world_saved",
            "horizon.on(\"world_saved\", function(event)\n    assert(event.wait == true, \"wait\")\nend)",
        );
        let mods = boxed(m);
        dispatch_world_saved(&mut FakeHost::empty(), &mods, true);
        no_error(&id);
    }

    #[test]
    fn generated_living_drown() {
        let (m, id, _dir) = loaded(
            "gen_living_drown",
            "horizon.on(\"living_drown\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.is_drowning == true, \"is_drowning\")\n    event.is_drowning = false\n    assert(event.damage_amount == 4.5, \"damage_amount\")\n    event.damage_amount = 18.5\n    assert(event.bubble_count == 11, \"bubble_count\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut is_drowning = true;
            let mut damage_amount = 4.5;
            let mut cancel = false;
            dispatch_living_drown(
                &mods,
                7,
                "minecraft:skeleton",
                &mut is_drowning,
                &mut damage_amount,
                11,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut is_drowning = true;
        let mut damage_amount = 4.5;
        let mut cancel = false;
        dispatch_living_drown(
            &mods,
            7,
            "minecraft:zombie",
            &mut is_drowning,
            &mut damage_amount,
            11,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert!(!is_drowning);
        assert_eq!(damage_amount, 18.5);
        no_error(&id);
    }

    #[test]
    fn generated_living_jump() {
        let (m, id, _dir) = loaded(
            "gen_living_jump",
            "horizon.on(\"living_jump\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_living_jump(&mods, 7, "minecraft:skeleton");
        }
        no_error(&id);
        dispatch_living_jump(&mods, 7, "minecraft:zombie");
        no_error(&id);
    }

    #[test]
    fn generated_baby_entity_spawn() {
        let (m, id, _dir) = loaded(
            "gen_baby_entity_spawn",
            "horizon.on(\"baby_entity_spawn\", function(event)\n    assert(event.parent == 7, \"parent\")\n    assert(event.partner == 8, \"partner\")\n    assert(event.player == 9, \"player\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_baby_entity_spawn(
                &mut FakeHost::empty(),
                &mods,
                7,
                8,
                9,
                "minecraft:skeleton",
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_baby_entity_spawn(
            &mut FakeHost::empty(),
            &mods,
            7,
            8,
            9,
            "minecraft:zombie",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_can_player_sleep() {
        let (m, id, _dir) = loaded(
            "gen_can_player_sleep",
            "horizon.on(\"can_player_sleep\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pos.x == 3, \"pos\")\n    assert(event.pos.y == 4, \"pos\")\n    assert(event.pos.z == 5, \"pos\")\n    assert(event.problem == \"test:problem\", \"problem\")\n    event.problem = \"test:problem:edited\"\nend)",
        );
        let mods = boxed(m);
        let mut problem = Some("test:problem".to_owned());
        dispatch_can_player_sleep(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            [3, 4, 5],
            &mut problem,
        );
        assert_eq!(problem, Some("test:problem:edited".to_owned()));
        no_error(&id);
    }

    #[test]
    fn generated_item_container() {
        let (m, id, _dir) = loaded(
            "gen_item_container",
            "horizon.on(\"item_container\", function(event)\n    assert(event.item == \"test:item\", \"item\")\n    assert(event.slots == nil, \"replace seed\")\n    event.slots = 81000\nend)",
        );
        let mods = boxed(m);
        let mut slots: Option<u32> = None;
        dispatch_item_container(&FakeHost::empty(), &mods, "test:item", &mut slots);
        assert_eq!(slots, Some(81000));
        no_error(&id);
    }

    #[test]
    fn generated_living_conversion_post() {
        let (m, id, _dir) = loaded(
            "gen_living_conversion_post",
            "horizon.on(\"living_conversion_post\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.converted == 9, \"converted\")\n    assert(event.converted_kind == \"test:converted_kind\", \"converted_kind\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_living_conversion_post(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                9,
                "test:converted_kind",
            );
        }
        no_error(&id);
        dispatch_living_conversion_post(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            9,
            "test:converted_kind",
        );
        no_error(&id);
    }

    #[test]
    fn generated_living_conversion_pre() {
        let (m, id, _dir) = loaded(
            "gen_living_conversion_pre",
            "horizon.on(\"living_conversion_pre\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.outcome == \"test:outcome\", \"outcome\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_living_conversion_pre(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                "test:outcome",
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_living_conversion_pre(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            "test:outcome",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_living_death() {
        let (m, id, _dir) = loaded(
            "gen_living_death",
            "horizon.on(\"living_death\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.damage_type == \"test:damage_type\", \"damage_type\")\n    assert(event.attacker == 10, \"attacker\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_living_death(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                "test:damage_type",
                10,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_living_death(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            "test:damage_type",
            10,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_living_knockback_v2() {
        let (m, id, _dir) = loaded(
            "gen_living_knockback_v2",
            "horizon.on(\"living_knockback_v2\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.attacker == 9, \"attacker\")\n    assert(event.attacker_main.id == \"test:attacker_main\", \"attacker_main\")\n    assert(event.attacker_off.id == \"test:attacker_off\", \"attacker_off\")\n    assert(event.strength == 6.5, \"strength\")\n    event.strength = 20.5\n    assert(event.x == 7.5, \"x\")\n    event.x = 21.5\n    assert(event.z == 8.5, \"z\")\n    event.z = 22.5\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut strength = 6.5;
            let mut x = 7.5;
            let mut z = 8.5;
            let mut canceled = false;
            dispatch_living_knockback_v2(
                &mods,
                7,
                "minecraft:skeleton",
                9,
                Some(&ItemStack {
                    item: "test:attacker_main".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                Some(&ItemStack {
                    item: "test:attacker_off".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                &mut strength,
                &mut x,
                &mut z,
                &mut canceled,
            );
        }
        no_error(&id);
        let mut strength = 6.5;
        let mut x = 7.5;
        let mut z = 8.5;
        let mut canceled = false;
        dispatch_living_knockback_v2(
            &mods,
            7,
            "minecraft:zombie",
            9,
            Some(&ItemStack {
                item: "test:attacker_main".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
            Some(&ItemStack {
                item: "test:attacker_off".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
            &mut strength,
            &mut x,
            &mut z,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        assert_eq!(strength, 20.5);
        assert_eq!(x, 21.5);
        assert_eq!(z, 22.5);
        no_error(&id);
    }

    #[test]
    fn generated_mob_split() {
        let (m, id, _dir) = loaded(
            "gen_mob_split",
            "horizon.on(\"mob_split\", function(event)\n    assert(event.parent == 7, \"parent\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.size == 9, \"size\")\n    assert(event.child_size == 10, \"child_size\")\n    assert(event.count == 11, \"count\")\n    event.count = 54\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut count = 11;
            let mut cancel = false;
            dispatch_mob_split(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                9,
                10,
                &mut count,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut count = 11;
        let mut cancel = false;
        dispatch_mob_split(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            9,
            10,
            &mut count,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(count, 54);
        no_error(&id);
    }

    #[test]
    fn generated_movement_input() {
        let (m, id, _dir) = loaded(
            "gen_movement_input",
            "horizon.on(\"movement_input\", function(event)\n    assert(event.forward == 1.5, \"forward\")\n    event.forward = 15.5\n    assert(event.strafe == 2.5, \"strafe\")\n    event.strafe = 16.5\n    assert(event.jump == true, \"jump\")\n    event.jump = false\n    assert(event.sneak == false, \"sneak\")\n    event.sneak = true\nend)",
        );
        let mods = boxed(m);
        let mut forward = 1.5;
        let mut strafe = 2.5;
        let mut jump = true;
        let mut sneak = false;
        dispatch_movement_input(
            &FakeHost::empty(),
            &mods,
            &mut forward,
            &mut strafe,
            &mut jump,
            &mut sneak,
        );
        assert_eq!(forward, 15.5);
        assert_eq!(strafe, 16.5);
        assert!(!jump);
        assert!(sneak);
        no_error(&id);
    }

    #[test]
    fn generated_village_siege() {
        let (m, id, _dir) = loaded(
            "gen_village_siege",
            "horizon.on(\"village_siege\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.player == 8, \"player\")\n    assert(event.attempted_pos.x == 3, \"attempted_pos\")\n    assert(event.attempted_pos.y == 2.0, \"attempted_pos\")\n    assert(event.attempted_pos.z == 3.0, \"attempted_pos\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_village_siege(
            &mut FakeHost::empty(),
            &mods,
            "test:dimension",
            8,
            [3.0, 2.0, 3.0],
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_xp_change() {
        let (m, id, _dir) = loaded(
            "gen_player_xp_change",
            "horizon.on(\"player_xp_change\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.amount == 9, \"amount\")\n    event.amount = 52\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut amount = 9;
        let mut cancel = false;
        dispatch_player_xp_change(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &mut amount,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(amount, 52);
        no_error(&id);
    }

    #[test]
    fn generated_render_item_in_frame() {
        let (m, id, _dir) = loaded(
            "gen_render_item_in_frame",
            "horizon.on(\"render_item_in_frame\", function(event)\n    assert(event.entity_id == 11, \"kind\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut canceled = false;
            dispatch_render_item_in_frame(
                &FakeHost::empty(),
                &mods,
                13,
                [2, 3, 4],
                &ItemStack {
                    item: "test:stack".into(),
                    count: 3,
                    damage: 2,
                    extra: None,
                },
                &mut canceled,
            );
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            let mut canceled = false;
            dispatch_render_item_in_frame(
                &FakeHost::empty(),
                &mods,
                99,
                [2, 3, 4],
                &ItemStack {
                    item: "test:stack".into(),
                    count: 3,
                    damage: 2,
                    extra: None,
                },
                &mut canceled,
            );
        }
        no_error(&id);
        let mut canceled = false;
        dispatch_render_item_in_frame(
            &FakeHost::empty(),
            &mods,
            11,
            [2, 3, 4],
            &ItemStack {
                item: "test:stack".into(),
                count: 3,
                damage: 2,
                extra: None,
            },
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_render_name_tag() {
        let (m, id, _dir) = loaded(
            "gen_render_name_tag",
            "horizon.on(\"render_name_tag\", function(event)\n    assert(event.entity_id == 7, \"entity_id\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.vanilla_visible == true, \"vanilla_visible\")\n    assert(event.visible == false, \"visible\")\n    event.visible = true\n    assert(event.text == \"test:text\", \"text\")\n    event.text = \"test:text:edited\"\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut visible = false;
            let mut text = Some("test:text".to_owned());
            dispatch_render_name_tag(
                &FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                true,
                &mut visible,
                &mut text,
            );
        }
        no_error(&id);
        let mut visible = false;
        let mut text = Some("test:text".to_owned());
        dispatch_render_name_tag(
            &FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            true,
            &mut visible,
            &mut text,
        );
        assert!(visible);
        assert_eq!(text, Some("test:text:edited".to_owned()));
        no_error(&id);
    }

    #[test]
    fn generated_tooltip_color() {
        let (m, id, _dir) = loaded(
            "gen_tooltip_color",
            "horizon.on(\"tooltip_color\", function(event)\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.background == 8, \"background\")\n    event.background = 51\n    assert(event.border_top == 9, \"border_top\")\n    event.border_top = 52\n    assert(event.border_bottom == 10, \"border_bottom\")\n    event.border_bottom = 53\nend)",
        );
        let mods = boxed(m);
        let mut background = 8;
        let mut border_top = 9;
        let mut border_bottom = 10;
        dispatch_tooltip_color(
            &FakeHost::empty(),
            &mods,
            Some(&ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
            &mut background,
            &mut border_top,
            &mut border_bottom,
        );
        assert_eq!(background, 51);
        assert_eq!(border_top, 52);
        assert_eq!(border_bottom, 53);
        no_error(&id);
    }

    #[test]
    fn generated_camera_pose() {
        let (m, id, _dir) = loaded(
            "gen_camera_pose",
            "horizon.on(\"camera_pose\", function(event)\n    assert(event.partial == 1.5, \"partial\")\n    assert(event.position.x == 2, \"position\")\n    assert(event.position.y == 2.0, \"position\")\n    assert(event.position.z == 3.0, \"position\")\n    event.position = { x = 12.0, y = 2.0, z = 3.0 }\n    assert(event.yaw == 3.5, \"yaw\")\n    event.yaw = 17.5\n    assert(event.pitch == 4.5, \"pitch\")\n    event.pitch = 18.5\nend)",
        );
        let mods = boxed(m);
        let mut position = [2.0, 2.0, 3.0];
        let mut yaw = 3.5;
        let mut pitch = 4.5;
        dispatch_camera_pose(
            &FakeHost::empty(),
            &mods,
            1.5,
            &mut position,
            &mut yaw,
            &mut pitch,
        );
        assert_eq!(position, [12.0, 2.0, 3.0]);
        assert_eq!(yaw, 17.5);
        assert_eq!(pitch, 18.5);
        no_error(&id);
    }

    #[test]
    fn generated_client_start() {
        let (m, id, _dir) = loaded(
            "gen_client_start",
            "horizon.on(\"client_start\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_start(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_client_disconnecting() {
        let (m, id, _dir) = loaded(
            "gen_client_disconnecting",
            "horizon.on(\"client_disconnecting\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_disconnecting(&FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_client_stopping() {
        let (m, id, _dir) = loaded(
            "gen_client_stopping",
            "horizon.on(\"client_stopping\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_stopping(&FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_menu_background() {
        let (m, id, _dir) = loaded(
            "gen_menu_background",
            "horizon.on(\"menu_background\", function(event)\n    assert(event.radius == 1.5, \"radius\")\n    event.radius = 15.5\nend)",
        );
        let mods = boxed(m);
        let mut radius = Some(1.5);
        dispatch_menu_background(&FakeHost::empty(), &mods, &mut radius);
        assert_eq!(radius, Some(15.5));
        no_error(&id);
    }

    #[test]
    fn generated_block_entity_accepts_fluid() {
        let (m, id, _dir) = loaded(
            "gen_block_entity_accepts_fluid",
            "horizon.on(\"block_entity_accepts_fluid\", function(event)\n    assert(event.block == \"test:block\", \"block\")\n    assert(event.tank == 8, \"tank\")\n    assert(event.fluid == \"test:fluid\", \"fluid\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_block_entity_accepts_fluid(
            &FakeHost::empty(),
            &mods,
            "test:block",
            8,
            "test:fluid",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_block_place() {
        let (m, id, _dir) = loaded(
            "gen_block_place",
            "horizon.on(\"block_place\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    assert(event.state == \"test:state12\", \"state\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_block_place(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &ItemStack {
                item: "test:stack".into(),
                count: 3,
                damage: 2,
                extra: None,
            },
            [4, 5, 6],
            BlockStateId(12),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_block_redstone() {
        let (m, id, _dir) = loaded(
            "gen_block_redstone",
            "horizon.on(\"block_redstone\", function(event)\n    assert(event.dimension == \"minecraft:overworld\", \"dimension\")\n    assert(event.pos.x == 2, \"pos\")\n    assert(event.pos.y == 3, \"pos\")\n    assert(event.pos.z == 4, \"pos\")\n    assert(event.old == 9, \"old\")\n    assert(event.new == 10, \"new\")\n    event.new = 53\nend)",
        );
        let mods = boxed(m);
        let mut new = 10;
        dispatch_block_redstone(
            &FakeLevel,
            &mods,
            LevelDimension::Overworld,
            BlockPos::new(2, 3, 4),
            9,
            &mut new,
        );
        assert_eq!(new, 53);
        no_error(&id);
    }

    #[test]
    fn generated_chat_suggestion_applied() {
        let (m, id, _dir) = loaded(
            "gen_chat_suggestion_applied",
            "horizon.on(\"chat_suggestion_applied\", function(event)\n    assert(event.original == \"test:original\", \"original\")\n    assert(event.start == 8, \"start\")\n    assert(event[\"end\"] == 9, \"end\")\n    assert(event.suggestion == \"test:suggestion\", \"suggestion\")\n    assert(event.input == \"test:input\", \"input\")\nend)",
        );
        let mods = boxed(m);
        dispatch_chat_suggestion_applied(
            &FakeHost::empty(),
            &mods,
            "test:original",
            8,
            9,
            "test:suggestion",
            "test:input",
        );
        no_error(&id);
    }

    #[test]
    fn generated_client_attack() {
        let (m, id, _dir) = loaded(
            "gen_client_attack",
            "horizon.on(\"client_attack\", function(event)\n    assert(event.target == 11, \"kind\")\n    assert(event.player == false, \"player\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_client_attack(&mut FakeHost::empty(), &mods, 13, false);
        }
        no_error(&id);
        // gone event: the handler stays quiet.
        {
            dispatch_client_attack(&mut FakeHost::empty(), &mods, 99, false);
        }
        no_error(&id);
        dispatch_client_attack(&mut FakeHost::empty(), &mods, 11, false);
        no_error(&id);
    }

    #[test]
    fn generated_client_hurt() {
        let (m, id, _dir) = loaded(
            "gen_client_hurt",
            "horizon.on(\"client_hurt\", function(event)\n    assert(event.attacker == 7, \"attacker\")\n    assert(event.player == false, \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_client_hurt(&mut FakeHost::empty(), &mods, Some(7), false);
        no_error(&id);
    }

    #[test]
    fn generated_client_player_network_logging_in() {
        let (m, id, _dir) = loaded(
            "gen_client_player_network_logging_in",
            "horizon.on(\"client_player_network_logging_in\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_player_network_logging_in(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_client_tick_pre() {
        let (m, id, _dir) = loaded(
            "gen_client_tick_pre",
            "horizon.on(\"client_tick_pre\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_tick_pre(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_client_tick_post() {
        let (m, id, _dir) = loaded(
            "gen_client_tick_post",
            "horizon.on(\"client_tick_post\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_client_tick_post(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_creative_slot_v2() {
        let (m, id, _dir) = loaded(
            "gen_creative_slot_v2",
            "horizon.on(\"creative_slot_v2\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.slot == 9, \"slot\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_creative_slot_v2(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            Some(&ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_custom_block_fluids() {
        let (m, id, _dir) = loaded(
            "gen_custom_block_fluids",
            "horizon.on(\"custom_block_fluids\", function(event)\n    assert(event.block == \"test:block\", \"block\")\n    assert(event.custom == false, \"custom\")\n    event.custom = true\nend)",
        );
        let mods = boxed(m);
        let mut custom = false;
        dispatch_custom_block_fluids(&FakeHost::empty(), &mods, "test:block", &mut custom);
        assert!(custom);
        no_error(&id);
    }

    #[test]
    fn generated_entity_change_block_v2() {
        let (m, id, _dir) = loaded(
            "gen_entity_change_block_v2",
            "horizon.on(\"entity_change_block_v2\", function(event)\n    assert(event.player == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    assert(event.from == \"test:from\", \"from\")\n    assert(event.to == \"test:to\", \"to\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_entity_change_block_v2(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                Some(8),
                "minecraft:skeleton",
                [4, 5, 6],
                "test:from",
                "test:to",
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_entity_change_block_v2(
            &mut FakeHost::empty(),
            &mods,
            Some(7),
            Some(8),
            "minecraft:zombie",
            [4, 5, 6],
            "test:from",
            "test:to",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_entity_interact_specific() {
        let (m, id, _dir) = loaded(
            "gen_entity_interact_specific",
            "horizon.on(\"entity_interact_specific\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.location.x == 4, \"location\")\n    assert(event.location.y == 2.0, \"location\")\n    assert(event.location.z == 3.0, \"location\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut claimed = false;
            dispatch_entity_interact_specific(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                Some(&ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                [4.0, 2.0, 3.0],
                &mut claimed,
            );
        }
        no_error(&id);
        let mut claimed = false;
        dispatch_entity_interact_specific(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            Some(&ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
            [4.0, 2.0, 3.0],
            &mut claimed,
        );
        assert!(claimed, "claimed not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_entity_outline() {
        let (m, id, _dir) = loaded(
            "gen_entity_outline",
            "horizon.on(\"entity_outline\", function(event)\n    assert(event.entity_id == 7, \"entity_id\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.outline == 9, \"outline\")\n    event.outline = 52\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut outline = Some(9);
            dispatch_entity_outline(
                &FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                &mut outline,
            );
        }
        no_error(&id);
        let mut outline = Some(9);
        dispatch_entity_outline(
            &FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            &mut outline,
        );
        assert_eq!(outline, Some(52));
        no_error(&id);
    }

    #[test]
    fn generated_fishing_hook_landed() {
        let (m, id, _dir) = loaded(
            "gen_fishing_hook_landed",
            "horizon.on(\"fishing_hook_landed\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.hook == 8, \"hook\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.block.x == 4, \"block\")\n    assert(event.block.y == 5, \"block\")\n    assert(event.block.z == 6, \"block\")\n    assert(event.face.x == 5, \"face\")\n    assert(event.face.y == 6, \"face\")\n    assert(event.face.z == 7, \"face\")\nend)",
        );
        let mods = boxed(m);
        dispatch_fishing_hook_landed(
            &mut FakeHost::empty(),
            &mods,
            7,
            8,
            "test:dimension",
            [4, 5, 6],
            [5, 6, 7],
        );
        no_error(&id);
    }

    #[test]
    fn generated_inventory_click() {
        let (m, id, _dir) = loaded(
            "gen_inventory_click",
            "horizon.on(\"inventory_click\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.window == 9, \"window\")\n    assert(event.slot == 10, \"slot\")\n    assert(event.button == 11, \"button\")\n    assert(event.mode == 12, \"mode\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_inventory_click(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            10,
            11,
            12,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_inventory_drag() {
        let (m, id, _dir) = loaded(
            "gen_inventory_drag",
            "horizon.on(\"inventory_drag\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.window == 9, \"window\")\n    assert(event.kind == 10, \"kind\")\n    assert(event.slots[1] == 8 and event.slots[2] == 13, \"slots\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_inventory_drag(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            9,
            10,
            &[8, 13],
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_item_dropped_v2() {
        let (m, id, _dir) = loaded(
            "gen_item_dropped_v2",
            "horizon.on(\"item_dropped_v2\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_item_dropped_v2(
            &mut FakeHost::empty(),
            &mods,
            7,
            &ItemStack {
                item: "test:stack".into(),
                count: 2,
                damage: 1,
                extra: None,
            },
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_item_interact_mob() {
        let (m, id, _dir) = loaded(
            "gen_item_interact_mob",
            "horizon.on(\"item_interact_mob\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.swing == nil, \"replace seed\")\n    event.swing = true\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut swing: Option<bool> = None;
            dispatch_item_interact_mob(
                &mut FakeHost::empty(),
                &mods,
                7,
                "minecraft:skeleton",
                Some(&ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                &mut swing,
            );
        }
        no_error(&id);
        let mut swing: Option<bool> = None;
        dispatch_item_interact_mob(
            &mut FakeHost::empty(),
            &mods,
            7,
            "minecraft:zombie",
            Some(&ItemStack {
                item: "test:stack".into(),
                count: 1,
                damage: 0,
                extra: None,
            }),
            &mut swing,
        );
        assert_eq!(swing, Some(true));
        no_error(&id);
    }

    #[test]
    fn generated_item_picked_up_v2() {
        let (m, id, _dir) = loaded(
            "gen_item_picked_up_v2",
            "horizon.on(\"item_picked_up_v2\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    assert(event.thrower == \"00000000-0000-0000-0000-000000000009\", \"thrower\")\n    assert(event.position.x == 4, \"position\")\n    assert(event.position.y == 2.0, \"position\")\n    assert(event.position.z == 3.0, \"position\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_item_picked_up_v2(
            &mut FakeHost::empty(),
            &mods,
            7,
            &ItemStack {
                item: "test:stack".into(),
                count: 2,
                damage: 1,
                extra: None,
            },
            Some(9),
            [4.0, 2.0, 3.0],
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_living_change_target() {
        let (m, id, _dir) = loaded(
            "gen_living_change_target",
            "horizon.on(\"living_change_target\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.target == \"00000000-0000-0000-0000-00000000000a\", \"target\")\n    assert(event.target_mob == 11, \"target_mob\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_living_change_target(
                &mods,
                "test:dimension",
                8,
                "minecraft:skeleton",
                Some(10),
                Some(11),
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_living_change_target(
            &mods,
            "test:dimension",
            8,
            "minecraft:zombie",
            Some(10),
            Some(11),
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_living_damage_pre() {
        let (m, id, _dir) = loaded(
            "gen_living_damage_pre",
            "horizon.on(\"living_damage_pre\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.damage_type == \"test:damage_type\", \"damage_type\")\n    assert(event.causing == 10, \"causing\")\n    assert(event.direct == 11, \"direct\")\n    assert(event.amount == 6.5, \"amount\")\n    event.amount = 20.5\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut amount = 6.5;
            dispatch_living_damage_pre(
                &mods,
                7,
                "minecraft:skeleton",
                "test:damage_type",
                10,
                11,
                &mut amount,
            );
        }
        no_error(&id);
        let mut amount = 6.5;
        dispatch_living_damage_pre(
            &mods,
            7,
            "minecraft:zombie",
            "test:damage_type",
            10,
            11,
            &mut amount,
        );
        assert_eq!(amount, 20.5);
        no_error(&id);
    }

    #[test]
    fn generated_living_damage_post() {
        let (m, id, _dir) = loaded(
            "gen_living_damage_post",
            "horizon.on(\"living_damage_post\", function(event)\n    assert(event.entity == 7, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.damage_type == \"test:damage_type\", \"damage_type\")\n    assert(event.causing == 10, \"causing\")\n    assert(event.direct == 11, \"direct\")\n    assert(event.amount == 6.5, \"amount\")\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            dispatch_living_damage_post(
                &mods,
                7,
                "minecraft:skeleton",
                "test:damage_type",
                10,
                11,
                6.5,
            );
        }
        no_error(&id);
        dispatch_living_damage_post(
            &mods,
            7,
            "minecraft:zombie",
            "test:damage_type",
            10,
            11,
            6.5,
        );
        no_error(&id);
    }

    #[test]
    fn generated_living_incoming_damage_v3() {
        let (m, id, _dir) = loaded(
            "gen_living_incoming_damage_v3",
            "horizon.on(\"living_incoming_damage_v3\", function(event)\n    assert(event.victim == 7, \"victim\")\n    assert(event.victim_uuid == \"00000000-0000-0000-0000-000000000008\", \"victim_uuid\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.attacker == 10, \"attacker\")\n    assert(event.attacker_uuid == \"00000000-0000-0000-0000-00000000000b\", \"attacker_uuid\")\n    assert(event.direct == 12, \"direct\")\n    assert(event.damage_type == \"test:damage_type\", \"damage_type\")\n    assert(event.amount == 8.5, \"amount\")\n    event.amount = 22.5\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut amount = 8.5;
            let mut cancel = false;
            dispatch_living_incoming_damage_v3(
                &mods,
                7,
                Some(8),
                "minecraft:skeleton",
                Some(10),
                Some(11),
                Some(12),
                "test:damage_type",
                &mut amount,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut amount = 8.5;
        let mut cancel = false;
        dispatch_living_incoming_damage_v3(
            &mods,
            7,
            Some(8),
            "minecraft:zombie",
            Some(10),
            Some(11),
            Some(12),
            "test:damage_type",
            &mut amount,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(amount, 22.5);
        no_error(&id);
    }

    #[test]
    fn generated_log_line() {
        let (m, id, _dir) = loaded(
            "gen_log_line",
            "horizon.on(\"log_line\", function(event)\n    assert(event.level == \"test:level\", \"level\")\n    assert(event.thread == \"test:thread\", \"thread\")\n    assert(event.message == \"test:message\", \"message\")\n    assert(event.line == \"test:line\", \"line\")\nend)",
        );
        let mods = boxed(m);
        dispatch_log_line(
            &mods,
            "test:level",
            "test:thread",
            "test:message",
            "test:line",
        );
        no_error(&id);
    }

    #[test]
    fn generated_permission_check() {
        let (m, id, _dir) = loaded(
            "gen_permission_check",
            "horizon.on(\"permission_check\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.node == \"test:node\", \"node\")\n    assert(event.default_level == 9, \"default_level\")\n    assert(event.result == nil, \"replace seed\")\n    event.result = true\nend)",
        );
        let mods = boxed(m);
        let mut result: Option<bool> = None;
        dispatch_permission_check(&FakeHost::empty(), &mods, 7, "test:node", 9, &mut result);
        assert_eq!(result, Some(true));
        no_error(&id);
    }

    #[test]
    fn generated_player_register_channels() {
        let (m, id, _dir) = loaded(
            "gen_player_register_channels",
            "horizon.on(\"player_register_channels\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.channels[1] == \"test:seed\", \"channels\")\n    assert(event.register == false, \"register\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_register_channels(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &["test:seed".to_owned()],
            false,
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_command_level() {
        let (m, id, _dir) = loaded(
            "gen_player_command_level",
            "horizon.on(\"player_command_level\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.command == \"test:command\", \"command\")\n    assert(event.level == 10, \"level\")\n    event.level = 53\nend)",
        );
        let mods = boxed(m);
        let mut level = 10;
        dispatch_player_command_level(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            "test:command",
            &mut level,
        );
        assert_eq!(level, 53);
        no_error(&id);
    }

    #[test]
    fn generated_player_commands_sent_v2() {
        let (m, id, _dir) = loaded(
            "gen_player_commands_sent_v2",
            "horizon.on(\"player_commands_sent_v2\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.names[1] == \"test:seed\", \"names\")\n    event.names[1] = \"test:edited\"; table.insert(event.names, \"test:added\")\n    assert(event.names_hidden[1] == \"test:seed\", \"names_hidden\")\n    event.names_hidden[1] = \"test:edited\"; table.insert(event.names_hidden, \"test:added\")\nend)",
        );
        let mods = boxed(m);
        let mut names = vec!["test:seed".to_owned()];
        let mut names_hidden = vec!["test:seed".to_owned()];
        dispatch_player_commands_sent_v2(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &mut names,
            &mut names_hidden,
        );
        assert_eq!(
            names,
            vec!["test:edited".to_owned(), "test:added".to_owned()]
        );
        assert_eq!(
            names_hidden,
            vec!["test:edited".to_owned(), "test:added".to_owned()]
        );
        no_error(&id);
    }

    #[test]
    fn generated_player_hurt_by_player() {
        let (m, id, _dir) = loaded(
            "gen_player_hurt_by_player",
            "horizon.on(\"player_hurt_by_player\", function(event)\n    assert(event.attacker == 7, \"attacker\")\n    assert(event.attacker_entity == 8, \"attacker_entity\")\n    assert(event.victim == 9, \"victim\")\n    assert(event.victim_entity == 10, \"victim_entity\")\n    assert(event.source == \"test:source\", \"source\")\n    assert(event.damage_type == \"test:damage_type\", \"damage_type\")\n    assert(event.amount == 7.5, \"amount\")\n    event.amount = 21.5\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut amount = 7.5;
        let mut canceled = false;
        dispatch_player_hurt_by_player(
            &mods,
            7,
            8,
            9,
            10,
            "test:source",
            "test:damage_type",
            &mut amount,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        assert_eq!(amount, 21.5);
        no_error(&id);
    }

    #[test]
    fn generated_player_launch_projectile_v2() {
        let (m, id, _dir) = loaded(
            "gen_player_launch_projectile_v2",
            "horizon.on(\"player_launch_projectile_v2\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.stack.id == \"test:stack\", \"stack\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_player_launch_projectile_v2(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &ItemStack {
                item: "test:stack".into(),
                count: 3,
                damage: 2,
                extra: None,
            },
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_player_logging_out() {
        let (m, id, _dir) = loaded(
            "gen_player_logging_out",
            "horizon.on(\"player_logging_out\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\nend)",
        );
        let mods = boxed(m);
        dispatch_player_logging_out(&mut FakeHost::empty(), &mods, 7, "test:name");
        no_error(&id);
    }

    #[test]
    fn generated_player_move() {
        let (m, id, _dir) = loaded(
            "gen_player_move",
            "horizon.on(\"player_move\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.name == \"test:name\", \"name\")\n    assert(event.from.x == 3, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.to.x == 4, \"to\")\n    assert(event.to.y == 2.0, \"to\")\n    assert(event.to.z == 3.0, \"to\")\n    event.to = { x = 14.0, y = 2.0, z = 3.0 }\n    assert(event.yaw == 5.5, \"yaw\")\n    assert(event.pitch == 6.5, \"pitch\")\n    assert(event.on_ground == true, \"on_ground\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut to = [4.0, 2.0, 3.0];
        let mut cancel = false;
        dispatch_player_move(
            &mods,
            7,
            "test:name",
            [3.0, 2.0, 3.0],
            &mut to,
            5.5,
            6.5,
            true,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(to, [14.0, 2.0, 3.0]);
        no_error(&id);
    }

    #[test]
    fn generated_player_respawn_position() {
        let (m, id, _dir) = loaded(
            "gen_player_respawn_position",
            "horizon.on(\"player_respawn_position\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    event.dimension = \"test:dimension:edited\"\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\n    event.pos = { x = 14.0, y = 2.0, z = 3.0 }\n    assert(event.yaw == 5.5, \"yaw\")\n    event.yaw = 19.5\n    assert(event.pitch == 6.5, \"pitch\")\n    event.pitch = 20.5\nend)",
        );
        let mods = boxed(m);
        let mut dimension = "test:dimension".to_owned();
        let mut pos = [4.0, 2.0, 3.0];
        let mut yaw = 5.5;
        let mut pitch = 6.5;
        dispatch_player_respawn_position(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &mut dimension,
            &mut pos,
            &mut yaw,
            &mut pitch,
        );
        assert_eq!(dimension, "test:dimension:edited".to_owned());
        assert_eq!(pos, [14.0, 2.0, 3.0]);
        assert_eq!(yaw, 19.5);
        assert_eq!(pitch, 20.5);
        no_error(&id);
    }

    #[test]
    fn generated_player_spawn_location() {
        let (m, id, _dir) = loaded(
            "gen_player_spawn_location",
            "horizon.on(\"player_spawn_location\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    event.dimension = \"test:dimension:edited\"\n    assert(event.position.x == 4, \"position\")\n    assert(event.position.y == 2.0, \"position\")\n    assert(event.position.z == 3.0, \"position\")\n    event.position = { x = 14.0, y = 2.0, z = 3.0 }\n    assert(event.rotation.x == 5.5, \"rotation\")\n    assert(event.rotation.y == 6.5, \"rotation\")\n    event.rotation = { x = 15.5, y = 16.5 }\nend)",
        );
        let mods = boxed(m);
        let mut dimension = "test:dimension".to_owned();
        let mut position = [4.0, 2.0, 3.0];
        let mut rotation = [5.5, 6.5];
        dispatch_player_spawn_location(
            &mut FakeHost::empty(),
            &mods,
            7,
            "test:name",
            &mut dimension,
            &mut position,
            &mut rotation,
        );
        assert_eq!(dimension, "test:dimension:edited".to_owned());
        assert_eq!(position, [14.0, 2.0, 3.0]);
        assert_eq!(rotation, [15.5, 16.5]);
        no_error(&id);
    }

    #[test]
    fn generated_player_teleport() {
        let (m, id, _dir) = loaded(
            "gen_player_teleport",
            "horizon.on(\"player_teleport\", function(event)\n    assert(event.player == 7, \"player\")\n    assert(event.name == \"test:name\", \"name\")\n    assert(event.cause == \"test:cause\", \"cause\")\n    assert(event.from_dimension == \"test:from_dimension\", \"from_dimension\")\n    assert(event.from.x == 5, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.to_dimension == \"test:to_dimension\", \"to_dimension\")\n    assert(event.to.x == 7, \"to\")\n    assert(event.to.y == 2.0, \"to\")\n    assert(event.to.z == 3.0, \"to\")\n    event.to = { x = 17.0, y = 2.0, z = 3.0 }\n    assert(event.relative == 14, \"relative\")\n    event.relative = 57\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut to = [7.0, 2.0, 3.0];
        let mut relative = 14;
        let mut cancel = false;
        dispatch_player_teleport(
            &mods,
            7,
            "test:name",
            "test:cause",
            "test:from_dimension",
            [5.0, 2.0, 3.0],
            "test:to_dimension",
            &mut to,
            &mut relative,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(to, [17.0, 2.0, 3.0]);
        assert_eq!(relative, 57);
        no_error(&id);
    }

    #[test]
    fn generated_player_tick_pre() {
        let (m, id, _dir) = loaded(
            "gen_player_tick_pre",
            "horizon.on(\"player_tick_pre\", function(event)\n\nend)",
        );
        let mods = boxed(m);
        dispatch_player_tick_pre(&mut FakeHost::empty(), &mods);
        no_error(&id);
    }

    #[test]
    fn generated_projectile_world_border() {
        let (m, id, _dir) = loaded(
            "gen_projectile_world_border",
            "horizon.on(\"projectile_world_border\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 2.0, \"pos\")\n    assert(event.pos.z == 3.0, \"pos\")\n    event.pos = { x = 14.0, y = 2.0, z = 3.0 }\n    assert(event.velocity.x == 5, \"velocity\")\n    assert(event.velocity.y == 2.0, \"velocity\")\n    assert(event.velocity.z == 3.0, \"velocity\")\n    event.velocity = { x = 15.0, y = 2.0, z = 3.0 }\n    assert(event.collide == false, \"collide\")\n    event.collide = true\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut pos = [4.0, 2.0, 3.0];
            let mut velocity = [5.0, 2.0, 3.0];
            let mut collide = false;
            dispatch_projectile_world_border(
                &mods,
                "test:dimension",
                8,
                "minecraft:skeleton",
                &mut pos,
                &mut velocity,
                &mut collide,
            );
        }
        no_error(&id);
        let mut pos = [4.0, 2.0, 3.0];
        let mut velocity = [5.0, 2.0, 3.0];
        let mut collide = false;
        dispatch_projectile_world_border(
            &mods,
            "test:dimension",
            8,
            "minecraft:zombie",
            &mut pos,
            &mut velocity,
            &mut collide,
        );
        assert_eq!(pos, [14.0, 2.0, 3.0]);
        assert_eq!(velocity, [15.0, 2.0, 3.0]);
        assert!(collide);
        no_error(&id);
    }

    #[test]
    fn generated_resource_pack_status_v2() {
        let (m, id, _dir) = loaded(
            "gen_resource_pack_status_v2",
            "horizon.on(\"resource_pack_status_v2\", function(event)\n    assert(event.player.uuid == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.player.name == \"test:name\", \"player\")\n    assert(event.pack == 9, \"pack\")\n    assert(event.status == 10, \"status\")\nend)",
        );
        let mods = boxed(m);
        dispatch_resource_pack_status_v2(&mut FakeHost::empty(), &mods, 7, "test:name", 9, 10);
        no_error(&id);
    }

    #[test]
    fn generated_screen_mouse_dragged_pre_v2() {
        let (m, id, _dir) = loaded(
            "gen_screen_mouse_dragged_pre_v2",
            "horizon.on(\"screen_mouse_dragged_pre_v2\", function(event)\n    assert(event.screen == \"test:screen\", \"screen\")\n    assert(event.mouse.x == 2.5, \"mouse\")\n    assert(event.mouse.y == 3.5, \"mouse\")\n    assert(event.button == 9, \"button\")\n    assert(event.delta.x == 4.5, \"delta\")\n    assert(event.delta.y == 5.5, \"delta\")\n    assert(event.modifiers == 11, \"modifiers\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_screen_mouse_dragged_pre_v2(
            &mut FakeHost::empty(),
            &mods,
            "test:screen",
            [2.5, 3.5],
            9,
            [4.5, 5.5],
            11,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_screen_mouse_scrolled_pre_v2() {
        let (m, id, _dir) = loaded(
            "gen_screen_mouse_scrolled_pre_v2",
            "horizon.on(\"screen_mouse_scrolled_pre_v2\", function(event)\n    assert(event.screen == \"test:screen\", \"screen\")\n    assert(event.mouse.x == 2.5, \"mouse\")\n    assert(event.mouse.y == 3.5, \"mouse\")\n    assert(event.scroll.x == 3.5, \"scroll\")\n    assert(event.scroll.y == 4.5, \"scroll\")\n    assert(event.modifiers == 10, \"modifiers\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut canceled = false;
        dispatch_screen_mouse_scrolled_pre_v2(
            &mut FakeHost::empty(),
            &mods,
            "test:screen",
            [2.5, 3.5],
            [3.5, 4.5],
            10,
            &mut canceled,
        );
        assert!(canceled, "canceled not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_stateful_block_entity() {
        let (m, id, _dir) = loaded(
            "gen_stateful_block_entity",
            "horizon.on(\"stateful_block_entity\", function(event)\n    assert(event.block == \"test:block\", \"block\")\n    assert(event.stateful == false, \"stateful\")\n    event.stateful = true\nend)",
        );
        let mods = boxed(m);
        let mut stateful = false;
        dispatch_stateful_block_entity(&FakeHost::empty(), &mods, "test:block", &mut stateful);
        assert!(stateful);
        no_error(&id);
    }

    #[test]
    fn generated_toast_add() {
        let (m, id, _dir) = loaded(
            "gen_toast_add",
            "horizon.on(\"toast_add\", function(event)\n    assert(event.kind == \"test:kind\", \"kind\")\n    assert(event.id == \"test:id\", \"id\")\n    horizon.cancel()\nend)",
        );
        let mods = boxed(m);
        let mut cancel = false;
        dispatch_toast_add(
            &mut FakeHost::empty(),
            &mods,
            "test:kind",
            "test:id",
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_trample() {
        let (m, id, _dir) = loaded(
            "gen_trample",
            "horizon.on(\"trample\", function(event)\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.entity == 8, \"entity\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.pos.x == 4, \"pos\")\n    assert(event.pos.y == 5, \"pos\")\n    assert(event.pos.z == 6, \"pos\")\n    assert(event.fall_distance == 5.5, \"fall_distance\")\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut cancel = false;
            dispatch_trample(
                &mods,
                "test:dimension",
                8,
                "minecraft:skeleton",
                [4, 5, 6],
                5.5,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut cancel = false;
        dispatch_trample(
            &mods,
            "test:dimension",
            8,
            "minecraft:zombie",
            [4, 5, 6],
            5.5,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        no_error(&id);
    }

    #[test]
    fn generated_vehicle_moved_v2() {
        let (m, id, _dir) = loaded(
            "gen_vehicle_moved_v2",
            "horizon.on(\"vehicle_moved_v2\", function(event)\n    assert(event.player == \"00000000-0000-0000-0000-000000000007\", \"player\")\n    assert(event.dimension == \"test:dimension\", \"dimension\")\n    assert(event.vehicle == 9, \"vehicle\")\n    assert(event.kind == \"minecraft:zombie\", \"kind\")\n    assert(event.from.x == 5, \"from\")\n    assert(event.from.y == 2.0, \"from\")\n    assert(event.from.z == 3.0, \"from\")\n    assert(event.to.x == 6, \"to\")\n    assert(event.to.y == 2.0, \"to\")\n    assert(event.to.z == 3.0, \"to\")\n    event.to = { x = 16.0, y = 2.0, z = 3.0 }\n    horizon.cancel()\nend, \"minecraft:zombie\")",
        );
        let mods = boxed(m);
        // K2 event: the handler stays quiet.
        {
            let mut to = [6.0, 2.0, 3.0];
            let mut cancel = false;
            dispatch_vehicle_moved_v2(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                "test:dimension",
                9,
                "minecraft:skeleton",
                [5.0, 2.0, 3.0],
                &mut to,
                &mut cancel,
            );
        }
        no_error(&id);
        let mut to = [6.0, 2.0, 3.0];
        let mut cancel = false;
        dispatch_vehicle_moved_v2(
            &mut FakeHost::empty(),
            &mods,
            Some(7),
            "test:dimension",
            9,
            "minecraft:zombie",
            [5.0, 2.0, 3.0],
            &mut to,
            &mut cancel,
        );
        assert!(cancel, "cancel not cancelled");
        assert_eq!(to, [16.0, 2.0, 3.0]);
        no_error(&id);
    }

    #[test]
    fn generated_no_handler_does_no_lua_work() {
        const LUA: &str = "-- nothing registered";
        let (m, id, dir) = loaded("gen_no_handler", LUA);
        let mods = boxed(m);
        // tick: unregistered, nothing runs.
        {
            dispatch_tick(&mut FakeHost::empty(), &mods);
        }
        // server_about_to_start: unregistered, nothing runs.
        {
            dispatch_server_about_to_start(&mut FakeHost::empty(), &mods, true);
        }
        // server_stopping: unregistered, nothing runs.
        {
            dispatch_server_stopping(&mut FakeHost::empty(), &mods, true);
        }
        // level_tick_pre: unregistered, nothing runs.
        {
            dispatch_level_tick_pre(&mut FakeHost::empty(), &mods, "test:dimension");
        }
        // level_tick_post: unregistered, nothing runs.
        {
            dispatch_level_tick_post(&mut FakeHost::empty(), &mods, "test:dimension");
        }
        // block_changed: unregistered, nothing runs.
        {
            dispatch_block_changed(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                BlockStateId(10),
                BlockStateId(11),
            );
        }
        // key: unregistered, nothing runs.
        {
            dispatch_key(&mut FakeHost::empty(), &mods, "test:key", false);
        }
        // wheel: unregistered, nothing runs.
        {
            let mut claimed = false;
            dispatch_wheel(&mut FakeHost::empty(), &mods, 1.5, &mut claimed);
            assert!(!claimed, "claimed changed with no handler");
        }
        // settings: unregistered, nothing runs.
        {
            let mut buttons = vec!["test:seed".to_owned()];
            dispatch_settings(&mut FakeHost::empty(), &mods, "test:mod_id", &mut buttons);
        }
        // settings_click: unregistered, nothing runs.
        {
            dispatch_settings_click(&mut FakeHost::empty(), &mods, "test:mod_id", 8);
        }
        // screen_click: unregistered, nothing runs.
        {
            let mut button: Option<i32> = None;
            dispatch_screen_click(
                &FakeHost::empty(),
                &mods,
                "test:block",
                &[5, 10],
                [3.5, 4.5],
                false,
                &mut button,
            );
        }
        // screen_tooltip: unregistered, nothing runs.
        {
            let mut lines = vec!["test:seed".to_owned()];
            dispatch_screen_tooltip(
                &FakeHost::empty(),
                &mods,
                "test:block",
                &[5, 10],
                [3.5, 4.5],
                &mut lines,
            );
        }
        // menu_clicked: unregistered, nothing runs.
        {
            dispatch_menu_clicked(&mut FakeHost::empty(), &mods, 7, 8, 9, 10, 11);
        }
        // menu_closed: unregistered, nothing runs.
        {
            dispatch_menu_closed(&mut FakeHost::empty(), &mods, 7, 8);
        }
        // block_spread: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_block_spread(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                "test:kind",
                BlockPos::new(3, 4, 5),
                BlockPos::new(4, 5, 6),
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // fill_bucket: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_fill_bucket(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                [2, 3, 4],
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // on_equip: unregistered, nothing runs.
        {
            dispatch_on_equip(
                &mut FakeHost::empty(),
                &mods,
                "test:slot",
                Some(&ItemStack {
                    item: "test:new".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
            );
        }
        // is_signal_source: unregistered, nothing runs.
        {
            let mut source = false;
            dispatch_is_signal_source(&FakeLevel, &mods, BlockStateId(8), &mut source);
        }
        // left_click_block: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_left_click_block(
                &mut FakeHost::empty(),
                &mods,
                [1, 2, 3],
                [2, 3, 4],
                BlockStateId(10),
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // entity_leave_level: unregistered, nothing runs.
        {
            dispatch_entity_leave_level(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                "test:kind",
            );
        }
        // should_apply_effect_tick_this_tick: unregistered, nothing runs.
        {
            let mut apply = false;
            dispatch_should_apply_effect_tick_this_tick(
                &FakeHost::empty(),
                &mods,
                "test:effect",
                8,
                9,
                &mut apply,
            );
        }
        // apply_effect_tick: unregistered, nothing runs.
        {
            let mut keep = true;
            dispatch_apply_effect_tick(
                &FakeHost::empty(),
                &mods,
                "test:effect",
                "test:dimension",
                Some(9),
                10,
                &mut keep,
            );
        }
        // on_effect_added: unregistered, nothing runs.
        {
            dispatch_on_effect_added(&mut FakeHost::empty(), &mods, "test:effect", Some(8), 9);
        }
        // mob_effect_added: unregistered, nothing runs.
        {
            dispatch_mob_effect_added(&mut FakeHost::empty(), &mods, "test:effect", Some(8), 9);
        }
        // chunk_load: unregistered, nothing runs.
        {
            dispatch_chunk_load(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [4, -6],
                true,
            );
        }
        // chunk_unload: unregistered, nothing runs.
        {
            dispatch_chunk_unload(&mut FakeHost::empty(), &mods, "test:dimension", [4, -6]);
        }
        // has_analog_output: unregistered, nothing runs.
        {
            let mut output = false;
            dispatch_has_analog_output(&FakeLevel, &mods, BlockStateId(8), &mut output);
        }
        // component_types: unregistered, nothing runs.
        {
            let mut types = vec!["test:seed".to_owned()];
            dispatch_component_types(&mods, &mut types);
        }
        // tooltip_component: unregistered, nothing runs.
        {
            let mut size: Option<[f32; 2]> = None;
            dispatch_tooltip_component(
                &FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                &mut size,
            );
        }
        // can_survive: unregistered, nothing runs.
        {
            let mut survives: Option<bool> = None;
            dispatch_can_survive(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                BlockStateId(10),
                &mut survives,
            );
        }
        // may_place_on: unregistered, nothing runs.
        {
            let mut may: Option<bool> = None;
            dispatch_may_place_on(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockStateId(9),
                BlockStateId(10),
                BlockPos::new(4, 5, 6),
                &mut may,
            );
        }
        // is_valid_bonemeal_target: unregistered, nothing runs.
        {
            let mut valid: Option<bool> = None;
            dispatch_is_valid_bonemeal_target(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                BlockStateId(10),
                &mut valid,
            );
        }
        // is_bonemeal_success: unregistered, nothing runs.
        {
            let mut success = false;
            dispatch_is_bonemeal_success(
                &mut FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                BlockStateId(10),
                &mut success,
            );
        }
        // client_resources_reloaded: unregistered, nothing runs.
        {
            dispatch_client_resources_reloaded(&mut FakeHost::empty(), &mods);
        }
        // use_duration: unregistered, nothing runs.
        {
            let mut duration = 8;
            dispatch_use_duration(
                &FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                &mut duration,
            );
        }
        // can_eat: unregistered, nothing runs.
        {
            let mut can_eat = false;
            dispatch_can_eat(
                &FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                &mut can_eat,
            );
        }
        // perform_console_command: unregistered, nothing runs.
        {
            let mut ran = false;
            dispatch_perform_console_command(
                &mut FakeHost::empty(),
                &mods,
                "test:command",
                &mut ran,
            );
            assert!(!ran, "ran changed with no handler");
        }
        // server_data_loaded: unregistered, nothing runs.
        {
            dispatch_server_data_loaded(&mut FakeHost::empty(), &mods, true);
        }
        // compute_fov: unregistered, nothing runs.
        {
            let mut degrees = 3.5;
            dispatch_compute_fov(
                &FakeHost::empty(),
                &mods,
                1.5,
                [2.0, 2.0, 3.0],
                &mut degrees,
            );
        }
        // mouse_button_pre: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_mouse_button_pre(&mut FakeHost::empty(), &mods, 7, false, 9, &mut canceled);
            assert!(!canceled, "canceled changed with no handler");
        }
        // mouse_button_post: unregistered, nothing runs.
        {
            dispatch_mouse_button_post(&mut FakeHost::empty(), &mods, 7, false, 9);
        }
        // datapack_sync: unregistered, nothing runs.
        {
            let mut recipe_types = vec!["test:seed".to_owned()];
            dispatch_datapack_sync(&mut FakeHost::empty(), &mods, &mut recipe_types);
        }
        // recipes_received: unregistered, nothing runs.
        {
            dispatch_recipes_received(&mut FakeHost::empty(), &mods, &["test:seed".to_owned()]);
        }
        // mod_container_viewers: unregistered, nothing runs.
        {
            dispatch_mod_container_viewers(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                9,
                10,
            );
        }
        // experience_orb_can_merge: unregistered, nothing runs.
        {
            let mut can_merge = false;
            dispatch_experience_orb_can_merge(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                9,
                10,
                11,
                &mut can_merge,
            );
        }
        // experience_orb_merge: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_experience_orb_merge(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                9,
                10,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // menu_slot_rules: unregistered, nothing runs.
        {
            let mut may_place = false;
            let mut max_stack = 11;
            dispatch_menu_slot_rules(
                &mods,
                "test:menu",
                8,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 3,
                    damage: 2,
                    extra: None,
                },
                &mut may_place,
                &mut max_stack,
            );
        }
        // on_use_tick: unregistered, nothing runs.
        {
            dispatch_on_use_tick(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                Some(8),
                9,
            );
        }
        // release_using: unregistered, nothing runs.
        {
            let mut used = false;
            dispatch_release_using(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                Some(8),
                9,
                &mut used,
            );
        }
        // level_load: unregistered, nothing runs.
        {
            dispatch_level_load(&mut FakeHost::empty(), &mods, "test:dimension");
        }
        // level_unload: unregistered, nothing runs.
        {
            dispatch_level_unload(&mut FakeHost::empty(), &mods, "test:dimension");
        }
        // client_level_load: unregistered, nothing runs.
        {
            dispatch_client_level_load(&mut FakeHost::empty(), &mods, "test:dimension");
        }
        // client_level_unload: unregistered, nothing runs.
        {
            dispatch_client_level_unload(&mut FakeHost::empty(), &mods, "test:dimension");
        }
        // entity_mount: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_entity_mount(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                "test:kind",
                10,
                "test:vehicle_kind",
                false,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // player_start_tracking: unregistered, nothing runs.
        {
            dispatch_player_start_tracking(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                "test:kind",
            );
        }
        // player_stop_tracking: unregistered, nothing runs.
        {
            dispatch_player_stop_tracking(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                "test:kind",
            );
        }
        // living_breathe: unregistered, nothing runs.
        {
            let mut can_breathe = true;
            let mut consume = 10;
            let mut refill = 11;
            dispatch_living_breathe(
                &mods,
                7,
                "test:kind",
                &mut can_breathe,
                &mut consume,
                &mut refill,
            );
        }
        // entity_tick_pre: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_entity_tick_pre(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                "test:kind",
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // item_fluid_tank: unregistered, nothing runs.
        {
            let mut capacity: Option<i32> = None;
            dispatch_item_fluid_tank(&FakeHost::empty(), &mods, "test:item", &mut capacity);
        }
        // player_render_listened: unregistered, nothing runs.
        {
            let mut listened = true;
            dispatch_player_render_listened(&FakeHost::empty(), &mods, &mut listened);
        }
        // sign_changed: unregistered, nothing runs.
        {
            let mut lines = vec!["test:seed".to_owned()];
            let mut cancel = false;
            dispatch_sign_changed(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:dimension",
                [4, 5, 6],
                true,
                &mut lines,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // standalone_models: unregistered, nothing runs.
        {
            let mut models = vec!["test:seed".to_owned()];
            dispatch_standalone_models(&mods, &mut models);
        }
        // player_container_close: unregistered, nothing runs.
        {
            dispatch_player_container_close(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:dimension",
                "test:menu",
                Some([5, 6, 7]),
            );
        }
        // player_toggle_flight: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_toggle_flight(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                true,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // anvil_damaged: unregistered, nothing runs.
        {
            dispatch_anvil_damaged(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                "test:from",
                "test:to",
            );
        }
        // arrow_loose: unregistered, nothing runs.
        {
            let mut charge = 9;
            let mut canceled = false;
            dispatch_arrow_loose(
                &mut FakeHost::empty(),
                &mods,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                },
                false,
                &mut charge,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // beacon_state_change: unregistered, nothing runs.
        {
            dispatch_beacon_state_change(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                true,
            );
        }
        // bell_ring: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_bell_ring(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                Some(8),
                BlockPos::new(3, 4, 5),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_break_progress_update: unregistered, nothing runs.
        {
            dispatch_block_break_progress_update(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                10,
            );
        }
        // block_burn: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_burn(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_cook: unregistered, nothing runs.
        {
            dispatch_block_cook(
                &FakeHost::empty(),
                &mods,
                [1, 2, 3],
                "test:item",
                "test:result",
            );
        }
        // block_damage_abort: unregistered, nothing runs.
        {
            dispatch_block_damage_abort(&mut FakeHost::empty(), &mods, 7, "test:name", [3, 4, 5]);
        }
        // block_dispense_armor: unregistered, nothing runs.
        {
            dispatch_block_dispense_armor(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                "test:item",
                "test:target",
            );
        }
        // block_exp: unregistered, nothing runs.
        {
            let mut experience = 10;
            dispatch_block_exp(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                "test:block",
                &mut experience,
            );
        }
        // block_fade: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_fade(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                BlockStateId(10),
                BlockStateId(11),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_failed_dispense: unregistered, nothing runs.
        {
            dispatch_block_failed_dispense(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                "test:item",
            );
        }
        // block_form: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_form(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                BlockStateId(10),
                BlockStateId(11),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_ignite: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_ignite(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                Some(8),
                BlockPos::new(3, 4, 5),
                "test:cause",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_lock_check: unregistered, nothing runs.
        {
            let mut locked = false;
            dispatch_block_lock_check(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut locked,
            );
            assert!(!locked, "locked changed with no handler");
        }
        // block_receive_game: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_receive_game(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                "test:block",
                "test:event",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_shear_entity: unregistered, nothing runs.
        {
            dispatch_block_shear_entity(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                9,
                "test:kind",
            );
        }
        // brewing_complete: unregistered, nothing runs.
        {
            dispatch_brewing_complete(&FakeHost::empty(), &mods, [1, 2, 3]);
        }
        // brewing_fuel: unregistered, nothing runs.
        {
            dispatch_brewing_fuel(&FakeHost::empty(), &mods, [1, 2, 3], "test:fuel");
        }
        // brewing_start: unregistered, nothing runs.
        {
            dispatch_brewing_start(&FakeHost::empty(), &mods, [1, 2, 3], "test:ingredient");
        }
        // campfire_start: unregistered, nothing runs.
        {
            dispatch_campfire_start(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        }
        // cauldron_level_changed: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_cauldron_level_changed(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                Some(8),
                BlockPos::new(3, 4, 5),
                10,
                11,
                "test:reason",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // chorus_fruit_teleport: unregistered, nothing runs.
        {
            let mut target = [4.0, 2.0, 3.0];
            let mut cancel = false;
            dispatch_chorus_fruit_teleport(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3.0, 2.0, 3.0],
                &mut target,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // compost_item: unregistered, nothing runs.
        {
            dispatch_compost_item(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        }
        // crafter_craft: unregistered, nothing runs.
        {
            dispatch_crafter_craft(&FakeHost::empty(), &mods, [1, 2, 3], "test:recipe");
        }
        // creeper_ignite: unregistered, nothing runs.
        {
            dispatch_creeper_ignite(&mut FakeHost::empty(), &mods, 7, 8, "test:item");
        }
        // creeper_power: unregistered, nothing runs.
        {
            dispatch_creeper_power(&mut FakeHost::empty(), &mods, 7, [2.0, 2.0, 3.0]);
        }
        // critical_hit: unregistered, nothing runs.
        {
            let mut critical = true;
            let mut damage_multiplier = 4.5;
            dispatch_critical_hit(
                &mut FakeHost::empty(),
                &mods,
                "test:target_kind",
                false,
                &mut critical,
                &mut damage_multiplier,
            );
        }
        // difficulty_changed: unregistered, nothing runs.
        {
            dispatch_difficulty_changed(&mut FakeHost::empty(), &mods, "test:from", "test:to");
        }
        // dragon_egg_form: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_dragon_egg_form(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // enchant_item: unregistered, nothing runs.
        {
            dispatch_enchant_item(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &ItemStack {
                    item: "test:item".into(),
                    count: 4,
                    damage: 3,
                    extra: None,
                },
                11,
            );
        }
        // ender_pearl_teleport: unregistered, nothing runs.
        {
            let mut target = [5.0, 2.0, 3.0];
            let mut damage = 6.5;
            let mut cancel = false;
            dispatch_ender_pearl_teleport(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                [4.0, 2.0, 3.0],
                &mut target,
                &mut damage,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // entities_load: unregistered, nothing runs.
        {
            dispatch_entities_load(&mut FakeHost::empty(), &mods, "test:dimension", [4, -6], 9);
        }
        // entities_unload: unregistered, nothing runs.
        {
            dispatch_entities_unload(&mut FakeHost::empty(), &mods, "test:dimension", [4, -6], 9);
        }
        // entity_break_door: unregistered, nothing runs.
        {
            dispatch_entity_break_door(&mut FakeHost::empty(), &mods, 7, "test:kind", [3, 4, 5]);
        }
        // entity_breed: unregistered, nothing runs.
        {
            dispatch_entity_breed(&mut FakeHost::empty(), &mods, 7, 8, Some(9));
        }
        // entity_change_block: unregistered, nothing runs.
        {
            dispatch_entity_change_block(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                [3, 4, 5],
                "test:from",
                "test:to",
            );
        }
        // entity_drop_item: unregistered, nothing runs.
        {
            dispatch_entity_drop_item(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        // entity_enter_block: unregistered, nothing runs.
        {
            dispatch_entity_enter_block(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                [3, 4, 5],
                "test:block",
            );
        }
        // entity_enter_love_mode: unregistered, nothing runs.
        {
            dispatch_entity_enter_love_mode(&mut FakeHost::empty(), &mods, 7, "test:kind", Some(9));
        }
        // entity_portal: unregistered, nothing runs.
        {
            dispatch_entity_portal(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                [3, 4, 5],
            );
        }
        // entity_pose_change: unregistered, nothing runs.
        {
            dispatch_entity_pose_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:from",
                "test:to",
            );
        }
        // entity_shoot_bow: unregistered, nothing runs.
        {
            dispatch_entity_shoot_bow(&mut FakeHost::empty(), &mods, 7, "test:name", 9, 4.5);
        }
        // entity_tame: unregistered, nothing runs.
        {
            dispatch_entity_tame(&mut FakeHost::empty(), &mods, 7, "test:kind", Some(9));
        }
        // entity_teleport: unregistered, nothing runs.
        {
            dispatch_entity_teleport(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                [3.0, 2.0, 3.0],
                [4.0, 2.0, 3.0],
            );
        }
        // entity_toggle_glide: unregistered, nothing runs.
        {
            dispatch_entity_toggle_glide(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        }
        // entity_transform: unregistered, nothing runs.
        {
            dispatch_entity_transform(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                9,
                "test:transformed_kind",
            );
        }
        // entity_unleash: unregistered, nothing runs.
        {
            dispatch_entity_unleash(&mut FakeHost::empty(), &mods, 7, "test:kind");
        }
        // exp_bottle: unregistered, nothing runs.
        {
            dispatch_exp_bottle(&mut FakeHost::empty(), &mods, [1.0, 2.0, 3.0], 8);
        }
        // experience_orb_pickup_listened: unregistered, nothing runs.
        {
            let mut listened = true;
            dispatch_experience_orb_pickup_listened(&FakeHost::empty(), &mods, &mut listened);
        }
        // experience_orb_pickup: unregistered, nothing runs.
        {
            let mut value = 10;
            let mut count = 11;
            let mut canceled = false;
            dispatch_experience_orb_pickup(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:dimension",
                9,
                &mut value,
                &mut count,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // mending_repair: unregistered, nothing runs.
        {
            let mut repair = 10;
            dispatch_mending_repair(
                &mut FakeHost::empty(),
                &mods,
                7,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 2,
                    damage: 1,
                    extra: None,
                },
                9,
                &mut repair,
            );
        }
        // explosion_prime: unregistered, nothing runs.
        {
            dispatch_explosion_prime(&mut FakeHost::empty(), &mods, [1.0, 2.0, 3.0], 2.5);
        }
        // smoke_source: unregistered, nothing runs.
        {
            let mut smoke: Option<bool> = None;
            dispatch_smoke_source(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockStateId(9),
                &mut smoke,
            );
        }
        // firework_explode: unregistered, nothing runs.
        {
            dispatch_firework_explode(&mut FakeHost::empty(), &mods, 7, [2.0, 2.0, 3.0], 9);
        }
        // fluid_level_change: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_fluid_level_change(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                "test:fluid",
                10,
                11,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // framerate_limit: unregistered, nothing runs.
        {
            let mut limit = 10;
            dispatch_framerate_limit(&FakeHost::empty(), &mods, true, false, true, &mut limit);
        }
        // furnace_burn: unregistered, nothing runs.
        {
            dispatch_furnace_burn(&FakeHost::empty(), &mods, [1, 2, 3], "test:fuel");
        }
        // furnace_extract: unregistered, nothing runs.
        {
            dispatch_furnace_extract(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                "test:item",
                11,
            );
        }
        // furnace_smelt: unregistered, nothing runs.
        {
            dispatch_furnace_smelt(
                &FakeHost::empty(),
                &mods,
                [1, 2, 3],
                "test:item",
                "test:result",
            );
        }
        // furnace_start_smelt: unregistered, nothing runs.
        {
            dispatch_furnace_start_smelt(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        }
        // game_event: unregistered, nothing runs.
        {
            dispatch_game_event(&mut FakeHost::empty(), &mods, "test:event", [2.0, 2.0, 3.0]);
        }
        // hopper_inventory_search: unregistered, nothing runs.
        {
            dispatch_hopper_inventory_search(
                &FakeHost::empty(),
                &mods,
                [1, 2, 3],
                Some([2, 3, 4]),
                Some(9),
            );
        }
        // hopper_pickup_item: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_hopper_pickup_item(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                "test:item",
                10,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // inventory_block_start: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_inventory_block_start(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // inventory_move_item: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_inventory_move_item(
                &FakeHost::empty(),
                &mods,
                [1, 2, 3],
                [2, 3, 4],
                "test:item",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // item_despawn: unregistered, nothing runs.
        {
            dispatch_item_despawn(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        // item_expire: unregistered, nothing runs.
        {
            let mut extra_life = 9;
            dispatch_item_expire(
                &mut FakeHost::empty(),
                &mods,
                7,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 2,
                    damage: 1,
                    extra: None,
                },
                &mut extra_life,
            );
        }
        // item_spawn: unregistered, nothing runs.
        {
            dispatch_item_spawn(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:item",
                9,
                [4.0, 2.0, 3.0],
            );
        }
        // leaves_decay: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_leaves_decay(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // lightning_strike: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_lightning_strike(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                [2, 3, 4],
                "test:cause",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // moisture_change: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_moisture_change(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                9,
                10,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // armor_stand_manipulate: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_armor_stand_manipulate(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                "test:slot",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // bucket_empty: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_bucket_empty(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                "test:fluid",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // container_opened: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_container_opened(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:kind",
                Some([4, 5, 6]),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // creative_slot: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_creative_slot(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                "test:item",
                11,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // food_level_changed: unregistered, nothing runs.
        {
            let mut to = 10;
            let mut cancel = false;
            dispatch_food_level_changed(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                &mut to,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // hanging_break: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_hanging_break(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                "test:kind",
                [3, 4, 5],
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // hanging_place: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_hanging_place(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:kind",
                [4, 5, 6],
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // item_damaged: unregistered, nothing runs.
        {
            let mut amount = 10;
            let mut cancel = false;
            dispatch_item_damaged(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:item",
                &mut amount,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_clone: unregistered, nothing runs.
        {
            dispatch_player_clone(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        }
        // player_commands_sent: unregistered, nothing runs.
        {
            let mut names = vec!["test:seed".to_owned()];
            dispatch_player_commands_sent(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &mut names,
            );
        }
        // player_destroy_item: unregistered, nothing runs.
        {
            dispatch_player_destroy_item(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:item",
                "test:slot",
            );
        }
        // player_fish: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_fish(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_flower_pot_manipulate: unregistered, nothing runs.
        {
            dispatch_player_flower_pot_manipulate(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                "test:plant",
            );
        }
        // player_game_mode_changed: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_game_mode_changed(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_insert_lectern_book: unregistered, nothing runs.
        {
            dispatch_player_insert_lectern_book(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                "test:book",
            );
        }
        // player_item_frame_change: unregistered, nothing runs.
        {
            dispatch_player_item_frame_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                "test:action",
                "test:item",
                12,
            );
        }
        // player_item_mend: unregistered, nothing runs.
        {
            dispatch_player_item_mend(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:item",
                10,
                11,
            );
        }
        // player_lectern_page_change: unregistered, nothing runs.
        {
            dispatch_player_lectern_page_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                10,
            );
        }
        // player_sprint: unregistered, nothing runs.
        {
            dispatch_player_sprint(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        }
        // player_swap_hands: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_swap_hands(&mut FakeHost::empty(), &mods, 7, "test:name", &mut cancel);
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_take_lectern_book: unregistered, nothing runs.
        {
            dispatch_player_take_lectern_book(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                "test:book",
            );
        }
        // player_teleported: unregistered, nothing runs.
        {
            dispatch_player_teleported(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3.0, 2.0, 3.0],
                [4.0, 2.0, 3.0],
                "test:dimension",
            );
        }
        // player_totem: unregistered, nothing runs.
        {
            dispatch_player_totem(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // resource_pack_status: unregistered, nothing runs.
        {
            dispatch_resource_pack_status(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        }
        // spawn_change: unregistered, nothing runs.
        {
            dispatch_spawn_change(&mut FakeHost::empty(), &mods, [1, 2, 3], [2.5, 3.5]);
        }
        // tnt_primed: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_tnt_primed(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                [2, 3, 4],
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // vehicle_create: unregistered, nothing runs.
        {
            dispatch_vehicle_create(&mut FakeHost::empty(), &mods, "test:kind", [2.0, 2.0, 3.0]);
        }
        // vehicle_damage: unregistered, nothing runs.
        {
            let mut damage = 2.5;
            let mut cancel = false;
            dispatch_vehicle_damage(
                &mut FakeHost::empty(),
                &mods,
                "test:kind",
                &mut damage,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // vehicle_destroy: unregistered, nothing runs.
        {
            dispatch_vehicle_destroy(&mut FakeHost::empty(), &mods, "test:kind", [2.0, 2.0, 3.0]);
        }
        // vehicle_moved: unregistered, nothing runs.
        {
            dispatch_vehicle_moved(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                "test:kind",
                [3.0, 2.0, 3.0],
                [4.0, 2.0, 3.0],
            );
        }
        // world_border_bounds_changed: unregistered, nothing runs.
        {
            dispatch_world_border_bounds_changed(&mut FakeHost::empty(), &mods);
        }
        // world_border_center_changed: unregistered, nothing runs.
        {
            dispatch_world_border_center_changed(&mut FakeHost::empty(), &mods);
        }
        // world_game_rule_change: unregistered, nothing runs.
        {
            dispatch_world_game_rule_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:rule",
                "test:value",
            );
        }
        // note_play: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_note_play(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                Some(8),
                BlockPos::new(3, 4, 5),
                "test:instrument",
                11,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_advancement_criterion_grant: unregistered, nothing runs.
        {
            dispatch_player_advancement_criterion_grant(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:advancement",
                "test:criterion",
            );
        }
        // player_advancement_done: unregistered, nothing runs.
        {
            dispatch_player_advancement_done(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:advancement",
            );
        }
        // player_advancement_progress: unregistered, nothing runs.
        {
            dispatch_player_advancement_progress(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:advancement",
                "test:criterion",
                true,
            );
        }
        // player_arm_swing: unregistered, nothing runs.
        {
            dispatch_player_arm_swing(&mut FakeHost::empty(), &mods, 7, "test:name", "test:hand");
        }
        // player_attack_cooldown_reset: unregistered, nothing runs.
        {
            dispatch_player_attack_cooldown_reset(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_bed_leave: unregistered, nothing runs.
        {
            dispatch_player_bed_leave(&mut FakeHost::empty(), &mods, 7, "test:name", [3, 4, 5]);
        }
        // player_bucket_entity: unregistered, nothing runs.
        {
            dispatch_player_bucket_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                "test:entity",
            );
        }
        // player_bundle_item_select: unregistered, nothing runs.
        {
            dispatch_player_bundle_item_select(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
            );
        }
        // player_change_beacon_effect: unregistered, nothing runs.
        {
            dispatch_player_change_beacon_effect(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                "test:primary",
                "test:secondary",
            );
        }
        // player_change_main_hand: unregistered, nothing runs.
        {
            dispatch_player_change_main_hand(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        }
        // player_client_loaded_world: unregistered, nothing runs.
        {
            dispatch_player_client_loaded_world(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_deep_sleep: unregistered, nothing runs.
        {
            dispatch_player_deep_sleep(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_edit_book: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_edit_book(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                false,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_egg_throw: unregistered, nothing runs.
        {
            dispatch_player_egg_throw(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_elytra_boost: unregistered, nothing runs.
        {
            dispatch_player_elytra_boost(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_input: unregistered, nothing runs.
        {
            dispatch_player_input(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        }
        // player_item_held: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_item_held(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_jump: unregistered, nothing runs.
        {
            dispatch_player_jump(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_kicked: unregistered, nothing runs.
        {
            dispatch_player_kicked(&mut FakeHost::empty(), &mods, 7, "test:name", "test:reason");
        }
        // player_launch_projectile: unregistered, nothing runs.
        {
            dispatch_player_launch_projectile(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:item",
            );
        }
        // player_level_change: unregistered, nothing runs.
        {
            dispatch_player_level_change(&mut FakeHost::empty(), &mods, 7, "test:name", 9, 10);
        }
        // player_locale_change: unregistered, nothing runs.
        {
            dispatch_player_locale_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:locale",
            );
        }
        // player_loom_pattern_select: unregistered, nothing runs.
        {
            dispatch_player_loom_pattern_select(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        }
        // player_map_filled: unregistered, nothing runs.
        {
            dispatch_player_map_filled(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        }
        // player_name_entity: unregistered, nothing runs.
        {
            dispatch_player_name_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                "test:entity_name",
            );
        }
        // player_open_sign: unregistered, nothing runs.
        {
            dispatch_player_open_sign(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                false,
            );
        }
        // player_pick_item: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_pick_item(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                true,
                [4, 5, 6],
                11,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_pickup_arrow: unregistered, nothing runs.
        {
            dispatch_player_pickup_arrow(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                "test:item",
            );
        }
        // player_portal: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_portal(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:from_dimension",
                "test:to_dimension",
                "test:cause",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_ready_arrow: unregistered, nothing runs.
        {
            dispatch_player_ready_arrow(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:arrow",
            );
        }
        // player_recipe_book_click: unregistered, nothing runs.
        {
            dispatch_player_recipe_book_click(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:recipe",
                false,
            );
        }
        // player_recipe_book_settings_change: unregistered, nothing runs.
        {
            dispatch_player_recipe_book_settings_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                false,
                true,
            );
        }
        // player_recipe_discovered: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_recipe_discovered(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:recipe",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_riptide: unregistered, nothing runs.
        {
            dispatch_player_riptide(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_shear_entity: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_shear_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                false,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_spawn_point_set: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_spawn_point_set(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:dimension",
                [4, 5, 6],
                "test:cause",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_stat_award: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_stat_award(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:stat_type",
                "test:stat",
                11,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_stonecutter_recipe_select: unregistered, nothing runs.
        {
            dispatch_player_stonecutter_recipe_select(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
            );
        }
        // player_stop_spectating_entity: unregistered, nothing runs.
        {
            dispatch_player_stop_spectating_entity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
            );
        }
        // player_stop_using_item: unregistered, nothing runs.
        {
            dispatch_player_stop_using_item(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_swap_with_equipment_slot: unregistered, nothing runs.
        {
            dispatch_player_swap_with_equipment_slot(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:slot",
            );
        }
        // player_teleport_end_gateway: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_teleport_end_gateway(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_toggle_entity_age_lock: unregistered, nothing runs.
        {
            dispatch_player_toggle_entity_age_lock(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                false,
            );
        }
        // player_toggle_sneak: unregistered, nothing runs.
        {
            dispatch_player_toggle_sneak(&mut FakeHost::empty(), &mods, 7, "test:name", true);
        }
        // player_trade: unregistered, nothing runs.
        {
            dispatch_player_trade(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        }
        // player_unleash_entity: unregistered, nothing runs.
        {
            dispatch_player_unleash_entity(&mut FakeHost::empty(), &mods, 7, "test:name", 9);
        }
        // player_velocity: unregistered, nothing runs.
        {
            dispatch_player_velocity(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3.0, 2.0, 3.0],
            );
        }
        // portal_create: unregistered, nothing runs.
        {
            dispatch_portal_create(&mut FakeHost::empty(), &mods, "test:dimension", [2, 3, 4]);
        }
        // potion_splash: unregistered, nothing runs.
        {
            dispatch_potion_splash(&mut FakeHost::empty(), &mods, "test:item", [2.0, 2.0, 3.0]);
        }
        // prepare_anvil: unregistered, nothing runs.
        {
            let mut cost = 10;
            let mut result: Option<ItemStack> = None;
            dispatch_prepare_anvil(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut cost,
                &mut result,
            );
        }
        // prepare_grindstone: unregistered, nothing runs.
        {
            let mut result: Option<ItemStack> = None;
            dispatch_prepare_grindstone(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut result,
            );
        }
        // prepare_hopper_transfer: unregistered, nothing runs.
        {
            dispatch_prepare_hopper_transfer(&FakeHost::empty(), &mods, [1, 2, 3]);
        }
        // prepare_item_craft: unregistered, nothing runs.
        {
            let mut result: Option<ItemStack> = None;
            dispatch_prepare_item_craft(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                Some([3, 4, 5]),
                &mut result,
            );
        }
        // prepare_item_enchant: unregistered, nothing runs.
        {
            let mut costs = [4, 5, 6];
            dispatch_prepare_item_enchant(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut costs,
            );
        }
        // prepare_smithing: unregistered, nothing runs.
        {
            let mut result: Option<ItemStack> = None;
            dispatch_prepare_smithing(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut result,
            );
        }
        // raid_spawn_wave: unregistered, nothing runs.
        {
            dispatch_raid_spawn_wave(&mut FakeHost::empty(), &mods, 7, [2, 3, 4], 9, 10);
        }
        // raid_state_change: unregistered, nothing runs.
        {
            dispatch_raid_state_change(&mut FakeHost::empty(), &mods, 7, [2, 3, 4], "test:state");
        }
        // sculk_bloom: unregistered, nothing runs.
        {
            dispatch_sculk_bloom(&mut FakeHost::empty(), &mods, "test:dimension", [2, 3, 4]);
        }
        // sleep_finished_time: unregistered, nothing runs.
        {
            let mut new_time = 9;
            dispatch_sleep_finished_time(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                &mut new_time,
            );
        }
        // sponge_absorb: unregistered, nothing runs.
        {
            dispatch_sponge_absorb(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
            );
        }
        // target_hit: unregistered, nothing runs.
        {
            dispatch_target_hit(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                9,
            );
        }
        // thunder_change: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_thunder_change(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                false,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // time_skip: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_time_skip(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                9,
                "test:cause",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // trade_select: unregistered, nothing runs.
        {
            dispatch_trade_select(&mut FakeHost::empty(), &mods, 7, "test:name", Some(9), 10);
        }
        // vault_change_state: unregistered, nothing runs.
        {
            dispatch_vault_change_state(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                [2, 3, 4],
                "test:state",
            );
        }
        // vault_loot: unregistered, nothing runs.
        {
            dispatch_vault_loot(&FakeHost::empty(), &mods, [1, 2, 3], "test:item");
        }
        // weather_change: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_weather_change(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                false,
                true,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // whitelist_state_update: unregistered, nothing runs.
        {
            dispatch_whitelist_state_update(&mut FakeHost::empty(), &mods, true);
        }
        // world_border_bounds_change_finish: unregistered, nothing runs.
        {
            dispatch_world_border_bounds_change_finish(&mut FakeHost::empty(), &mods);
        }
        // world_saved: unregistered, nothing runs.
        {
            dispatch_world_saved(&mut FakeHost::empty(), &mods, true);
        }
        // living_drown: unregistered, nothing runs.
        {
            let mut is_drowning = true;
            let mut damage_amount = 4.5;
            let mut cancel = false;
            dispatch_living_drown(
                &mods,
                7,
                "test:kind",
                &mut is_drowning,
                &mut damage_amount,
                11,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // living_jump: unregistered, nothing runs.
        {
            dispatch_living_jump(&mods, 7, "test:kind");
        }
        // baby_entity_spawn: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_baby_entity_spawn(
                &mut FakeHost::empty(),
                &mods,
                7,
                8,
                9,
                "test:kind",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // can_player_sleep: unregistered, nothing runs.
        {
            let mut problem = Some("test:problem".to_owned());
            dispatch_can_player_sleep(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                [3, 4, 5],
                &mut problem,
            );
        }
        // item_container: unregistered, nothing runs.
        {
            let mut slots: Option<u32> = None;
            dispatch_item_container(&FakeHost::empty(), &mods, "test:item", &mut slots);
        }
        // living_conversion_post: unregistered, nothing runs.
        {
            dispatch_living_conversion_post(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                9,
                "test:converted_kind",
            );
        }
        // living_conversion_pre: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_living_conversion_pre(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                "test:outcome",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // living_death: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_living_death(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                "test:damage_type",
                10,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // living_knockback_v2: unregistered, nothing runs.
        {
            let mut strength = 6.5;
            let mut x = 7.5;
            let mut z = 8.5;
            let mut canceled = false;
            dispatch_living_knockback_v2(
                &mods,
                7,
                "test:kind",
                9,
                Some(&ItemStack {
                    item: "test:attacker_main".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                Some(&ItemStack {
                    item: "test:attacker_off".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                &mut strength,
                &mut x,
                &mut z,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // mob_split: unregistered, nothing runs.
        {
            let mut count = 11;
            let mut cancel = false;
            dispatch_mob_split(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                9,
                10,
                &mut count,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // movement_input: unregistered, nothing runs.
        {
            let mut forward = 1.5;
            let mut strafe = 2.5;
            let mut jump = true;
            let mut sneak = false;
            dispatch_movement_input(
                &FakeHost::empty(),
                &mods,
                &mut forward,
                &mut strafe,
                &mut jump,
                &mut sneak,
            );
        }
        // village_siege: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_village_siege(
                &mut FakeHost::empty(),
                &mods,
                "test:dimension",
                8,
                [3.0, 2.0, 3.0],
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // player_xp_change: unregistered, nothing runs.
        {
            let mut amount = 9;
            let mut cancel = false;
            dispatch_player_xp_change(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &mut amount,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // render_item_in_frame: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_render_item_in_frame(
                &FakeHost::empty(),
                &mods,
                7,
                [2, 3, 4],
                &ItemStack {
                    item: "test:stack".into(),
                    count: 3,
                    damage: 2,
                    extra: None,
                },
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // render_name_tag: unregistered, nothing runs.
        {
            let mut visible = false;
            let mut text = Some("test:text".to_owned());
            dispatch_render_name_tag(
                &FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                true,
                &mut visible,
                &mut text,
            );
        }
        // tooltip_color: unregistered, nothing runs.
        {
            let mut background = 8;
            let mut border_top = 9;
            let mut border_bottom = 10;
            dispatch_tooltip_color(
                &FakeHost::empty(),
                &mods,
                Some(&ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                &mut background,
                &mut border_top,
                &mut border_bottom,
            );
        }
        // camera_pose: unregistered, nothing runs.
        {
            let mut position = [2.0, 2.0, 3.0];
            let mut yaw = 3.5;
            let mut pitch = 4.5;
            dispatch_camera_pose(
                &FakeHost::empty(),
                &mods,
                1.5,
                &mut position,
                &mut yaw,
                &mut pitch,
            );
        }
        // client_start: unregistered, nothing runs.
        {
            dispatch_client_start(&mut FakeHost::empty(), &mods);
        }
        // client_disconnecting: unregistered, nothing runs.
        {
            dispatch_client_disconnecting(&FakeHost::empty(), &mods);
        }
        // client_stopping: unregistered, nothing runs.
        {
            dispatch_client_stopping(&FakeHost::empty(), &mods);
        }
        // menu_background: unregistered, nothing runs.
        {
            let mut radius = Some(1.5);
            dispatch_menu_background(&FakeHost::empty(), &mods, &mut radius);
        }
        // block_entity_accepts_fluid: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_entity_accepts_fluid(
                &FakeHost::empty(),
                &mods,
                "test:block",
                8,
                "test:fluid",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_place: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_block_place(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &ItemStack {
                    item: "test:stack".into(),
                    count: 3,
                    damage: 2,
                    extra: None,
                },
                [4, 5, 6],
                BlockStateId(12),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // block_redstone: unregistered, nothing runs.
        {
            let mut new = 10;
            dispatch_block_redstone(
                &FakeLevel,
                &mods,
                LevelDimension::Overworld,
                BlockPos::new(2, 3, 4),
                9,
                &mut new,
            );
        }
        // chat_suggestion_applied: unregistered, nothing runs.
        {
            dispatch_chat_suggestion_applied(
                &FakeHost::empty(),
                &mods,
                "test:original",
                8,
                9,
                "test:suggestion",
                "test:input",
            );
        }
        // client_attack: unregistered, nothing runs.
        {
            dispatch_client_attack(&mut FakeHost::empty(), &mods, 7, false);
        }
        // client_hurt: unregistered, nothing runs.
        {
            dispatch_client_hurt(&mut FakeHost::empty(), &mods, Some(7), false);
        }
        // client_player_network_logging_in: unregistered, nothing runs.
        {
            dispatch_client_player_network_logging_in(&mut FakeHost::empty(), &mods);
        }
        // client_tick_pre: unregistered, nothing runs.
        {
            dispatch_client_tick_pre(&mut FakeHost::empty(), &mods);
        }
        // client_tick_post: unregistered, nothing runs.
        {
            dispatch_client_tick_post(&mut FakeHost::empty(), &mods);
        }
        // creative_slot_v2: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_creative_slot_v2(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                Some(&ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // custom_block_fluids: unregistered, nothing runs.
        {
            let mut custom = false;
            dispatch_custom_block_fluids(&FakeHost::empty(), &mods, "test:block", &mut custom);
        }
        // entity_change_block_v2: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_entity_change_block_v2(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                Some(8),
                "test:kind",
                [4, 5, 6],
                "test:from",
                "test:to",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // entity_interact_specific: unregistered, nothing runs.
        {
            let mut claimed = false;
            dispatch_entity_interact_specific(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                Some(&ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                [4.0, 2.0, 3.0],
                &mut claimed,
            );
            assert!(!claimed, "claimed changed with no handler");
        }
        // entity_outline: unregistered, nothing runs.
        {
            let mut outline = Some(9);
            dispatch_entity_outline(&FakeHost::empty(), &mods, 7, "test:kind", &mut outline);
        }
        // fishing_hook_landed: unregistered, nothing runs.
        {
            dispatch_fishing_hook_landed(
                &mut FakeHost::empty(),
                &mods,
                7,
                8,
                "test:dimension",
                [4, 5, 6],
                [5, 6, 7],
            );
        }
        // inventory_click: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_inventory_click(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                11,
                12,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // inventory_drag: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_inventory_drag(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                9,
                10,
                &[8, 13],
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // item_dropped_v2: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_item_dropped_v2(
                &mut FakeHost::empty(),
                &mods,
                7,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 2,
                    damage: 1,
                    extra: None,
                },
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // item_interact_mob: unregistered, nothing runs.
        {
            let mut swing: Option<bool> = None;
            dispatch_item_interact_mob(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:kind",
                Some(&ItemStack {
                    item: "test:stack".into(),
                    count: 1,
                    damage: 0,
                    extra: None,
                }),
                &mut swing,
            );
        }
        // item_picked_up_v2: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_item_picked_up_v2(
                &mut FakeHost::empty(),
                &mods,
                7,
                &ItemStack {
                    item: "test:stack".into(),
                    count: 2,
                    damage: 1,
                    extra: None,
                },
                Some(9),
                [4.0, 2.0, 3.0],
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // living_change_target: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_living_change_target(
                &mods,
                "test:dimension",
                8,
                "test:kind",
                Some(10),
                Some(11),
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // living_damage_pre: unregistered, nothing runs.
        {
            let mut amount = 6.5;
            dispatch_living_damage_pre(
                &mods,
                7,
                "test:kind",
                "test:damage_type",
                10,
                11,
                &mut amount,
            );
        }
        // living_damage_post: unregistered, nothing runs.
        {
            dispatch_living_damage_post(&mods, 7, "test:kind", "test:damage_type", 10, 11, 6.5);
        }
        // living_incoming_damage_v3: unregistered, nothing runs.
        {
            let mut amount = 8.5;
            let mut cancel = false;
            dispatch_living_incoming_damage_v3(
                &mods,
                7,
                Some(8),
                "test:kind",
                Some(10),
                Some(11),
                Some(12),
                "test:damage_type",
                &mut amount,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // log_line: unregistered, nothing runs.
        {
            dispatch_log_line(
                &mods,
                "test:level",
                "test:thread",
                "test:message",
                "test:line",
            );
        }
        // permission_check: unregistered, nothing runs.
        {
            let mut result: Option<bool> = None;
            dispatch_permission_check(&FakeHost::empty(), &mods, 7, "test:node", 9, &mut result);
        }
        // player_register_channels: unregistered, nothing runs.
        {
            dispatch_player_register_channels(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &["test:seed".to_owned()],
                false,
            );
        }
        // player_command_level: unregistered, nothing runs.
        {
            let mut level = 10;
            dispatch_player_command_level(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                "test:command",
                &mut level,
            );
        }
        // player_commands_sent_v2: unregistered, nothing runs.
        {
            let mut names = vec!["test:seed".to_owned()];
            let mut names_hidden = vec!["test:seed".to_owned()];
            dispatch_player_commands_sent_v2(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &mut names,
                &mut names_hidden,
            );
        }
        // player_hurt_by_player: unregistered, nothing runs.
        {
            let mut amount = 7.5;
            let mut canceled = false;
            dispatch_player_hurt_by_player(
                &mods,
                7,
                8,
                9,
                10,
                "test:source",
                "test:damage_type",
                &mut amount,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // player_launch_projectile_v2: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_player_launch_projectile_v2(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &ItemStack {
                    item: "test:stack".into(),
                    count: 3,
                    damage: 2,
                    extra: None,
                },
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_logging_out: unregistered, nothing runs.
        {
            dispatch_player_logging_out(&mut FakeHost::empty(), &mods, 7, "test:name");
        }
        // player_move: unregistered, nothing runs.
        {
            let mut to = [4.0, 2.0, 3.0];
            let mut cancel = false;
            dispatch_player_move(
                &mods,
                7,
                "test:name",
                [3.0, 2.0, 3.0],
                &mut to,
                5.5,
                6.5,
                true,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_respawn_position: unregistered, nothing runs.
        {
            let mut dimension = "test:dimension".to_owned();
            let mut pos = [4.0, 2.0, 3.0];
            let mut yaw = 5.5;
            let mut pitch = 6.5;
            dispatch_player_respawn_position(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &mut dimension,
                &mut pos,
                &mut yaw,
                &mut pitch,
            );
        }
        // player_spawn_location: unregistered, nothing runs.
        {
            let mut dimension = "test:dimension".to_owned();
            let mut position = [4.0, 2.0, 3.0];
            let mut rotation = [5.5, 6.5];
            dispatch_player_spawn_location(
                &mut FakeHost::empty(),
                &mods,
                7,
                "test:name",
                &mut dimension,
                &mut position,
                &mut rotation,
            );
        }
        // player_teleport: unregistered, nothing runs.
        {
            let mut to = [7.0, 2.0, 3.0];
            let mut relative = 14;
            let mut cancel = false;
            dispatch_player_teleport(
                &mods,
                7,
                "test:name",
                "test:cause",
                "test:from_dimension",
                [5.0, 2.0, 3.0],
                "test:to_dimension",
                &mut to,
                &mut relative,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // player_tick_pre: unregistered, nothing runs.
        {
            dispatch_player_tick_pre(&mut FakeHost::empty(), &mods);
        }
        // projectile_world_border: unregistered, nothing runs.
        {
            let mut pos = [4.0, 2.0, 3.0];
            let mut velocity = [5.0, 2.0, 3.0];
            let mut collide = false;
            dispatch_projectile_world_border(
                &mods,
                "test:dimension",
                8,
                "test:kind",
                &mut pos,
                &mut velocity,
                &mut collide,
            );
        }
        // resource_pack_status_v2: unregistered, nothing runs.
        {
            dispatch_resource_pack_status_v2(&mut FakeHost::empty(), &mods, 7, "test:name", 9, 10);
        }
        // screen_mouse_dragged_pre_v2: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_screen_mouse_dragged_pre_v2(
                &mut FakeHost::empty(),
                &mods,
                "test:screen",
                [2.5, 3.5],
                9,
                [4.5, 5.5],
                11,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // screen_mouse_scrolled_pre_v2: unregistered, nothing runs.
        {
            let mut canceled = false;
            dispatch_screen_mouse_scrolled_pre_v2(
                &mut FakeHost::empty(),
                &mods,
                "test:screen",
                [2.5, 3.5],
                [3.5, 4.5],
                10,
                &mut canceled,
            );
            assert!(!canceled, "canceled changed with no handler");
        }
        // stateful_block_entity: unregistered, nothing runs.
        {
            let mut stateful = false;
            dispatch_stateful_block_entity(&FakeHost::empty(), &mods, "test:block", &mut stateful);
        }
        // toast_add: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_toast_add(
                &mut FakeHost::empty(),
                &mods,
                "test:kind",
                "test:id",
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // trample: unregistered, nothing runs.
        {
            let mut cancel = false;
            dispatch_trample(
                &mods,
                "test:dimension",
                8,
                "test:kind",
                [4, 5, 6],
                5.5,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        // vehicle_moved_v2: unregistered, nothing runs.
        {
            let mut to = [6.0, 2.0, 3.0];
            let mut cancel = false;
            dispatch_vehicle_moved_v2(
                &mut FakeHost::empty(),
                &mods,
                Some(7),
                "test:dimension",
                9,
                "test:kind",
                [5.0, 2.0, 3.0],
                &mut to,
                &mut cancel,
            );
            assert!(!cancel, "cancel changed with no handler");
        }
        no_error(&id);
        drop_dir(dir);
    }
}
