//! Gameplay and startup, above the reusable client library.
// No `#![recursion_limit]` or other inner attribute horizon-mixins doesn't know here: the
// launcher's horizon-mixins then rejects every native mod targeting this crate (#8169).
#![forbid(unsafe_code)]
extern crate self as horizon_client;
pub use horizon_client_lib::*;
pub mod dev_input;
pub mod entry;
pub mod game;
pub mod mod_api;
/// Constructors for the mod API's `#[non_exhaustive]` event and player structs
/// (whos-carmen/horizon#28): the structs say fields may be added, so only their
/// defining crate can build them, and the Lua runtime's tests moved out of it
/// into `event-horizon-lua`. The `test-support` feature (as `horizon-items`'s)
/// turns them on for a dev-dependency; players' builds never see them.
#[cfg(any(test, feature = "test-support"))]
pub mod test_events {
    use crate::mod_api::*;

    /// Builds `AttackEntityEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn attack_entity_event<'a>(
        entity: i32,
        kind: &'a str,
        pos: [f64; 3],
    ) -> AttackEntityEvent<'a> {
        AttackEntityEvent { entity, kind, pos }
    }
    /// Builds `BlockEntityEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn block_entity_event<'a>(block: &'a str) -> BlockEntityEvent<'a> {
        BlockEntityEvent { block }
    }
    /// Builds `BlockEntityTickEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn block_entity_tick_event<'a>(
        pos: [i32; 3],
        entity: &'a mut ModBlockEntity,
    ) -> BlockEntityTickEvent<'a> {
        BlockEntityTickEvent { pos, entity }
    }
    /// Builds `BlockEntityTicksEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn block_entity_ticks_event<'a>(block: &'a str, ticks: bool) -> BlockEntityTicksEvent<'a> {
        BlockEntityTicksEvent { block, ticks }
    }
    /// Builds `DataPackFilesEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn data_pack_files_event<'a>(
        files: &'a mut Vec<(String, Vec<u8>)>,
    ) -> DataPackFilesEvent<'a> {
        DataPackFilesEvent { files }
    }
    /// Builds `DestroyBlockEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn destroy_block_event(pos: [i32; 3], state: BlockStateId) -> DestroyBlockEvent {
        DestroyBlockEvent { pos, state }
    }
    /// Builds `EntityJoinLevelEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn entity_join_level_event<'a>(
        dimension: &'a str,
        entity: u32,
        kind: &'a str,
        pos: [f64; 3],
        loaded_from_disk: bool,
    ) -> EntityJoinLevelEvent<'a> {
        EntityJoinLevelEvent {
            dimension,
            entity,
            kind,
            pos,
            loaded_from_disk,
        }
    }
    /// Builds `EntityTickListenedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn entity_tick_listened_event(listened: bool) -> EntityTickListenedEvent {
        EntityTickListenedEvent { listened }
    }
    /// Builds `EntityTickPostEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn entity_tick_post_event<'a>(
        dimension: &'a str,
        entity: u32,
        kind: &'a str,
    ) -> EntityTickPostEvent<'a> {
        EntityTickPostEvent {
            dimension,
            entity,
            kind,
        }
    }
    /// Builds `ExplosionDetonateEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn explosion_detonate_event<'a>(
        dimension: &'a str,
        center: [f64; 3],
        blocks: &'a mut Vec<[i32; 3]>,
    ) -> ExplosionDetonateEvent<'a> {
        ExplosionDetonateEvent {
            dimension,
            center,
            blocks,
        }
    }
    /// Builds `HearsProjectileImpactsEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn hears_projectile_impacts_event(hears: bool) -> HearsProjectileImpactsEvent {
        HearsProjectileImpactsEvent { hears }
    }
    /// Builds `HurtEnemyEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn hurt_enemy_event<'a>(
        stack: &'a ItemStack,
        entity: u32,
        kind: &'a str,
    ) -> HurtEnemyEvent<'a> {
        HurtEnemyEvent {
            stack,
            entity,
            kind,
        }
    }
    /// Builds `InteractEntityEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn interact_entity_event<'a>(
        entity: i32,
        kind: &'a str,
        stack: Option<&'a ItemStack>,
    ) -> InteractEntityEvent<'a> {
        InteractEntityEvent {
            entity,
            kind,
            stack,
        }
    }
    /// Builds `InventoryTickEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn inventory_tick_event(slot: i32, selected: bool) -> InventoryTickEvent {
        InventoryTickEvent { slot, selected }
    }
    /// Builds `ItemCraftedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn item_crafted_event<'a>(
        player: u128,
        recipe: &'a str,
        stack: &'a ItemStack,
    ) -> ItemCraftedEvent<'a> {
        ItemCraftedEvent {
            player,
            recipe,
            stack,
        }
    }
    /// Builds `ItemDroppedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn item_dropped_event<'a>(player: u128, item: &'a str, count: u32) -> ItemDroppedEvent<'a> {
        ItemDroppedEvent {
            player,
            item,
            count,
        }
    }
    /// Builds `ItemPickedUpEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn item_picked_up_event<'a>(
        player: u128,
        item: &'a str,
        count: u32,
    ) -> ItemPickedUpEvent<'a> {
        ItemPickedUpEvent {
            player,
            item,
            count,
        }
    }
    /// Builds `LivingEntityUseItemFinishEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_entity_use_item_finish_event<'a>(
        stack: &'a ItemStack,
        entity: Option<i32>,
        hand: Hand,
        duration: i32,
        result: &'a mut Option<ItemStack>,
    ) -> LivingEntityUseItemFinishEvent<'a> {
        LivingEntityUseItemFinishEvent {
            stack,
            entity,
            hand,
            duration,
            result,
        }
    }
    /// Builds `LivingEntityUseItemStartEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_entity_use_item_start_event<'a>(
        stack: &'a ItemStack,
        entity: Option<i32>,
        hand: Hand,
        duration: i32,
    ) -> LivingEntityUseItemStartEvent<'a> {
        LivingEntityUseItemStartEvent {
            stack,
            entity,
            hand,
            duration,
        }
    }
    /// Builds `LivingEquipmentChangeEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_equipment_change_event<'a>(
        entity: u32,
        kind: &'a str,
        slot: &'a str,
        from: Option<&'a ItemStack>,
        to: Option<&'a ItemStack>,
    ) -> LivingEquipmentChangeEvent<'a> {
        LivingEquipmentChangeEvent {
            entity,
            kind,
            slot,
            from,
            to,
        }
    }
    /// Builds `LivingEquipmentListenedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_equipment_listened_event(listened: bool) -> LivingEquipmentListenedEvent {
        LivingEquipmentListenedEvent { listened }
    }
    /// Builds `LivingFallEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_fall_event<'a>(
        entity: u32,
        kind: &'a str,
        distance: f64,
        multiplier: f32,
    ) -> LivingFallEvent<'a> {
        LivingFallEvent {
            entity,
            kind,
            distance,
            multiplier,
        }
    }
    /// Builds `LivingHealEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_heal_event(entity: u32, amount: f32) -> LivingHealEvent {
        LivingHealEvent { entity, amount }
    }
    /// Builds `LivingIncomingDamageEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_incoming_damage_event<'a>(
        entity: i32,
        kind: &'a str,
        damage_type: &'a str,
        causing: i32,
        direct: i32,
        amount: f32,
    ) -> LivingIncomingDamageEvent<'a> {
        LivingIncomingDamageEvent {
            entity,
            kind,
            damage_type,
            causing,
            direct,
            amount,
        }
    }
    /// Builds `LivingKnockbackEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn living_knockback_event<'a>(
        entity: i32,
        kind: &'a str,
        damage_type: &'a str,
        causing: i32,
        direct: i32,
        strength: f64,
        x: f64,
        z: f64,
    ) -> LivingKnockbackEvent<'a> {
        LivingKnockbackEvent {
            entity,
            kind,
            damage_type,
            causing,
            direct,
            strength,
            x,
            z,
        }
    }
    /// Builds `LootDropsEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn loot_drops_event<'a>(
        context: &'a NativeLootDrops<'a>,
        drops: &'a mut Vec<ItemStack>,
    ) -> LootDropsEvent<'a> {
        LootDropsEvent { context, drops }
    }
    /// Builds `MobAiRegisterEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn mob_ai_register_event<'a>(mob: &'a mut Mob) -> MobAiRegisterEvent<'a> {
        MobAiRegisterEvent { mob }
    }
    /// Builds `MobDiedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn mob_died_event<'a>(
        dimension: &'a str,
        entity: u32,
        kind: &'a str,
        damage_type: &'a str,
        causing: i32,
        player: Option<u128>,
    ) -> MobDiedEvent<'a> {
        MobDiedEvent {
            dimension,
            entity,
            kind,
            damage_type,
            causing,
            player,
        }
    }
    /// Builds `MobKilledEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn mob_killed_event<'a>(player: u128, entity: u32, kind: &'a str) -> MobKilledEvent<'a> {
        MobKilledEvent {
            player,
            entity,
            kind,
        }
    }
    /// Builds `MobSpawnedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn mob_spawned_event<'a>(
        dimension: &'a str,
        kind: &'a str,
        pos: [f64; 3],
        natural: bool,
    ) -> MobSpawnedEvent<'a> {
        MobSpawnedEvent {
            dimension,
            kind,
            pos,
            natural,
        }
    }
    /// Builds `NativeLifecycleContext` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn native_lifecycle_context(
        authority: NativeAuthority,
        epoch: NativeWorldEpoch,
        tick: NativeTickId,
    ) -> NativeLifecycleContext {
        NativeLifecycleContext {
            authority,
            epoch,
            tick,
        }
    }
    /// Builds `NativeLootDrops` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn native_loot_drops<'a>(
        table: &'a str,
        dimension: &'a str,
        origin: [f64; 3],
        block: Option<([i32; 3], BlockStateId)>,
        entity: Option<(u32, &'a str)>,
        player: Option<u128>,
        tool: Option<&'a ItemStack>,
        explosion: Option<f32>,
        used: bool,
    ) -> NativeLootDrops<'a> {
        NativeLootDrops {
            table,
            dimension,
            origin,
            block,
            entity,
            player,
            tool,
            explosion,
            used,
        }
    }
    /// Builds `NativeServerPlayer` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn native_server_player(
        uuid: u128,
        name: String,
        permission: u8,
        dimension: String,
        position: [f64; 3],
        game_mode: i32,
        health: f32,
        yaw: f32,
        pitch: f32,
        entity_id: i32,
        address: String,
    ) -> NativeServerPlayer {
        NativeServerPlayer {
            uuid,
            name,
            permission,
            dimension,
            position,
            game_mode,
            health,
            yaw,
            pitch,
            entity_id,
            address,
        }
    }
    /// Builds `PerformCommandEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn perform_command_event<'a>(
        player: &'a NativeServerPlayer,
        command: &'a str,
    ) -> PerformCommandEvent<'a> {
        PerformCommandEvent { player, command }
    }
    /// Builds `PlaceBlockEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn place_block_event<'a>(
        stack: &'a ItemStack,
        pos: [i32; 3],
        state: BlockStateId,
    ) -> PlaceBlockEvent<'a> {
        PlaceBlockEvent { stack, pos, state }
    }
    /// Builds `PlayerBedEnterEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_bed_enter_event<'a>(
        player: u128,
        name: &'a str,
        pos: [i32; 3],
    ) -> PlayerBedEnterEvent<'a> {
        PlayerBedEnterEvent { player, name, pos }
    }
    /// Builds `PlayerChangedDimensionEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_changed_dimension_event<'a>(
        player: u128,
        name: &'a str,
        from: &'a str,
        to: &'a str,
    ) -> PlayerChangedDimensionEvent<'a> {
        PlayerChangedDimensionEvent {
            player,
            name,
            from,
            to,
        }
    }
    /// Builds `PlayerChatEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_chat_event<'a>(
        player: u128,
        name: &'a str,
        message: &'a str,
        replies: &'a mut Vec<Text>,
    ) -> PlayerChatEvent<'a> {
        PlayerChatEvent {
            player,
            name,
            message,
            replies,
        }
    }
    /// Builds `PlayerCommandEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_command_event<'a>(
        player: u128,
        name: &'a str,
        command: &'a str,
        replies: &'a mut Vec<Text>,
    ) -> PlayerCommandEvent<'a> {
        PlayerCommandEvent {
            player,
            name,
            command,
            replies,
        }
    }
    /// Builds `PlayerContainerOpenEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_container_open_event<'a>(
        player: u128,
        name: &'a str,
        dimension: &'a str,
        menu: &'a str,
        pos: Option<[i32; 3]>,
    ) -> PlayerContainerOpenEvent<'a> {
        PlayerContainerOpenEvent {
            player,
            name,
            dimension,
            menu,
            pos,
        }
    }
    /// Builds `PlayerDiedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_died_event<'a>(player: u128, cause: Option<&'a str>) -> PlayerDiedEvent<'a> {
        PlayerDiedEvent { player, cause }
    }
    /// Builds `PlayerExpChangeEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_exp_change_event<'a>(
        player: u128,
        name: &'a str,
        amount: i32,
    ) -> PlayerExpChangeEvent<'a> {
        PlayerExpChangeEvent {
            player,
            name,
            amount,
        }
    }
    /// Builds `PlayerLoggedInEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_logged_in_event<'a>(player: u128, name: &'a str) -> PlayerLoggedInEvent<'a> {
        PlayerLoggedInEvent { player, name }
    }
    /// Builds `PlayerLoggedOutEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_logged_out_event<'a>(player: u128, name: &'a str) -> PlayerLoggedOutEvent<'a> {
        PlayerLoggedOutEvent { player, name }
    }
    /// Builds `PlayerRespawnedEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_respawned_event<'a>(player: u128, name: &'a str) -> PlayerRespawnedEvent<'a> {
        PlayerRespawnedEvent { player, name }
    }
    /// Builds `PlayerTickEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn player_tick_event() -> PlayerTickEvent {
        PlayerTickEvent {}
    }
    /// Builds `ProjectileImpactEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn projectile_impact_event<'a>(
        dimension: &'a str,
        projectile: u32,
        kind: &'a str,
        owner: u32,
        entity: Option<u32>,
        entity_kind: &'a str,
        block: [i32; 3],
        face: [i32; 3],
        at: [f64; 3],
    ) -> ProjectileImpactEvent<'a> {
        ProjectileImpactEvent {
            dimension,
            projectile,
            kind,
            owner,
            entity,
            entity_kind,
            block,
            face,
            at,
        }
    }
    /// Builds `ProjectileLaunchEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn projectile_launch_event<'a>(
        player: Option<u128>,
        name: &'a str,
        entity: u32,
        kind: &'a str,
    ) -> ProjectileLaunchEvent<'a> {
        ProjectileLaunchEvent {
            player,
            name,
            entity,
            kind,
        }
    }
    /// Builds `RandomTickEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn random_tick_event(
        dimension: LevelDimension,
        pos: BlockPos,
        state: BlockStateId,
    ) -> RandomTickEvent {
        RandomTickEvent {
            dimension,
            pos,
            state,
        }
    }
    /// Builds `RegisterCommandsEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn register_commands_event<'a>(
        commands: &'a mut Vec<NativeCommand>,
    ) -> RegisterCommandsEvent<'a> {
        RegisterCommandsEvent { commands }
    }
    /// Builds `RightClickBlockEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn right_click_block_event(pos: [i32; 3], face: [i32; 3]) -> RightClickBlockEvent {
        RightClickBlockEvent { pos, face }
    }
    /// Builds `ServerTickEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn server_tick_event<'a>(
        context: &'a NativeLifecycleContext,
        phase: ServerTickPhase,
    ) -> ServerTickEvent<'a> {
        ServerTickEvent { context, phase }
    }
    /// Builds `TickBlockEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn tick_block_event(
        dimension: LevelDimension,
        pos: BlockPos,
        state: BlockStateId,
    ) -> TickBlockEvent {
        TickBlockEvent {
            dimension,
            pos,
            state,
        }
    }
    /// Builds `UseItemEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn use_item_event<'a>(stack: &'a ItemStack) -> UseItemEvent<'a> {
        UseItemEvent { stack }
    }
    /// Builds `UseItemOnEvent` for the workspace's own tests: the struct is
    /// `#[non_exhaustive]`, so only this crate can construct one.
    pub fn use_item_on_event<'a>(
        stack: &'a ItemStack,
        pos: [i32; 3],
        face: [i32; 3],
    ) -> UseItemOnEvent<'a> {
        UseItemOnEvent { stack, pos, face }
    }
}
