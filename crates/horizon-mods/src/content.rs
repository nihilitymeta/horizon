//! A content mod's `assets/` and `data/` (API 0.2, docs/modpacks.md): the
//! files it adds to the game's resource tree, and the blocks, items, mob
//! effects, entity types and menu types it declares in
//! `data/<id>/horizon/{block,item,mob_effect,entity_type,menu}/`.
//!
//! Content is data, not code: nothing here runs in the sandbox (a scripted
//! mod's `scripts/registry.lua`, feature `lua`, runs in a state of its own
//! with no reach beyond the files it registers: [`crate::registry`]; the
//! game and the launcher, built with the feature, run it when they read
//! the content, the registry server and its CI never do). What a mod may
//! add is fenced instead. Its own namespaces (its id and the ones its
//! manifest lists), plus additions to vanilla tags and to the convention
//! tags (`c`, which NeoForge and Fabric mods share). Paths are plain, files
//! and totals are capped, vanilla ids cannot be redefined, and no two mods
//! share a namespace ([`clash`]).
use crate::manifest::{Capability, MANIFEST, Manifest};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
};

/// Content files per mod, bytes per file and bytes per mod.
pub const MAX_FILES: usize = 65_536;
pub const MAX_FILE: u64 = 16 << 20;
pub const MAX_TOTAL: u64 = 512 << 20;

/// The script a scripted mod starts from (capability `scripts`).
pub const SCRIPT_ENTRY: &str = "scripts/init.lua";
/// The script whose declarations join the content files (feature `lua`).
pub const REGISTRY_SCRIPT: &str = "scripts/registry.lua";

/// Checks a content path: `assets/` or `data/`, then `/`-separated parts of
/// `[a-z0-9_.-]`, none of them `.` or `..` (vanilla's resource-location
/// characters, so the path means the same on every OS); or `scripts/`, then
/// such parts ending in a `.lua` file.
pub fn check_path(name: &str) -> Result<(), String> {
    let bad = || format!("{name:?} is not a plain assets/, data/ or scripts/ path");
    let mut parts = name.split('/');
    let top = parts.next();
    if !matches!(top, Some("assets" | "data" | "scripts")) {
        return Err(bad());
    }
    let script = top == Some("scripts");
    if script && !name.ends_with(".lua") {
        return Err(bad());
    }
    let mut count = 0;
    for part in parts {
        count += 1;
        let chars_ok = part
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b));
        if part.is_empty() || part == "." || part == ".." || !chars_ok {
            return Err(bad());
        }
    }
    // assets/<namespace>/<file> at least; scripts/<file>.
    if count < if script { 1 } else { 2 } {
        return Err(bad());
    }
    Ok(())
}

/// A vanilla identifier, `minecraft:path`.
/// `Identifier.isValidNamespace` and `isValidPath` on an id with both parts
/// given: `[a-z0-9_.-]`, and `/` in the path too.
fn namespaced_id(id: &str) -> bool {
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b);
    id.split_once(':').is_some_and(|(namespace, path)| {
        !namespace.is_empty()
            && !path.is_empty()
            && namespace.bytes().all(ok)
            && path.bytes().all(|b| ok(b) || b == b'/')
    })
}

fn vanilla_id(id: &str) -> bool {
    id.strip_prefix("minecraft:").is_some_and(|path| {
        !path.is_empty()
            && path
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b))
    })
}

/// What a declared block changes from its template; unset keeps the
/// template's value.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockProperties {
    /// Destroy time in seconds of hand mining (-1: unbreakable).
    pub hardness: Option<f32>,
    pub explosion_resistance: Option<f32>,
    /// Light emitted, 0 to 15.
    pub light: Option<u8>,
    pub requires_tool: Option<bool>,
    pub friction: Option<f32>,
    /// A `MapColor` id, 0 to 61.
    pub map_color: Option<u8>,
    /// A vanilla block whose sounds this block uses.
    pub sounds: Option<String>,
    /// Which states tick randomly, whatever the template does (`isRandomlyTicking`).
    pub random_ticks: Option<RandomTicks>,
    /// How the block is drawn, whatever the template does (`BlockBehaviour.getRenderShape`).
    pub render_shape: Option<RenderShape>,
    /// `false`: the block occludes nothing, so its neighbours' faces are drawn and light passes
    /// it (`BlockBehaviour.Properties.noOcclusion`); unset or `true`: as its template.
    pub occludes: Option<bool>,
}

/// Vanilla's `RenderShape`, by its constant's name: `MODEL` puts the block model in the chunk
/// mesh, `ENTITYBLOCK_ANIMATED` leaves it out for a block whose parts are drawn every frame
/// (its block entity's drawing), and `INVISIBLE` draws nothing.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RenderShape {
    Model,
    EntityblockAnimated,
    Invisible,
}

impl RenderShape {
    /// The constant's name, as the block tables write it.
    pub fn name(self) -> &'static str {
        match self {
            RenderShape::Model => "MODEL",
            RenderShape::EntityblockAnimated => "ENTITYBLOCK_ANIMATED",
            RenderShape::Invisible => "INVISIBLE",
        }
    }
}

/// Which of a block's states get random ticks (`BlockBehaviour.isRandomlyTicking(state)`), so a
/// crop, sapling or soil on a template that doesn't tick still grows. The sections' random tick
/// counts follow it, and `native_random_tick` fires for the states it picks.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum RandomTicks {
    /// Every state (`true`), or none (`false`, for a template that ticks).
    All(bool),
    /// The states whose own state property `property` is one of `values`.
    States {
        property: String,
        values: Vec<String>,
    },
}

impl RandomTicks {
    /// Whether a state ticks, `combo` being its values of the block's own state properties.
    pub fn applies(&self, combo: &[(String, String)]) -> bool {
        match self {
            RandomTicks::All(all) => *all,
            RandomTicks::States { property, values } => combo
                .iter()
                .any(|(name, value)| name == property && values.contains(value)),
        }
    }
}

/// Boxes selected by one property (legacy) or an ordered tuple of properties.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BlockShapes {
    #[serde(default)]
    pub property: String,
    #[serde(default)]
    pub properties: Vec<String>,
    #[serde(default)]
    pub values: BTreeMap<String, Vec<[f64; 6]>>,
    pub default: Option<Vec<[f64; 6]>>,
}

impl BlockShapes {
    pub fn property_names(&self) -> Vec<&str> {
        if self.property.is_empty() {
            self.properties.iter().map(String::as_str).collect()
        } else {
            vec![self.property.as_str()]
        }
    }

    fn check(&self) -> Result<(), String> {
        let names = self.property_names();
        if (!self.property.is_empty() && !self.properties.is_empty())
            || names.iter().any(|name| !state_name(name))
            || names.iter().collect::<BTreeSet<_>>().len() != names.len()
            || (names.is_empty() && !self.values.is_empty())
            || (self.values.is_empty() && self.default.is_none())
        {
            return Err("shapes needs distinct properties and values, or a default".into());
        }
        for key in self.values.keys() {
            let values: Vec<_> = key.split(',').collect();
            if values.len() != names.len() || values.iter().any(|value| !state_name(value)) {
                return Err(format!("invalid shape key {key:?}: one value per property"));
            }
        }
        for boxes in self.values.values().chain(self.default.iter()) {
            if boxes.len() > 16
                || boxes.iter().any(|bounds| {
                    bounds.iter().any(|value| !value.is_finite())
                        || (0..3).any(|axis| bounds[axis] > bounds[axis + 3])
                })
            {
                return Err("shape needs at most 16 finite boxes with ordered bounds".into());
            }
        }
        Ok(())
    }
}

/// A state property a block declares on its own, beside its template's.
/// A block's states become the template's states times these values, so a
/// pipe can have a connection per side without a template that has one.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StateProperty {
    /// `false` and `true`, in vanilla's spelling.
    Boolean,
    /// A list of values, in the order the state space is built in.
    Enum { values: Vec<String> },
}

impl StateProperty {
    /// Its values, in the order the state space is built in; a boolean's two
    /// come `false` first, as vanilla orders them.
    pub fn values(&self) -> Vec<&str> {
        match self {
            StateProperty::Boolean => vec!["false", "true"],
            StateProperty::Enum { values } => values.iter().map(String::as_str).collect(),
        }
    }
}

/// A vanilla state property's name, and a value's: lower case, digits and
/// underscores (`minecraft:oak_log` is `axis`, a repeater is `delay`).
fn state_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockFile {
    template: String,
    #[serde(default)]
    properties: BlockProperties,
    shapes: Option<BlockShapes>,
    collision_shapes: Option<BlockShapes>,
    #[serde(default)]
    state_properties: BTreeMap<String, StateProperty>,
    #[serde(default)]
    default_state: BTreeMap<String, String>,
    #[serde(default)]
    connects_to: Option<String>,
    #[serde(default)]
    thickness: Option<f32>,
    #[serde(default)]
    wood_type: Option<String>,
    #[serde(default)]
    wall_block: Option<String>,
    #[serde(default = "yes")]
    item: bool,
    #[serde(default)]
    components: BTreeMap<String, serde_json::Value>,
}
fn yes() -> bool {
    true
}

/// A block the mod adds: a vanilla template's behaviour, states and shapes
/// under a new id, with `properties` applied. A native mod may build one in
/// code (`native_blocks`): `..Default::default()` leaves the rest as a file
/// leaves it out, except `item`, which a file sets unless told not to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// `<mod id>:<path>`.
    pub id: String,
    /// The vanilla block it copies, `minecraft:<path>`.
    pub template: String,
    pub properties: BlockProperties,
    pub shapes: Option<BlockShapes>,
    pub collision_shapes: Option<BlockShapes>,
    /// State properties of its own, which its template does not have.
    pub state_properties: BTreeMap<String, StateProperty>,
    /// State properties overlaid on the template default.
    pub default_state: BTreeMap<String, String>,
    /// Also add a block item with the same id.
    pub item: bool,
    /// Its item's component patch, as an [`Item`]'s.
    pub components: BTreeMap<String, serde_json::Value>,
    /// A connecting block (template `minecraft:chorus_plant`, a `PipeBlock`
    /// without the chorus plant's rules): the block tag, without `#`, whose
    /// blocks it joins besides itself (a cable's machines).
    pub connects_to: Option<String>,
    /// A connecting block's width in pixels (2 to 14): its shapes, the
    /// chorus plant's (10) when unset.
    pub thickness: Option<f32>,
    /// A sign's `WoodType` (template a vanilla sign), which names its editor's
    /// background: `<namespace>:<path>` a mod's texture, a bare name vanilla's.
    /// The template's when unset.
    pub wood_type: Option<String>,
    /// The block a standing or ceiling sign's item places on a wall
    /// (`StandingAndWallBlockItem.wallBlock`): one of this mod's blocks, whose
    /// template is the matching wall sign and which has no item. The
    /// template's when unset.
    pub wall_block: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreativeTabFile {
    title: String,
    icon: String,
    items: Vec<String>,
}

/// A creative inventory tab the mod adds (`data/<id>/horizon/creative_tab/`):
/// its title, the item on its button, and its items in order. Items no mod
/// or vanilla registers are left out when the game loads.
#[derive(Clone, Debug, PartialEq)]
pub struct CreativeTab {
    /// `<mod id>:<path>`.
    pub id: String,
    pub title: String,
    pub icon: String,
    pub items: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemFile {
    template: String,
    max_stack_size: Option<u8>,
    rarity: Option<Rarity>,
    #[serde(default)]
    components: BTreeMap<String, serde_json::Value>,
    crafting_remainder: Option<String>,
}

/// An item the mod adds: a vanilla template item's components under a new
/// id, with these changed. A native mod may build one in code
/// (`native_items`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: String,
    /// The vanilla item it copies, `minecraft:<path>`.
    pub template: String,
    pub max_stack_size: Option<u8>,
    pub rarity: Option<Rarity>,
    /// A 26.3 data component patch over the template's: `!minecraft:<type>` removes one.
    pub components: BTreeMap<String, serde_json::Value>,
    /// What it leaves in the crafting grid (`Item.Properties.craftRemainder`):
    /// a vanilla item or one of the mod's own.
    pub crafting_remainder: Option<String>,
}

/// `MobEffectCategory`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EffectCategory {
    Beneficial,
    Harmful,
    Neutral,
}

/// A mob effect the mod adds: what `new MobEffect(category, color)` takes.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MobEffect {
    /// `<mod id>:<path>`.
    #[serde(skip)]
    pub id: String,
    pub category: EffectCategory,
    /// RGB, `0x000000` to `0xFFFFFF`.
    pub color: u32,
}

/// the vanilla entity types a mod's type may copy: thrown item
/// projectiles (`ThrowableItemProjectile`).
pub const ENTITY_TEMPLATES: [&str; 1] = ["minecraft:snowball"];

/// the vanilla mobs a mod entity type may copy (`template`): the game's
/// species, each with its model, sounds, movement and goals.
pub const MOB_TEMPLATES: &[&str] = &[
    "minecraft:pig",
    "minecraft:cow",
    "minecraft:mooshroom",
    "minecraft:sheep",
    "minecraft:chicken",
    "minecraft:rabbit",
    "minecraft:zombie_horse",
    "minecraft:skeleton_horse",
    "minecraft:giant",
    "minecraft:horse",
    "minecraft:donkey",
    "minecraft:mule",
    "minecraft:llama",
    "minecraft:trader_llama",
    "minecraft:goat",
    "minecraft:panda",
    "minecraft:polar_bear",
    "minecraft:fox",
    "minecraft:wolf",
    "minecraft:cat",
    "minecraft:ocelot",
    "minecraft:parrot",
    "minecraft:bee",
    "minecraft:turtle",
    "minecraft:armadillo",
    "minecraft:frog",
    "minecraft:axolotl",
    "minecraft:squid",
    "minecraft:glow_squid",
    "minecraft:tadpole",
    "minecraft:cod",
    "minecraft:salmon",
    "minecraft:tropical_fish",
    "minecraft:pufferfish",
    "minecraft:dolphin",
    "minecraft:bat",
    "minecraft:villager",
    "minecraft:wandering_trader",
    "minecraft:iron_golem",
    "minecraft:snow_golem",
    "minecraft:copper_golem",
    "minecraft:zombie",
    "minecraft:husk",
    "minecraft:drowned",
    "minecraft:zombie_villager",
    "minecraft:skeleton",
    "minecraft:stray",
    "minecraft:parched",
    "minecraft:bogged",
    "minecraft:wither_skeleton",
    "minecraft:creeper",
    "minecraft:spider",
    "minecraft:cave_spider",
    "minecraft:enderman",
    "minecraft:endermite",
    "minecraft:silverfish",
    "minecraft:slime",
    "minecraft:magma_cube",
    "minecraft:witch",
    "minecraft:phantom",
    "minecraft:evoker",
    "minecraft:illusioner",
    "minecraft:allay",
    "minecraft:creaking",
    "minecraft:camel",
    "minecraft:happy_ghast",
    "minecraft:sniffer",
    "minecraft:breeze",
    "minecraft:wither",
    "minecraft:ravager",
    "minecraft:vex",
    "minecraft:shulker",
    "minecraft:guardian",
    "minecraft:elder_guardian",
    "minecraft:pillager",
    "minecraft:vindicator",
    "minecraft:piglin",
    "minecraft:piglin_brute",
    "minecraft:zombified_piglin",
    "minecraft:hoglin",
    "minecraft:zoglin",
    "minecraft:ghast",
    "minecraft:blaze",
    "minecraft:strider",
    "minecraft:warden",
    "minecraft:camel_husk",
    "minecraft:nautilus",
    "minecraft:zombie_nautilus",
    "minecraft:sulfur_cube",
    "minecraft:mannequin",
];

/// an entity type the mod adds: a copy of a vanilla template's. A thrown
/// one ([`ENTITY_TEMPLATES`]) is thrown from `item` and drawn as it. A mob
/// one ([`MOB_TEMPLATES`]) moves, fights, sounds and is drawn as its
/// template, with its own `attributes`, `size` and `texture`, and either
/// the template's goals or none (for a mod's scripts to fill).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EntityType {
    /// `<mod id>:<path>`.
    #[serde(skip)]
    pub id: String,
    /// one of [`ENTITY_TEMPLATES`] or [`MOB_TEMPLATES`].
    pub template: String,
    /// thrown: one of the mod's own items, which throws it.
    #[serde(default)]
    pub item: Option<String>,
    /// thrown: what a hit deals to an entity, as `thrown` damage.
    #[serde(default)]
    pub damage: f32,
    /// mob: attribute values that differ from the template's, by their
    /// name without `minecraft:` (`max_health`, `movement_speed`,
    /// `attack_damage`, `follow_range`, `armor`, `knockback_resistance`...).
    #[serde(default)]
    pub attributes: BTreeMap<String, f64>,
    /// mob: its hitbox, when it differs from the template's.
    #[serde(default)]
    pub size: Option<Size>,
    /// mob: `<namespace>:entity/<name>`, the mod's
    /// `assets/<namespace>/textures/entity/<name>.png`, laid out as the
    /// template's texture; the template's own without one.
    #[serde(default)]
    pub texture: Option<String>,
    /// mob: whether it starts with its template's goals (the default) or
    /// with empty goal selectors for scripts to fill (`"none"`).
    #[serde(default)]
    pub goals: Goals,
    /// mob: nature flags that differ from the template's, by name
    /// ([`MOB_FLAGS`]): `burns_in_day`, `fire_immune`, `fall_immune`,
    /// `water_sensitive`.
    #[serde(default)]
    pub flags: BTreeMap<String, bool>,
    /// mob: `<namespace>:entity/<name>`, the mod's
    /// `assets/<namespace>/models/entity/<name>.json`, a model of its own
    /// (parts holding cubes, named as the animation set moves them);
    /// the template's model without one.
    #[serde(default)]
    pub model: Option<String>,
    /// mob: the animation set that moves the model's parts (`zombie`,
    /// `quadruped`, `iron_golem`...); the template's without one.
    #[serde(default)]
    pub animation: Option<String>,
    /// mob: sound events of its own, by `ambient`, `hurt`, `death` and
    /// `step`; the template's for the ones left out.
    #[serde(default)]
    pub sounds: BTreeMap<String, String>,
}

/// the nature flags a mob entity type may set (`flags`).
pub const MOB_FLAGS: [&str; 4] = [
    "burns_in_day",
    "fire_immune",
    "fall_immune",
    "water_sensitive",
];

/// The animation sets a mob entity type may name (`animation`): the game's
/// own, by name (`mobs::content_model::ANIMS` lists the same names, and a
/// test keeps the two together).
pub const MOB_ANIMATIONS: &[&str] = &[
    "nautilus",
    "happy_ghast",
    "sniffer",
    "breeze",
    "warden",
    "allay",
    "creaking",
    "camel",
    "wither",
    "vex",
    "ravager",
    "guardian",
    "shulker",
    "quadruped",
    "sheep",
    "chicken",
    "zombie",
    "drowned",
    "zombie_villager",
    "skeleton",
    "creeper",
    "spider",
    "enderman",
    "silverfish",
    "slime",
    "magma_cube",
    "witch",
    "villager",
    "iron_golem",
    "snow_golem",
    "copper_golem",
    "illager",
    "piglin",
    "zombified_piglin",
    "hoglin",
    "wolf",
    "feline",
    "fox",
    "rabbit",
    "horse",
    "llama",
    "goat",
    "panda",
    "polar_bear",
    "parrot",
    "bee",
    "turtle",
    "armadillo",
    "frog",
    "axolotl",
    "squid",
    "fish",
    "pufferfish",
    "dolphin",
    "bat",
    "phantom",
    "ghast",
    "blaze",
    "strider",
];

/// The sound events a mob entity type may give (`sounds`), by key.
pub const MOB_SOUNDS: [&str; 4] = ["ambient", "hurt", "death", "step"];

/// The file a mob entity type's `model` names: `<namespace>:entity/<name>`
/// is `assets/<namespace>/models/entity/<name>.json`, a plain path
/// ([`check_path`]). The namespace comes back too, for the owner check.
/// The game's loader reads a model through this as well, so a mob never
/// opens a file outside that folder.
pub fn mob_model_file(model: &str) -> Result<(&str, String), String> {
    let shape = || format!("model {model:?} is <namespace>:entity/<name>, a file of this mod");
    let (ns, path) = model.split_once(':').ok_or_else(shape)?;
    let name = path.strip_prefix("entity/").ok_or_else(shape)?;
    if ns.is_empty() || name.is_empty() || name.contains('/') {
        return Err(shape());
    }
    // The name on its own passes the path check (so `..` does not).
    check_path(&format!("assets/{ns}/models/entity/{name}")).map_err(|_| shape())?;
    Ok((ns, format!("assets/{ns}/models/entity/{name}.json")))
}

impl EntityType {
    /// Whether the type copies a mob rather than a thrown item.
    pub fn is_mob(&self) -> bool {
        !ENTITY_TEMPLATES.contains(&self.template.as_str())
    }
}

/// a mob entity type's hitbox.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub width: f64,
    pub height: f64,
    /// where it looks from; the template's, scaled to the height, without one.
    #[serde(default)]
    pub eye_height: Option<f64>,
}

/// what goals a mob entity type starts with.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Goals {
    /// the template's, as vanilla registers them for the species.
    #[default]
    Template,
    /// none: the selectors are empty until a script fills them.
    None,
}

/// a menu type the mod adds (vanilla's `MenuType`, NeoForge's
/// `IMenuTypeExtension.create`): its container's slot count, the
/// `ContainerData` count its menu syncs, and its take-only slots
/// (`FurnaceResultSlot.mayPlace`). The slots come first in the menu, then the
/// player's 36, and a mod screen (`native_screen`) places them.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MenuType {
    /// `<mod id>:<path>`.
    #[serde(skip)]
    pub id: String,
    /// 1 to [`MAX_MENU_SLOTS`].
    pub slots: u16,
    /// 0 to [`MAX_MENU_SLOTS`].
    #[serde(default)]
    pub data_slots: u16,
    /// slots a player takes from but never puts into, each below `slots`.
    #[serde(default)]
    pub results: Vec<u16>,
}

/// the most container or data slots a mod menu type has, as many as a mod
/// container (`native_block_entity_container`) may.
pub const MAX_MENU_SLOTS: u16 = 256;

#[derive(Debug, Clone)]
enum Source {
    Dir(PathBuf),
    Zip(PathBuf),
    /// No files: blocks and items a native mod declares in code.
    Code,
}

/// A mod's content: its files and declarations.
#[derive(Debug)]
pub struct Content {
    source: Source,
    /// Every content file (`assets/...`, `data/...`), sorted.
    pub files: Vec<String>,
    /// Every script (`scripts/...`), sorted: [`SCRIPT_ENTRY`] among them when
    /// the mod has capability `scripts`.
    pub scripts: Vec<String>,
    pub blocks: Vec<Block>,
    pub items: Vec<Item>,
    pub effects: Vec<MobEffect>,
    /// Its creative inventory tabs, by id.
    pub creative_tabs: Vec<CreativeTab>,
    pub entity_types: Vec<EntityType>,
    pub menu_types: Vec<MenuType>,
    /// The namespaces it owns: the mod id, then the manifest's `namespaces`.
    pub namespaces: Vec<String>,
    /// The files `scripts/registry.lua` registered, by path: in `files` too,
    /// read from here.
    registered: BTreeMap<String, Vec<u8>>,
}

/// Reads a mod's content files ([`Content::reader`]).
impl Content {
    /// Whether `name` is a file `scripts/registry.lua` registered rather than one of the
    /// mod's own: the game makes it again when it reads the mod, so a package carries only
    /// the mod's own files.
    pub fn is_registered(&self, name: &str) -> bool {
        self.registered.contains_key(name)
    }
}

/// A content mod to put in the root: its id and content.
pub struct ContentMod {
    pub id: String,
    pub content: Content,
}

pub struct FileReader<'a> {
    from: ReadFrom<'a>,
    registered: &'a BTreeMap<String, Vec<u8>>,
}
enum ReadFrom<'a> {
    Dir(&'a Path),
    Zip(Box<zip::ZipArchive<std::fs::File>>),
}
impl FileReader<'_> {
    /// One content file's bytes.
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, String> {
        check_path(name)?;
        if let Some(bytes) = self.registered.get(name) {
            return Ok(bytes.clone());
        }
        let reader: Box<dyn Read + '_> = match &mut self.from {
            ReadFrom::Dir(root) => {
                Box::new(std::fs::File::open(root.join(name)).map_err(|e| format!("{name}: {e}"))?)
            }
            ReadFrom::Zip(zip) => Box::new(zip.by_name(name).map_err(|e| format!("{name}: {e}"))?),
        };
        let mut out = Vec::new();
        reader
            .take(MAX_FILE + 1)
            .read_to_end(&mut out)
            .map_err(|e| format!("{name}: {e}"))?;
        if out.len() as u64 > MAX_FILE {
            return Err(format!("{name} is over {} MiB", MAX_FILE >> 20));
        }
        Ok(out)
    }
}

/// Why two content mods cannot be on together: a namespace both own (one's
/// id or listed namespace is the other's), which would mix their files.
/// `a` and `b` are (mod id, owned namespaces).
pub fn clash(a: (&str, &[String]), b: (&str, &[String])) -> Option<String> {
    let shared = a.1.iter().find(|ns| b.1.contains(ns))?;
    Some(format!(
        "mods {} and {} both use the namespace {shared}; turn one of them off",
        a.0, b.0
    ))
}

impl Content {
    /// The content of the mod at `path` (a `.hmod` or a mod folder).
    pub fn read(path: &Path, manifest: &Manifest) -> Result<Content, String> {
        Self::read_declaring(path, manifest, &[])
    }
    /// [`Content::read`] for a mod whose native code also declares items (`native_items`):
    /// a file's `crafting_remainder` may name one of those `declared` ids, which are not in the
    /// files.
    pub fn read_declaring(
        path: &Path,
        manifest: &Manifest,
        declared: &[String],
    ) -> Result<Content, String> {
        let (source, sized) = if path.is_dir() {
            let mut out = Vec::new();
            for top in ["assets", "data", "scripts"] {
                walk(path, &path.join(top), &mut out)?;
            }
            (Source::Dir(path.to_owned()), out)
        } else {
            let mut zip = open_zip(path)?;
            let mut out = Vec::new();
            for i in 0..zip.len() {
                let entry = zip.by_index_raw(i).map_err(|e| e.to_string())?;
                let name = entry.name().map_err(|e| e.to_string())?.into_owned();
                let content = ["assets/", "data/", "scripts/"]
                    .iter()
                    .any(|top| name.starts_with(top));
                if content && !entry.is_dir() {
                    out.push((name.to_owned(), entry.size()));
                }
            }
            (Source::Zip(path.to_owned()), out)
        };
        let mut files = Vec::with_capacity(sized.len());
        let mut scripts = Vec::new();
        let mut total = 0u64;
        for (name, size) in sized {
            check_path(&name)?;
            if size > MAX_FILE {
                return Err(format!("{name} is over {} MiB", MAX_FILE >> 20));
            }
            total += size;
            if name.starts_with("scripts/") {
                scripts.push(name);
            } else {
                files.push(name);
            }
        }
        if files.len() + scripts.len() > MAX_FILES {
            return Err(format!("more than {MAX_FILES} content files"));
        }
        if total > MAX_TOTAL {
            return Err(format!("content is over {} MiB", MAX_TOTAL >> 20));
        }
        scripts.sort();
        scripts.dedup();
        // The registry script's declarations join the files, in memory.
        #[cfg_attr(not(feature = "lua"), allow(unused_mut))]
        let mut registered = BTreeMap::new();
        #[cfg(feature = "lua")]
        if scripts.iter().any(|s| s == REGISTRY_SCRIPT) {
            let mut reader = FileReader {
                from: read_from(&source)?,
                registered: &registered,
            };
            let code = reader.read(REGISTRY_SCRIPT)?;
            let code =
                String::from_utf8(code).map_err(|_| format!("{REGISTRY_SCRIPT}: not UTF-8"))?;
            let namespaces: Vec<String> = manifest.owned_namespaces().map(str::to_owned).collect();
            // The modules it may require: scripts/registry/**.lua.
            let mut modules = crate::registry::Modules::new();
            for name in scripts
                .iter()
                .filter(|s| s.starts_with(crate::registry::MODULES) && s.ends_with(".lua"))
            {
                let code = reader.read(name)?;
                let code = String::from_utf8(code).map_err(|_| format!("{name}: not UTF-8"))?;
                modules.insert(name.clone(), code);
            }
            let made = crate::registry::run(&code, &modules, &manifest.id, &namespaces)?;
            for (name, bytes) in made {
                if files.contains(&name) {
                    let lang = name.contains("/lang/");
                    return Err(format!(
                        "{name}: both a file of the mod and registered by {REGISTRY_SCRIPT}{}",
                        if lang {
                            " (registry.lang or registry.asset): use one"
                        } else {
                            ""
                        }
                    ));
                }
                if bytes.len() as u64 > MAX_FILE {
                    return Err(format!("{name}: registered over {} MiB", MAX_FILE >> 20));
                }
                total += bytes.len() as u64;
                files.push(name.clone());
                registered.insert(name, bytes);
            }
            if files.len() + scripts.len() > MAX_FILES {
                return Err(format!("more than {MAX_FILES} content files"));
            }
            if total > MAX_TOTAL {
                return Err(format!("content is over {} MiB", MAX_TOTAL >> 20));
            }
        }
        files.sort();
        files.dedup();
        let mut content = Content {
            source,
            files,
            scripts,
            blocks: Vec::new(),
            items: Vec::new(),
            effects: Vec::new(),
            creative_tabs: Vec::new(),
            entity_types: Vec::new(),
            menu_types: Vec::new(),
            namespaces: manifest.owned_namespaces().map(str::to_owned).collect(),
            registered,
        };
        content.check(manifest, declared)?;
        Ok(content)
    }

    /// Content with no files, for the blocks and items native mods declare
    /// in code in `namespace` ([`Content::declare`]).
    pub fn declared(namespace: &str) -> Content {
        Content {
            source: Source::Code,
            files: Vec::new(),
            scripts: Vec::new(),
            blocks: Vec::new(),
            items: Vec::new(),
            effects: Vec::new(),
            creative_tabs: Vec::new(),
            entity_types: Vec::new(),
            menu_types: Vec::new(),
            registered: BTreeMap::new(),
            namespaces: vec![namespace.to_owned()],
        }
    }

    /// Adds blocks and items a native mod declares in code (`native_blocks`,
    /// `native_items`), checked as declaration files are: ids in this mod's
    /// own namespace (its id, as `data/<id>/horizon/` is) and declared once
    /// with what it already has, a vanilla template, a crafting remainder
    /// that is vanilla or this mod's. All or nothing: on an error nothing is
    /// added.
    pub fn declare(&mut self, blocks: &[Block], items: &[Item]) -> Result<(), String> {
        let own = self
            .namespaces
            .first()
            .map(String::as_str)
            .unwrap_or_default();
        let mut ids: BTreeSet<&str> = self
            .blocks
            .iter()
            .map(|b| b.id.as_str())
            .chain(self.items.iter().map(|i| i.id.as_str()))
            .collect();
        let mut item_ids: BTreeSet<&str> = self
            .blocks
            .iter()
            .filter(|b| b.item)
            .map(|b| b.id.as_str())
            .chain(self.items.iter().map(|i| i.id.as_str()))
            .collect();
        let in_own =
            |id: &str| namespaced_id(id) && id.split_once(':').is_some_and(|(ns, _)| ns == own);
        for block in blocks {
            let id = block.id.as_str();
            if !in_own(id) {
                return Err(format!("block {id:?} is not an id in {own}:"));
            }
            check_block(block).map_err(|e| format!("block {id}: {e}"))?;
            if !ids.insert(id) {
                return Err(format!("block {id} is declared twice"));
            }
            if block.item {
                item_ids.insert(id);
            }
        }
        let all: Vec<&Block> = self.blocks.iter().chain(blocks).collect();
        check_sign_walls(&all)?;
        for item in items {
            let id = item.id.as_str();
            if !in_own(id) {
                return Err(format!("item {id:?} is not an id in {own}:"));
            }
            check_item(item).map_err(|e| format!("item {id}: {e}"))?;
            if !item_ids.insert(id) {
                return Err(format!(
                    "item {id} is declared twice, or is also a block's item"
                ));
            }
        }
        for item in items {
            if let Some(rest) = &item.crafting_remainder
                && (rest == "minecraft:air"
                    || !vanilla_id(rest) && !item_ids.contains(rest.as_str()))
            {
                return Err(format!(
                    "item {}: crafting_remainder {rest:?} is not a vanilla item (minecraft:...) or one of this mod's",
                    item.id
                ));
            }
        }
        self.blocks.extend_from_slice(blocks);
        self.items.extend_from_slice(items);
        Ok(())
    }

    /// Where the files are read from: the mod's folder or its `.hmod`.
    pub fn source(&self) -> &Path {
        match &self.source {
            Source::Dir(path) | Source::Zip(path) => path,
            Source::Code => Path::new(""),
        }
    }

    /// No files: a mod with code only.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// One content file's bytes.
    pub fn file(&self, name: &str) -> Result<Vec<u8>, String> {
        self.reader()?.read(name)
    }

    /// The bytes of a file `scripts/registry.lua` registered: none for a
    /// file of the mod's folder or archive.
    pub fn registered(&self, name: &str) -> Option<&[u8]> {
        self.registered.get(name).map(Vec::as_slice)
    }

    /// A reader of its content files. A packaged mod's archive is opened
    /// once for all of them (opening it reads its whole index).
    pub fn reader(&self) -> Result<FileReader<'_>, String> {
        reader_of(&self.source, &self.registered)
    }

    /// Where content may go, and the declarations.
    fn check(&mut self, manifest: &Manifest, declared: &[String]) -> Result<(), String> {
        let id = manifest.id.as_str();
        if !self.files.is_empty() && !manifest.capabilities.contains(&Capability::Registries) {
            return Err(format!(
                "has assets/ or data/ but not capability registries in {MANIFEST}"
            ));
        }
        let scripted = manifest.capabilities.contains(&Capability::Scripts);
        if !self.scripts.is_empty() && !scripted {
            return Err(format!(
                "has scripts/ but not capability scripts in {MANIFEST}"
            ));
        }
        if scripted && !self.scripts.iter().any(|s| s == SCRIPT_ENTRY) {
            return Err(format!(
                "capability scripts in {MANIFEST} but no {SCRIPT_ENTRY}"
            ));
        }
        let mut blocks = Vec::new();
        let mut items = Vec::new();
        let mut effects = Vec::new();
        let mut tabs = Vec::new();
        let mut types = Vec::new();
        let mut menus = Vec::new();
        for name in &self.files {
            let mut parts = name.splitn(3, '/');
            let (top, namespace, rest) = (
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
            );
            let own = namespace == id;
            let owned = own || manifest.namespaces.iter().any(|n| n == namespace);
            // Vanilla's own files stay vanilla's; a mod may only add to its
            // tags and atlases, and to the convention tags mods share.
            let shared = matches!(namespace, "minecraft" | "c");
            let atlas = top == "assets" && namespace == "minecraft" && rest.starts_with("atlases/");
            let allowed = owned || (top == "data" && shared && rest.starts_with("tags/")) || atlas;
            if !allowed {
                return Err(format!(
                    "{name}: a mod adds files in its own namespaces ({top}/{id}/ and those listed in namespaces), to vanilla and convention (c) tags, or to vanilla's atlases"
                ));
            }
            if !own && top == "data" && rest.starts_with("horizon/") {
                return Err(format!(
                    "{name}: blocks, items, mob effects, entity types and menu types are declared in data/{id}/horizon/"
                ));
            }
            let Some(decl) = (own && top == "data")
                .then(|| rest.strip_prefix("horizon/"))
                .flatten()
            else {
                continue;
            };
            let parse = |kind: &str| -> Option<String> {
                let path = decl
                    .strip_prefix(kind)?
                    .strip_prefix('/')?
                    .strip_suffix(".json")?;
                Some(format!("{id}:{path}"))
            };
            if let Some(block) = parse("block") {
                blocks.push((block, name.clone()));
            } else if let Some(item) = parse("item") {
                items.push((item, name.clone()));
            } else if let Some(effect) = parse("mob_effect") {
                effects.push((effect, name.clone()));
            } else if let Some(tab) = parse("creative_tab") {
                tabs.push((tab, name.clone()));
            } else if let Some(kind) = parse("entity_type") {
                types.push((kind, name.clone()));
            } else if let Some(menu) = parse("menu") {
                menus.push((menu, name.clone()));
            } else {
                return Err(format!(
                    "{name}: data/{id}/horizon/ holds block/, item/, mob_effect/, creative_tab/, entity_type/ and menu/*.json only"
                ));
            }
        }
        // One reader for every declaration: opening a packaged mod reads its
        // whole index, so a reader per file made loading a big mod quadratic.
        let source = self.source.clone();
        let registered = self.registered.clone();
        let mut reader = reader_of(&source, &registered)?;
        let mut ids = BTreeSet::new();
        for (block, name) in blocks {
            let file: BlockFile =
                serde_json::from_slice(&reader.read(&name)?).map_err(|e| format!("{name}: {e}"))?;
            if file
                .connects_to
                .as_ref()
                .is_some_and(|t| !t.starts_with('#'))
            {
                return Err(format!(
                    "{name}: connects_to is a block tag, #namespace:path"
                ));
            }
            let block = Block {
                id: block,
                template: file.template,
                properties: file.properties,
                shapes: file.shapes,
                collision_shapes: file.collision_shapes,
                state_properties: file.state_properties,
                default_state: file.default_state,
                item: file.item,
                components: file.components,
                connects_to: file
                    .connects_to
                    .map(|t| t.trim_start_matches('#').to_owned()),
                thickness: file.thickness,
                wood_type: file.wood_type,
                wall_block: file.wall_block,
            };
            check_block(&block).map_err(|e| format!("{name}: {e}"))?;
            if block.item {
                ids.insert(block.id.clone());
            }
            self.blocks.push(block);
        }
        // After every block, so a wall form may be declared in a later file.
        check_sign_walls(&self.blocks.iter().collect::<Vec<_>>())?;
        let mut remainders = Vec::new();
        for (item, name) in items {
            let file: ItemFile =
                serde_json::from_slice(&reader.read(&name)?).map_err(|e| format!("{name}: {e}"))?;
            let item = Item {
                id: item,
                template: file.template,
                max_stack_size: file.max_stack_size,
                rarity: file.rarity,
                components: file.components,
                crafting_remainder: file.crafting_remainder,
            };
            check_item(&item).map_err(|e| format!("{name}: {e}"))?;
            if !ids.insert(item.id.clone()) {
                return Err(format!("{name}: {} is also a block's item", item.id));
            }
            if let Some(rest) = &item.crafting_remainder {
                remainders.push((name, rest.clone()));
            }
            self.items.push(item);
        }
        for (effect, name) in effects {
            let file: MobEffect =
                serde_json::from_slice(&reader.read(&name)?).map_err(|e| format!("{name}: {e}"))?;
            if file.color > 0xFF_FFFF {
                return Err(format!("{name}: color is RGB, 0 to 16777215 (0xFFFFFF)"));
            }
            self.effects.push(MobEffect { id: effect, ..file });
        }
        // After every item, so a remainder may be declared in a later file.
        for (name, rest) in remainders {
            if rest == "minecraft:air" {
                return Err(format!(
                    "{name}: crafting_remainder {rest:?} is no remainder (leave it out)"
                ));
            }
            if !vanilla_id(&rest) && !ids.contains(&rest) && !declared.contains(&rest) {
                return Err(format!(
                    "{name}: crafting_remainder {rest:?} is not a vanilla item (minecraft:...) or one of this mod's"
                ));
            }
        }
        for (tab, name) in tabs {
            let file: CreativeTabFile =
                serde_json::from_slice(&reader.read(&name)?).map_err(|e| format!("{name}: {e}"))?;
            let id_like = |s: &str| {
                s.split_once(':')
                    .is_some_and(|(n, p)| !n.is_empty() && !p.is_empty())
            };
            if !id_like(&file.icon) || !file.items.iter().all(|i| id_like(i)) {
                return Err(format!(
                    "{name}: icon and items are item ids (namespace:path)"
                ));
            }
            self.creative_tabs.push(CreativeTab {
                id: tab,
                title: file.title,
                icon: file.icon,
                items: file.items,
            });
        }
        for (kind, name) in types {
            let file: EntityType =
                serde_json::from_slice(&reader.read(&name)?).map_err(|e| format!("{name}: {e}"))?;
            let thrown = ENTITY_TEMPLATES.contains(&file.template.as_str());
            if !thrown && !MOB_TEMPLATES.contains(&file.template.as_str()) {
                return Err(format!(
                    "{name}: template {:?} is not one of {ENTITY_TEMPLATES:?} or a vanilla mob (minecraft:zombie, minecraft:pig...)",
                    file.template
                ));
            }
            if thrown {
                // its own item, so no vanilla item loses its use to it.
                let Some(item) = &file.item else {
                    return Err(format!(
                        "{name}: item names one of this mod's items, which throws it"
                    ));
                };
                if !ids.contains(item) {
                    return Err(format!(
                        "{name}: item {item:?} is not one of this mod's items"
                    ));
                }
                if self
                    .entity_types
                    .iter()
                    .any(|t| t.item.as_ref() == Some(item))
                {
                    return Err(format!(
                        "{name}: item {item:?} already throws another entity type"
                    ));
                }
                if !(file.damage.is_finite() && file.damage >= 0.0) {
                    return Err(format!("{name}: damage is 0 or more"));
                }
                if !file.attributes.is_empty()
                    || file.size.is_some()
                    || file.texture.is_some()
                    || file.goals != Goals::Template
                    || !file.flags.is_empty()
                {
                    return Err(format!(
                        "{name}: attributes, size, texture, goals and flags are for a mob template"
                    ));
                }
            } else {
                if file.item.is_some() || file.damage != 0.0 {
                    return Err(format!("{name}: item and damage are for a thrown template"));
                }
                for (key, value) in &file.attributes {
                    if key.is_empty() || key.contains(':') || !value.is_finite() {
                        return Err(format!(
                            "{name}: attribute {key:?} is a name without a namespace (max_health, movement_speed...) and a finite number"
                        ));
                    }
                }
                if let Some(size) = &file.size
                    && !(size.width.is_finite()
                        && size.width > 0.0
                        && size.height.is_finite()
                        && size.height > 0.0
                        && size.eye_height.is_none_or(|e| e.is_finite() && e >= 0.0))
                {
                    return Err(format!("{name}: size has a positive width and height"));
                }
                if let Some(flag) = file.flags.keys().find(|f| !MOB_FLAGS.contains(&f.as_str())) {
                    return Err(format!("{name}: flag {flag:?} is not one of {MOB_FLAGS:?}"));
                }
                if let Some(texture) = &file.texture {
                    let Some((ns, path)) = texture.split_once(':') else {
                        return Err(format!(
                            "{name}: texture is <namespace>:entity/<name>, a file of this mod"
                        ));
                    };
                    let file_name = format!("assets/{ns}/textures/{path}.png");
                    if !self.namespaces.iter().any(|n| n == ns) || !self.files.contains(&file_name)
                    {
                        return Err(format!(
                            "{name}: texture {texture:?} is not a file of this mod ({file_name})"
                        ));
                    }
                }
                if let Some(model) = &file.model {
                    let (ns, file_name) =
                        mob_model_file(model).map_err(|e| format!("{name}: {e}"))?;
                    if !self.namespaces.iter().any(|n| n == ns) || !self.files.contains(&file_name)
                    {
                        return Err(format!(
                            "{name}: model {model:?} is not a file of this mod ({file_name})"
                        ));
                    }
                }
                if let Some(animation) = &file.animation
                    && !MOB_ANIMATIONS.contains(&animation.as_str())
                {
                    return Err(format!(
                        "{name}: animation {animation:?} is not one of the game's sets ({})",
                        MOB_ANIMATIONS.join(", ")
                    ));
                }
                for (key, sound) in &file.sounds {
                    if !MOB_SOUNDS.contains(&key.as_str()) {
                        return Err(format!(
                            "{name}: sounds has {key:?}; the keys are {MOB_SOUNDS:?}"
                        ));
                    }
                    if !namespaced_id(sound) {
                        return Err(format!(
                            "{name}: sound {sound:?} is a namespaced sound event (minecraft:entity.zombie.ambient)"
                        ));
                    }
                }
            }
            self.entity_types.push(EntityType { id: kind, ..file });
        }
        for (menu, name) in menus {
            let file: MenuType =
                serde_json::from_slice(&reader.read(&name)?).map_err(|e| format!("{name}: {e}"))?;
            if !(1..=MAX_MENU_SLOTS).contains(&file.slots) || file.data_slots > MAX_MENU_SLOTS {
                return Err(format!(
                    "{name}: slots is 1 to {MAX_MENU_SLOTS}, data_slots 0 to {MAX_MENU_SLOTS}"
                ));
            }
            let mut seen = BTreeSet::new();
            for &r in &file.results {
                if r >= file.slots {
                    return Err(format!(
                        "{name}: result {r} is past the last slot, {}",
                        file.slots - 1
                    ));
                }
                if !seen.insert(r) {
                    return Err(format!("{name}: result {r} is listed twice"));
                }
            }
            self.menu_types.push(MenuType { id: menu, ..file });
        }
        Ok(())
    }
}

/// Namespaced component types, removals as `{}`, and none of `fields`,
/// which the file sets with its own keys.
fn check_components(
    components: &BTreeMap<String, serde_json::Value>,
    fields: &[&str],
) -> Result<(), String> {
    for (key, value) in components {
        let (removed, kind) = match key.strip_prefix('!') {
            Some(kind) => (true, kind),
            None => (false, key.as_str()),
        };
        if !namespaced_id(kind) {
            return Err(format!("component {key:?} must be namespaced and valid"));
        }
        if let Some(own) = kind
            .strip_prefix("minecraft:")
            .filter(|k| fields.contains(k))
        {
            return Err(format!("component {key:?}: set {own} instead"));
        }
        if removed && !value.as_object().is_some_and(|o| o.is_empty()) {
            return Err(format!("component {key:?}: a removal's value is {{}}"));
        }
    }
    Ok(())
}

/// An item's fields, as a file or code gives them.
fn check_item(item: &Item) -> Result<(), String> {
    if !vanilla_id(&item.template) {
        return Err(format!(
            "template {:?} is not a vanilla item (minecraft:...)",
            item.template
        ));
    }
    if item.max_stack_size.is_some_and(|n| !(1..=99).contains(&n)) {
        return Err("max_stack_size is 1 to 99".into());
    }
    check_components(&item.components, &["max_stack_size", "rarity"])
}

/// A block's fields, as a file or code gives them (`connects_to` without
/// its `#`).
fn check_block(file: &Block) -> Result<(), String> {
    if !vanilla_id(&file.template) {
        return Err(format!(
            "template {:?} is not a vanilla block (minecraft:...)",
            file.template
        ));
    }
    for shapes in file.shapes.iter().chain(file.collision_shapes.iter()) {
        shapes.check()?;
    }
    for (name, property) in &file.state_properties {
        if !state_name(name) {
            return Err(format!(
                "state property {name:?} is lower case letters, digits and underscores"
            ));
        }
        let values = property.values();
        if values.is_empty() {
            return Err(format!("state property {name:?} needs a value"));
        }
        let mut seen = BTreeSet::new();
        for value in values {
            if !state_name(value) {
                return Err(format!(
                    "state property {name:?}: value {value:?} is lower case letters, digits and underscores"
                ));
            }
            if !seen.insert(value) {
                return Err(format!("state property {name:?}: value {value:?} repeats"));
            }
        }
    }
    for key in file.default_state.keys() {
        if !state_name(key) {
            return Err(format!(
                "default_state key {key:?} is lower case letters, digits and underscores"
            ));
        }
    }
    let p = &file.properties;
    if let Some(RandomTicks::States { property, values }) = &p.random_ticks {
        let declared = file
            .state_properties
            .get(property)
            .map(StateProperty::values);
        if declared.is_none_or(|all| {
            values.is_empty() || values.iter().any(|v| !all.contains(&v.as_str()))
        }) {
            return Err(format!(
                "random_ticks needs values of the block's own state property {property:?} (state_properties)"
            ));
        }
    }
    let finite = |v: Option<f32>| v.is_none_or(f32::is_finite);
    if !finite(p.hardness) || !finite(p.explosion_resistance) || !finite(p.friction) {
        return Err("hardness, explosion_resistance and friction are numbers".into());
    }
    if p.hardness.is_some_and(|h| h < 0.0 && h != -1.0) {
        return Err("hardness is 0 or more, or -1 (unbreakable)".into());
    }
    if p.explosion_resistance.is_some_and(|r| r < 0.0) {
        return Err("explosion_resistance is 0 or more".into());
    }
    if p.friction.is_some_and(|f| !(0.0..=2.0).contains(&f)) {
        return Err("friction is 0 to 2".into());
    }
    if p.light.is_some_and(|l| l > 15) {
        return Err("light is 0 to 15".into());
    }
    if p.map_color.is_some_and(|c| c > 61) {
        return Err("map_color is a MapColor id, 0 to 61".into());
    }
    if p.sounds.as_deref().is_some_and(|s| !vanilla_id(s)) {
        return Err("sounds names a vanilla block (minecraft:...)".into());
    }
    if !file.item && !file.components.is_empty() {
        return Err("item is false, so the block has no item to take components".into());
    }
    check_components(&file.components, &[])?;
    // A block file has no `max_stack_size` / `rarity` keys, so those two take the same checks here.
    if let Some(n) = file.components.get("minecraft:max_stack_size")
        && !n.as_u64().is_some_and(|n| (1..=99).contains(&n))
    {
        return Err("component \"minecraft:max_stack_size\" is an integer 1 to 99".into());
    }
    if let Some(r) = file.components.get("minecraft:rarity")
        && serde_json::from_value::<Rarity>(r.clone()).is_err()
    {
        return Err("component \"minecraft:rarity\" is common, uncommon, rare or epic".into());
    }
    if let Some(tag) = &file.connects_to {
        if file.template != "minecraft:chorus_plant" {
            return Err("connects_to needs template minecraft:chorus_plant (a PipeBlock)".into());
        }
        let id = tag.as_str();
        let valid = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-./".contains(c))
        };
        if !id
            .split_once(':')
            .is_some_and(|(ns, path)| valid(ns) && valid(path))
        {
            return Err("connects_to is a block tag, #namespace:path".into());
        }
    }
    match file.thickness {
        Some(_) if file.connects_to.is_none() => {
            return Err("thickness is a connecting block's (connects_to)".into());
        }
        Some(t) if !(2.0..=14.0).contains(&t) => return Err("thickness is 2 to 14 pixels".into()),
        Some(_) if file.shapes.is_some() => {
            return Err("thickness sets the shapes, so a block has one or the other".into());
        }
        _ => {}
    }
    // Every vanilla `SignBlock`'s id ends in `_sign`, and no other block's does.
    let sign = file.template.ends_with("_sign");
    if let Some(wood) = &file.wood_type {
        if !sign {
            return Err("wood_type needs a vanilla sign template".into());
        }
        let (namespace, path) = wood.split_once(':').unwrap_or(("minecraft", wood));
        if !namespaced_id(&format!("{namespace}:{path}"))
            || check_path(&format!("assets/{namespace}/{path}")).is_err()
        {
            return Err("wood_type is a name, or namespace:path, in lower case".into());
        }
    }
    if let Some(wall) = &file.wall_block {
        if !sign || file.template.contains("_wall_") {
            return Err("wall_block needs a standing or ceiling sign template".into());
        }
        if !file.item {
            return Err("wall_block is where the item places the block, and item is false".into());
        }
        if !namespaced_id(wall) {
            return Err("wall_block is a block id, namespace:path".into());
        }
    }
    Ok(())
}

/// A sign's `wall_block` is one of the mod's own blocks: the wall form of its
/// kind, with no item of its own and no other sign. Checked here, a pair that
/// doesn't fit leaves out its mod alone.
fn check_sign_walls(blocks: &[&Block]) -> Result<(), String> {
    let mut paired = BTreeMap::new();
    for sign in blocks {
        let Some(wall) = &sign.wall_block else {
            continue;
        };
        let id = &sign.id;
        let form = if sign.template.ends_with("_hanging_sign") {
            "_wall_hanging_sign"
        } else {
            "_wall_sign"
        };
        let Some(block) = blocks.iter().find(|b| b.id == *wall) else {
            return Err(format!(
                "{id}: wall_block {wall} is not one of this mod's blocks"
            ));
        };
        if !block.template.ends_with(form) {
            return Err(format!(
                "{id}: wall_block {wall} needs a template ending in {form}"
            ));
        }
        if block.item {
            return Err(format!(
                "{id}: wall_block {wall} has this sign's item, so its own \"item\" is false"
            ));
        }
        if let Some(other) = paired.insert(wall, id) {
            return Err(format!(
                "{id}: wall_block {wall} is already paired with {other}"
            ));
        }
    }
    Ok(())
}

fn reader_of<'a>(
    source: &'a Source,
    registered: &'a BTreeMap<String, Vec<u8>>,
) -> Result<FileReader<'a>, String> {
    Ok(FileReader {
        from: read_from(source)?,
        registered,
    })
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).map_err(|e| e.to_string())
}

/// Files under `dir`, relative to `root` with `/` separators. Symbolic links
/// are not followed: a mod folder holds its own files.
fn read_from(source: &Source) -> Result<ReadFrom<'_>, String> {
    Ok(match source {
        Source::Dir(root) => ReadFrom::Dir(root),
        Source::Zip(path) => ReadFrom::Zip(Box::new(open_zip(path)?)),
        Source::Code => ReadFrom::Dir(Path::new("")),
    })
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> Result<(), String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in read.flatten() {
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            walk(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let parts: Option<Vec<&str>> = rel.iter().map(|p| p.to_str()).collect();
            let name = parts.ok_or_else(|| format!("{} is not UTF-8", rel.display()))?;
            out.push((name.join("/"), meta.len()));
        }
        if out.len() > MAX_FILES {
            return Err(format!("more than {MAX_FILES} content files"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Package;

    const MANIFEST_TEXT: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["registries"]
"#;
    const BLOCK: &[u8] = br#"{"template": "minecraft:iron_block",
        "properties": {"hardness": 4.0, "light": 3, "sounds": "minecraft:amethyst_block"}}"#;
    const ITEM: &[u8] = br#"{"template": "minecraft:diamond", "rarity": "uncommon"}"#;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("horizon-content-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    #[test]
    fn shape_tables_validate_tuples_defaults_and_collision() {
        let content = hmod("shape-tuples", MANIFEST_TEXT, &[(
            "data/ruby/horizon/block/a.json",
            br#"{"template":"minecraft:stone","shapes":{"properties":["facing","count"],"values":{"north,2":[[0,0,0,1,0.5,1]]},"default":[[0,0,0,1,1,1]]},"collision_shapes":{"default":[]}}"#,
        )]).unwrap();
        let block = &content.blocks[0];
        assert_eq!(
            block.shapes.as_ref().unwrap().property_names(),
            ["facing", "count"]
        );
        assert_eq!(
            block.collision_shapes.as_ref().unwrap().default,
            Some(vec![])
        );
        for value in [
            r#"{"property":"axis","properties":["count"],"default":[]}"#,
            r#"{"properties":["axis","axis"],"default":[]}"#,
            r#"{"properties":["Axis"],"default":[]}"#,
            r#"{"properties":["axis","count"],"values":{"x":[]}}"#,
            r#"{"properties":["axis"],"values":{"":[]}}"#,
            r#"{"values":{"x":[]},"default":[]}"#,
            r#"{"default":[[1,0,0,0,1,1]]}"#,
        ] {
            let shapes: BlockShapes = serde_json::from_str(value).unwrap();
            assert!(shapes.check().is_err(), "{value}");
            let declaration =
                format!(r#"{{"template":"minecraft:stone","collision_shapes":{value}}}"#);
            assert!(
                hmod(
                    "bad-collision",
                    MANIFEST_TEXT,
                    &[("data/ruby/horizon/block/a.json", declaration.as_bytes(),)]
                )
                .is_err(),
                "{value}"
            );
        }
        let oversized = BlockShapes {
            property: String::new(),
            properties: vec![],
            values: BTreeMap::new(),
            default: Some(vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]; 17]),
        };
        assert!(oversized.check().is_err());
        let mut nonfinite = oversized;
        nonfinite.default = Some(vec![[f64::NAN; 6]]);
        assert!(nonfinite.check().is_err());
    }

    /// A mod folder holding `files`, read as content.
    #[cfg(feature = "lua")]
    fn folder(name: &str, manifest: &str, files: &[(&str, &[u8])]) -> Result<Content, String> {
        let dir = scratch(name);
        std::fs::write(dir.join(MANIFEST), manifest).unwrap();
        for (path, bytes) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let package = Package::read(&dir)?;
        Content::read(&dir, &package.manifest)
    }

    #[cfg(feature = "lua")]
    #[test]
    fn the_registry_script_declares_content_like_the_files_do() {
        const MANIFEST_SCRIPTED: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["registries", "scripts"]
experimental = ["scripts"]
"#;
        let registry = br#"
            registry.entity_type("crawler", { template = "minecraft:zombie", attributes = { max_health = 30 }, goals = "none" })
            registry.block("hive", { template = "minecraft:sculk", properties = { hardness = 1.5 }, item = true })
            registry.data("loot_table/blocks/hive", { type = "minecraft:block", pools = {} })
            registry.data("minecraft:tags/block/mineable/hoe", { values = { "ruby:hive" } })
        "#;
        let content = folder(
            "registry",
            MANIFEST_SCRIPTED,
            &[
                ("scripts/init.lua", b""),
                ("scripts/registry.lua", registry),
                ("data/ruby/horizon/item/ruby.json", ITEM),
            ],
        )
        .unwrap();
        assert_eq!(content.entity_types.len(), 1);
        assert_eq!(content.entity_types[0].id, "ruby:crawler");
        assert_eq!(content.entity_types[0].template, "minecraft:zombie");
        assert_eq!(content.blocks.len(), 1);
        assert_eq!(content.blocks[0].id, "ruby:hive");
        assert_eq!(content.items.len(), 1);
        assert_eq!(
            content.files,
            [
                "data/minecraft/tags/block/mineable/hoe.json",
                "data/ruby/horizon/block/hive.json",
                "data/ruby/horizon/entity_type/crawler.json",
                "data/ruby/horizon/item/ruby.json",
                "data/ruby/loot_table/blocks/hive.json",
            ]
        );
        // Registered files read like the others, through the reader too.
        let bytes = content
            .file("data/ruby/loot_table/blocks/hive.json")
            .unwrap();
        assert!(bytes.starts_with(b"{"));
        let mut reader = content.reader().unwrap();
        assert_eq!(
            reader.read("data/ruby/horizon/item/ruby.json").unwrap(),
            ITEM
        );
        assert!(
            reader
                .read("data/ruby/horizon/entity_type/crawler.json")
                .is_ok()
        );
        // A path both in the folder and registered is refused, as is a bad
        // declaration and a failing script.
        let e = folder(
            "registry-clash",
            MANIFEST_SCRIPTED,
            &[
                ("scripts/init.lua", b""),
                (
                    "scripts/registry.lua",
                    b"registry.item('ruby', { template = 'minecraft:diamond' })",
                ),
                ("data/ruby/horizon/item/ruby.json", ITEM),
            ],
        )
        .unwrap_err();
        assert!(e.contains("both a file of the mod and registered"), "{e}");
        let e = folder(
            "registry-bad",
            MANIFEST_SCRIPTED,
            &[
                ("scripts/init.lua", b""),
                (
                    "scripts/registry.lua",
                    b"registry.block('hive', { template = 'minecraft:sculk', connects_to = 'x' })",
                ),
            ],
        )
        .unwrap_err();
        assert!(
            e.contains("data/ruby/horizon/block/hive.json") && e.contains("connects_to"),
            "{e}"
        );
        let e = folder(
            "registry-fail",
            MANIFEST_SCRIPTED,
            &[
                ("scripts/init.lua", b""),
                ("scripts/registry.lua", b"error('no')"),
            ],
        )
        .unwrap_err();
        assert!(
            e.starts_with("scripts/registry.lua:") && e.contains("no"),
            "{e}"
        );
        // The script's modules come from the mod, in a folder and a .hmod
        // alike, and registry.lang meets the mod's own lang file: use one.
        let modules: &[(&str, &[u8])] = &[
            ("scripts/init.lua", b""),
            ("scripts/registry.lua", b"require('@registry/blocks')"),
            (
                "scripts/registry/blocks.lua",
                b"registry.block('hive', { template = 'minecraft:sculk' })\nregistry.lang('en_us', { ['block.ruby.hive'] = 'Hive' })",
            ),
        ];
        for content in [
            folder("registry-modules", MANIFEST_SCRIPTED, modules).unwrap(),
            hmod("registry-modules-hmod", MANIFEST_SCRIPTED, modules).unwrap(),
        ] {
            assert_eq!(content.blocks[0].id, "ruby:hive");
            assert_eq!(
                content.files,
                [
                    "assets/ruby/lang/en_us.json",
                    "data/ruby/horizon/block/hive.json"
                ]
            );
        }
        let mut clash = modules.to_vec();
        clash.push(("assets/ruby/lang/en_us.json", b"{}"));
        let e = folder("registry-lang-clash", MANIFEST_SCRIPTED, &clash).unwrap_err();
        assert!(
            e.contains("assets/ruby/lang/en_us.json") && e.contains("use one"),
            "{e}"
        );
    }

    fn hmod(name: &str, manifest: &str, files: &[(&str, &[u8])]) -> Result<Content, String> {
        let dir = scratch(name);
        let path = dir.join("m.hmod");
        std::fs::write(&path, Package::write_files(manifest, None, files)?).unwrap();
        let package = Package::read(&path);
        let content = package.and_then(|p| Content::read(&path, &p.manifest));
        std::fs::remove_dir_all(&dir).unwrap();
        content
    }

    #[test]
    fn blocks_and_items_declared_in_code_pass_the_files_checks() {
        let block = |id: &str| Block {
            id: id.into(),
            template: "minecraft:iron_block".into(),
            item: true,
            ..Block::default()
        };
        let item = |id: &str| Item {
            id: id.into(),
            template: "minecraft:diamond".into(),
            ..Item::default()
        };
        let mut content = Content::declared("ruby");
        assert!(content.is_empty());
        let wrench = Item {
            crafting_remainder: Some("ruby:gear".into()),
            ..item("ruby:wrench")
        };
        content
            .declare(&[block("ruby:gear"), block("ruby:casing")], &[wrench])
            .unwrap();
        assert_eq!(content.blocks.len(), 2);
        assert_eq!(content.items[0].id, "ruby:wrench");
        // Each refusal adds nothing.
        let refused = |blocks: &[Block], items: &[Item]| {
            let mut c = Content::declared("ruby");
            c.declare(&[block("ruby:gear")], &[]).unwrap();
            let e = c.declare(blocks, items).unwrap_err();
            assert_eq!((c.blocks.len(), c.items.len()), (1, 0), "{e}");
            e
        };
        assert!(refused(&[block("emerald:gear")], &[]).contains("not an id in ruby:"));
        assert!(refused(&[block("minecraft:gear")], &[]).contains("not an id in ruby:"));
        assert!(refused(&[], &[item("ruby")]).contains("not an id in ruby:"));
        assert!(refused(&[block("ruby:gear")], &[]).contains("declared twice"));
        assert!(refused(&[], &[item("ruby:gear")]).contains("block's item"));
        let modded = Block {
            template: "ruby:other".into(),
            ..block("ruby:casing")
        };
        assert!(refused(&[modded], &[]).contains("not a vanilla block"));
        let dim = Block {
            properties: BlockProperties {
                light: Some(16),
                ..BlockProperties::default()
            },
            ..block("ruby:lamp")
        };
        assert!(refused(&[dim], &[]).contains("light is 0 to 15"));
        let stack = Item {
            max_stack_size: Some(100),
            ..item("ruby:ingot")
        };
        assert!(refused(&[], &[stack]).contains("max_stack_size"));
        let rest = Item {
            crafting_remainder: Some("emerald:dust".into()),
            ..item("ruby:dust")
        };
        assert!(refused(&[block("ruby:pipe")], &[rest]).contains("crafting_remainder"));
        // A cable's tag comes bare from code.
        let cable = Block {
            template: "minecraft:chorus_plant".into(),
            connects_to: Some("ruby:cables".into()),
            ..block("ruby:cable")
        };
        content.declare(&[cable], &[]).unwrap();
    }

    #[test]
    fn a_connecting_block_file_names_its_tag_with_a_hash() {
        let cable = br#"{"template": "minecraft:chorus_plant", "connects_to": "ruby:cables"}"#;
        let files: &[(&str, &[u8])] = &[("data/ruby/horizon/block/cable.json", cable)];
        let e = hmod("bare-tag", MANIFEST_TEXT, files).unwrap_err();
        assert!(e.contains("#namespace:path"), "{e}");
    }

    #[test]
    fn content_only_mod_round_trips() {
        let files: &[(&str, &[u8])] = &[
            ("data/ruby/horizon/block/ruby_block.json", BLOCK),
            ("data/ruby/horizon/item/ruby.json", ITEM),
            ("data/ruby/recipe/ruby_block.json", b"{}"),
            ("data/minecraft/tags/block/mineable/pickaxe.json", b"{}"),
            ("data/c/tags/item/gems/ruby.json", b"{}"),
            ("assets/ruby/textures/block/ruby_block.png", b"png"),
            ("assets/minecraft/atlases/blocks.json", b"{}"),
        ];
        let content = hmod("ok", MANIFEST_TEXT, files).unwrap();
        assert_eq!(content.files.len(), 7);
        assert_eq!(
            content.blocks,
            [Block {
                id: "ruby:ruby_block".into(),
                template: "minecraft:iron_block".into(),
                shapes: None,
                collision_shapes: None,
                state_properties: BTreeMap::new(),
                default_state: BTreeMap::new(),
                properties: BlockProperties {
                    hardness: Some(4.0),
                    light: Some(3),
                    sounds: Some("minecraft:amethyst_block".into()),
                    ..BlockProperties::default()
                },
                item: true,
                components: BTreeMap::new(),
                connects_to: None,
                thickness: None,
                wood_type: None,
                wall_block: None,
            }]
        );
        assert_eq!(content.items[0].id, "ruby:ruby");
        assert_eq!(content.items[0].rarity, Some(Rarity::Uncommon));
        assert!(content.items[0].components.is_empty());
        assert_eq!(content.items[0].crafting_remainder, None);
    }

    #[test]
    fn items_name_their_crafting_remainder() {
        let files: &[(&str, &[u8])] = &[
            (
                "data/ruby/horizon/item/a_stew.json",
                br#"{"template": "minecraft:mushroom_stew", "crafting_remainder": "minecraft:bowl"}"#,
            ),
            (
                "data/ruby/horizon/item/b_flask.json",
                br#"{"template": "minecraft:potion", "crafting_remainder": "ruby:c_flask"}"#,
            ),
            ("data/ruby/horizon/item/c_flask.json", ITEM),
        ];
        let content = hmod("remainder", MANIFEST_TEXT, files).unwrap();
        let rests: Vec<_> = content
            .items
            .iter()
            .map(|i| i.crafting_remainder.as_deref())
            .collect();
        assert_eq!(rests, [Some("minecraft:bowl"), Some("ruby:c_flask"), None]);
    }

    #[test]
    fn a_file_item_may_leave_a_code_declared_item_as_its_remainder() {
        // `native_items` declares ruby:wrench in code; a file item names it as its remainder.
        let files: &[(&str, &[u8])] = &[(
            "data/ruby/horizon/item/tool.json",
            br#"{"template": "minecraft:iron_pickaxe", "crafting_remainder": "ruby:wrench"}"#,
        )];
        let read = |declared: &[String]| {
            let dir = scratch("declared-remainder");
            let path = dir.join("m.hmod");
            std::fs::write(
                &path,
                Package::write_files(MANIFEST_TEXT, None, files).unwrap(),
            )
            .unwrap();
            let package = Package::read(&path).unwrap();
            let content = Content::read_declaring(&path, &package.manifest, declared);
            std::fs::remove_dir_all(&dir).unwrap();
            content
        };
        assert!(read(&[]).is_err(), "a remainder nobody declares is refused");
        let content = read(&["ruby:wrench".to_owned()]).expect("the code-declared item is known");
        assert_eq!(
            content.items[0].crafting_remainder.as_deref(),
            Some("ruby:wrench")
        );
    }

    #[test]
    fn items_patch_their_template_components() {
        let sword = br#"{"template": "minecraft:iron_sword", "components": {
            "minecraft:max_damage": 900,
            "minecraft:attribute_modifiers": [{"type": "minecraft:attack_damage",
                "id": "minecraft:base_attack_damage", "amount": 7.0,
                "operation": "add_value", "slot": "mainhand"}],
            "!minecraft:repairable": {}}}"#;
        let files: &[(&str, &[u8])] = &[("data/ruby/horizon/item/ruby_sword.json", sword)];
        let content = hmod("components", MANIFEST_TEXT, files).unwrap();
        let components = &content.items[0].components;
        assert_eq!(
            components.keys().collect::<Vec<_>>(),
            [
                "!minecraft:repairable",
                "minecraft:attribute_modifiers",
                "minecraft:max_damage"
            ]
        );
        assert_eq!(components["minecraft:max_damage"], 900);
    }

    #[test]
    fn mob_effects_take_a_category_and_a_colour() {
        let files: &[(&str, &[u8])] = &[
            (
                "data/ruby/horizon/mob_effect/glow.json",
                br#"{"category": "beneficial", "color": 16711680}"#,
            ),
            (
                "data/ruby/horizon/mob_effect/curse/dark.json",
                br#"{"category": "harmful", "color": 0}"#,
            ),
        ];
        let content = hmod("effects", MANIFEST_TEXT, files).unwrap();
        let effects: Vec<_> = content
            .effects
            .iter()
            .map(|e| (e.id.as_str(), e.category, e.color))
            .collect();
        assert_eq!(
            effects,
            [
                ("ruby:curse/dark", EffectCategory::Harmful, 0),
                ("ruby:glow", EffectCategory::Beneficial, 0xFF0000),
            ]
        );
        for (case, file, why) in [
            (
                "effect-colour",
                r#"{"category": "neutral", "color": 16777216}"#,
                "color is RGB",
            ),
            (
                "effect-category",
                r#"{"category": "good", "color": 1}"#,
                "unknown variant",
            ),
            (
                "effect-key",
                r#"{"category": "neutral", "color": 1, "id": "x"}"#,
                "unknown field",
            ),
            (
                "effect-missing",
                r#"{"category": "neutral"}"#,
                "missing field",
            ),
        ] {
            let files: &[(&str, &[u8])] =
                &[("data/ruby/horizon/mob_effect/a.json", file.as_bytes())];
            let e = hmod(case, MANIFEST_TEXT, files).unwrap_err();
            assert!(e.contains(why), "{case}: {e}");
        }
    }

    #[test]
    fn menu_types_take_slots_data_slots_and_results() {
        let files: &[(&str, &[u8])] = &[
            (
                "data/ruby/horizon/menu/press.json",
                br#"{"slots": 9, "data_slots": 2, "results": [7, 8]}"#,
            ),
            ("data/ruby/horizon/menu/bag.json", br#"{"slots": 4}"#),
        ];
        let content = hmod("menus", MANIFEST_TEXT, files).unwrap();
        let menus: Vec<_> = content
            .menu_types
            .iter()
            .map(|m| (m.id.as_str(), m.slots, m.data_slots, m.results.clone()))
            .collect();
        assert_eq!(
            menus,
            [("ruby:bag", 4, 0, vec![]), ("ruby:press", 9, 2, vec![7, 8])]
        );
        for (case, file, why) in [
            ("menu-empty", r#"{"slots": 0}"#, "slots is 1 to 256"),
            ("menu-large", r#"{"slots": 257}"#, "slots is 1 to 256"),
            (
                "menu-data",
                r#"{"slots": 1, "data_slots": 257}"#,
                "data_slots 0 to 256",
            ),
            (
                "menu-result",
                r#"{"slots": 3, "results": [3]}"#,
                "result 3 is past the last slot, 2",
            ),
            (
                "menu-twice",
                r#"{"slots": 3, "results": [1, 1]}"#,
                "result 1 is listed twice",
            ),
            ("menu-key", r#"{"slots": 3, "rows": 1}"#, "unknown field"),
            ("menu-missing", r#"{"data_slots": 1}"#, "missing field"),
        ] {
            let files: &[(&str, &[u8])] = &[("data/ruby/horizon/menu/a.json", file.as_bytes())];
            let e = hmod(case, MANIFEST_TEXT, files).unwrap_err();
            assert!(e.contains(why), "{case}: {e}");
        }
    }

    #[test]
    fn a_mob_entity_type_copies_a_vanilla_mob_with_its_own_values() {
        let png: &[u8] = b"\x89PNG";
        let parasite = br#"{"template": "minecraft:zombie", "attributes": {"max_health": 30, "movement_speed": 0.3},
            "size": {"width": 0.7, "height": 2.1}, "texture": "ruby:entity/parasite", "goals": "none",
            "flags": {"burns_in_day": false}}"#;
        let files: &[(&str, &[u8])] = &[
            ("assets/ruby/textures/entity/parasite.png", png),
            ("data/ruby/horizon/entity_type/parasite.json", parasite),
            (
                "data/ruby/horizon/entity_type/boar.json",
                br#"{"template": "minecraft:pig"}"#,
            ),
        ];
        let content = hmod("mobs", MANIFEST_TEXT, files).unwrap();
        // Types come sorted by name.
        let [boar, parasite] = content.entity_types.as_slice() else {
            panic!("{:?}", content.entity_types);
        };
        assert!(parasite.is_mob() && boar.is_mob());
        assert_eq!(
            (parasite.id.as_str(), parasite.template.as_str()),
            ("ruby:parasite", "minecraft:zombie")
        );
        assert_eq!(parasite.attributes["max_health"], 30.0);
        assert_eq!(
            parasite.size.map(|s| (s.width, s.height, s.eye_height)),
            Some((0.7, 2.1, None))
        );
        assert_eq!(parasite.texture.as_deref(), Some("ruby:entity/parasite"));
        assert_eq!(parasite.goals, Goals::None);
        assert_eq!(parasite.flags.get("burns_in_day"), Some(&false));
        assert_eq!(
            (boar.goals, boar.texture.as_deref(), boar.item.as_deref()),
            (Goals::Template, None, None)
        );
        for (case, file, why) in [
            (
                "mob-item",
                r#"{"template": "minecraft:zombie", "item": "ruby:gem"}"#,
                "item and damage are for a thrown template",
            ),
            (
                "mob-attribute-namespace",
                r#"{"template": "minecraft:zombie", "attributes": {"minecraft:max_health": 1}}"#,
                "without a namespace",
            ),
            (
                "mob-size",
                r#"{"template": "minecraft:zombie", "size": {"width": 0, "height": 1}}"#,
                "positive width and height",
            ),
            (
                "mob-texture-missing",
                r#"{"template": "minecraft:zombie", "texture": "ruby:entity/ghost"}"#,
                "is not a file of this mod",
            ),
            (
                "mob-texture-namespace",
                r#"{"template": "minecraft:zombie", "texture": "entity/parasite"}"#,
                "<namespace>:entity/<name>",
            ),
            (
                "mob-goals",
                r#"{"template": "minecraft:zombie", "goals": "mine"}"#,
                "unknown variant",
            ),
            (
                "mob-model-missing",
                r#"{"template": "minecraft:zombie", "model": "ruby:entity/ghost"}"#,
                "is not a file of this mod",
            ),
            (
                "mob-model-other-namespace",
                r#"{"template": "minecraft:zombie", "model": "minecraft:entity/zombie"}"#,
                "is not a file of this mod",
            ),
            (
                "mob-model-not-entity",
                r#"{"template": "minecraft:zombie", "model": "ruby:block/crawler"}"#,
                "<namespace>:entity/<name>",
            ),
            (
                "mob-model-escapes",
                r#"{"template": "minecraft:zombie", "model": "ruby:entity/../../x"}"#,
                "<namespace>:entity/<name>",
            ),
            (
                "mob-animation",
                r#"{"template": "minecraft:zombie", "animation": "Zombie"}"#,
                "is not one of the game's sets",
            ),
            (
                "mob-sound-key",
                r#"{"template": "minecraft:zombie", "sounds": {"roar": "minecraft:entity.zombie.ambient"}}"#,
                "the keys are",
            ),
            (
                "mob-sound-id",
                r#"{"template": "minecraft:zombie", "sounds": {"hurt": "entity.zombie.hurt"}}"#,
                "namespaced sound event",
            ),
            (
                "mob-flag",
                r#"{"template": "minecraft:zombie", "flags": {"flies": true}}"#,
                "is not one of",
            ),
        ] {
            let files: &[(&str, &[u8])] = &[
                ("assets/ruby/textures/entity/parasite.png", png),
                ("data/ruby/horizon/entity_type/a.json", file.as_bytes()),
            ];
            let e = hmod(case, MANIFEST_TEXT, files).unwrap_err();
            assert!(e.contains(why), "{case}: {e}");
        }
    }

    #[test]
    fn a_mob_entity_type_names_a_model_animation_and_sounds_of_its_own() {
        let files: &[(&str, &[u8])] = &[
            ("assets/ruby/models/entity/crawler.json", br#"{"parts": {}}"#),
            (
                "data/ruby/horizon/entity_type/crawler.json",
                br#"{"template": "minecraft:zombie", "model": "ruby:entity/crawler", "animation": "spider",
                    "sounds": {"ambient": "minecraft:entity.spider.ambient", "step": "ruby:crawler.step"}}"#,
            ),
        ];
        let content = hmod("mob-look", MANIFEST_TEXT, files).unwrap();
        let crawler = &content.entity_types[0];
        assert_eq!(crawler.model.as_deref(), Some("ruby:entity/crawler"));
        assert_eq!(crawler.animation.as_deref(), Some("spider"));
        assert_eq!(crawler.sounds["step"], "ruby:crawler.step");
        assert_eq!(
            mob_model_file("ruby:entity/crawler"),
            Ok(("ruby", "assets/ruby/models/entity/crawler.json".to_owned()))
        );
        for bad in [
            "entity/crawler",
            "ruby:crawler",
            "ruby:entity/",
            "ruby:entity/a/b",
            "ruby:entity/Big",
            "ruby:entity/..",
        ] {
            assert!(mob_model_file(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn entity_types_copy_a_throwable_and_name_its_item() {
        let files: &[(&str, &[u8])] = &[
            ("data/ruby/horizon/item/gem.json", ITEM),
            (
                "data/ruby/horizon/entity_type/bolt.json",
                br#"{"template": "minecraft:snowball", "item": "ruby:gem", "damage": 2.5}"#,
            ),
            ("data/ruby/horizon/item/dust.json", ITEM),
            (
                "data/ruby/horizon/entity_type/puff.json",
                br#"{"template": "minecraft:snowball", "item": "ruby:dust"}"#,
            ),
        ];
        let content = hmod("types", MANIFEST_TEXT, files).unwrap();
        let types: Vec<_> = content
            .entity_types
            .iter()
            .map(|t| (t.id.as_str(), t.item.as_deref(), t.damage, t.is_mob()))
            .collect();
        assert_eq!(
            types,
            [
                ("ruby:bolt", Some("ruby:gem"), 2.5, false),
                ("ruby:puff", Some("ruby:dust"), 0.0, false)
            ]
        );
        let snowball = |rest: &str| format!(r#"{{"template": "minecraft:snowball"{rest}}}"#);
        for (case, file, why) in [
            (
                "type-template",
                r#"{"template": "minecraft:egg", "item": "ruby:gem"}"#.to_owned(),
                "is not one of",
            ),
            (
                "type-vanilla-item",
                snowball(r#", "item": "minecraft:stick""#),
                "is not one of this mod's items",
            ),
            (
                "type-unknown-item",
                snowball(r#", "item": "ruby:none""#),
                "is not one of this mod's items",
            ),
            (
                "type-damage",
                snowball(r#", "item": "ruby:gem", "damage": -1"#),
                "damage is 0 or more",
            ),
            (
                "type-key",
                snowball(r#", "item": "ruby:gem", "sizes": 1"#),
                "unknown field",
            ),
            (
                "type-mob-fields-on-a-throwable",
                snowball(r#", "item": "ruby:gem", "goals": "none""#),
                "are for a mob template",
            ),
            (
                "type-no-item",
                snowball(""),
                "item names one of this mod's items",
            ),
        ] {
            let files: &[(&str, &[u8])] = &[
                ("data/ruby/horizon/item/gem.json", ITEM),
                ("data/ruby/horizon/entity_type/a.json", file.as_bytes()),
            ];
            let e = hmod(case, MANIFEST_TEXT, files).unwrap_err();
            assert!(e.contains(why), "{case}: {e}");
        }
        let shared = snowball(r#", "item": "ruby:gem""#);
        let files: &[(&str, &[u8])] = &[
            ("data/ruby/horizon/item/gem.json", ITEM),
            ("data/ruby/horizon/entity_type/a.json", shared.as_bytes()),
            ("data/ruby/horizon/entity_type/b.json", shared.as_bytes()),
        ];
        let e = hmod("type-shared", MANIFEST_TEXT, files).unwrap_err();
        assert!(e.contains("already throws another entity type"), "{e}");
    }

    #[test]
    fn a_block_may_occlude_nothing() {
        let block = |occludes: &str| {
            let text = format!(
                r#"{{"template": "minecraft:barrel", "properties": {{"occludes": {occludes}}}}}"#
            );
            let files: &[(&str, &[u8])] =
                &[("data/ruby/horizon/block/panel.json", text.as_bytes())];
            hmod("occludes", MANIFEST_TEXT, files)
        };
        let occludes = |c: Result<Content, String>| c.unwrap().blocks[0].properties.occludes;
        assert_eq!(occludes(block("false")), Some(false));
        assert_eq!(occludes(block("true")), Some(true));
        assert!(block(r#""no""#).is_err());
    }

    #[test]
    fn blocks_choose_their_render_shape() {
        let block = |shape: &str| {
            let text = format!(
                r#"{{"template": "minecraft:stone", "properties": {{"render_shape": {shape}}}}}"#
            );
            let files: &[(&str, &[u8])] = &[("data/ruby/horizon/block/gear.json", text.as_bytes())];
            hmod("render-shape", MANIFEST_TEXT, files)
        };
        let shape = |c: Result<Content, String>| c.unwrap().blocks[0].properties.render_shape;
        assert_eq!(shape(block(r#""MODEL""#)), Some(RenderShape::Model));
        assert_eq!(
            shape(block(r#""ENTITYBLOCK_ANIMATED""#)),
            Some(RenderShape::EntityblockAnimated)
        );
        assert_eq!(shape(block(r#""INVISIBLE""#)), Some(RenderShape::Invisible));
        // Only vanilla's constants, as written there.
        for bad in [r#""animated""#, r#""model""#, "1"] {
            assert!(block(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn blocks_pick_the_states_that_tick_randomly() {
        let own = r#""state_properties": {"grown": {"type": "boolean"}}"#;
        let block = |ticks: &str| {
            let text = format!(
                r#"{{"template": "minecraft:dirt", {own}, "properties": {{"random_ticks": {ticks}}}}}"#
            );
            let files: &[(&str, &[u8])] = &[("data/ruby/horizon/block/crop.json", text.as_bytes())];
            hmod("random-ticks", MANIFEST_TEXT, files)
        };
        let ticks =
            |c: Result<Content, String>| c.unwrap().blocks[0].properties.random_ticks.clone();
        let state = |v: &str| [("grown".to_owned(), v.to_owned())];
        // Every state, whatever the template does; `false` stops a template that ticks.
        let all = ticks(block("true")).unwrap();
        assert!(all.applies(&state("true")) && all.applies(&[]));
        assert!(!ticks(block("false")).unwrap().applies(&state("false")));
        // Chosen states: a crop that stops ticking once it is grown.
        let chosen = ticks(block(r#"{"property": "grown", "values": ["false"]}"#)).unwrap();
        assert!(chosen.applies(&state("false")) && !chosen.applies(&state("true")));
        assert!(
            !chosen.applies(&[]),
            "a state without the property does not tick"
        );
        // Not the block's own property, no values, or a value it doesn't have.
        for bad in [
            r#"{"property": "age", "values": ["0"]}"#,
            r#"{"property": "grown", "values": []}"#,
            r#"{"property": "grown", "values": ["maybe"]}"#,
        ] {
            let e = block(bad).err().unwrap();
            assert!(e.contains("random_ticks"), "{bad}: {e}");
        }
        assert!(block("3").is_err());
    }

    #[test]
    fn block_items_patch_their_components() {
        let block = br#"{"template": "minecraft:iron_block", "components": {
            "minecraft:max_stack_size": 16, "!minecraft:lore": {}}}"#;
        let files: &[(&str, &[u8])] = &[("data/ruby/horizon/block/ruby_block.json", block)];
        let content = hmod("block-components", MANIFEST_TEXT, files).unwrap();
        let components = &content.blocks[0].components;
        assert_eq!(
            components.keys().collect::<Vec<_>>(),
            ["!minecraft:lore", "minecraft:max_stack_size"]
        );
        assert_eq!(components["minecraft:max_stack_size"], 16);
    }

    #[test]
    fn folder_mod_reads_the_same() {
        let dir = scratch("folder");
        std::fs::write(dir.join(MANIFEST), MANIFEST_TEXT).unwrap();
        let block = dir.join("data/ruby/horizon/block");
        std::fs::create_dir_all(&block).unwrap();
        std::fs::write(block.join("ruby_block.json"), BLOCK).unwrap();
        let package = Package::read(&dir).unwrap();
        assert!(package.wasm.is_none());
        let content = Content::read(&dir, &package.manifest).unwrap();
        assert_eq!(content.files, ["data/ruby/horizon/block/ruby_block.json"]);
        assert_eq!(content.file(&content.files[0]).unwrap(), BLOCK);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A connecting block names a block tag and copies a `PipeBlock`.
    #[test]
    fn connecting_blocks_copy_a_pipe_and_name_a_tag() {
        let block = |json: &str| {
            let files: &[(&str, &[u8])] =
                &[("data/ruby/horizon/block/cable.json", json.as_bytes())];
            hmod("cable", MANIFEST_TEXT, files)
        };
        let content = block(
            r##"{"template": "minecraft:chorus_plant", "connects_to": "#ruby:cable_joins"}"##,
        )
        .unwrap();
        assert_eq!(
            content.blocks[0].connects_to.as_deref(),
            Some("ruby:cable_joins")
        );
        let thin =
            r##"{"template": "minecraft:chorus_plant", "connects_to": "#ruby:j", "thickness": 4}"##;
        assert_eq!(block(thin).unwrap().blocks[0].thickness, Some(4.0));
        let e = block(r#"{"template": "minecraft:chorus_plant", "thickness": 4}"#).unwrap_err();
        assert!(e.contains("connects_to"), "{e}");
        let e = block(r##"{"template": "minecraft:chorus_plant", "connects_to": "#ruby:j", "thickness": 16}"##)
            .unwrap_err();
        assert!(e.contains("2 to 14"), "{e}");
        let e = block(r##"{"template": "minecraft:stone", "connects_to": "#ruby:cable_joins"}"##)
            .unwrap_err();
        assert!(e.contains("chorus_plant"), "{e}");
        for tag in ["ruby:cable_joins", "#cable_joins", "#Ruby:Cable"] {
            let json =
                format!(r#"{{"template": "minecraft:chorus_plant", "connects_to": "{tag}"}}"#);
            let e = block(&json).unwrap_err();
            assert!(e.contains("block tag"), "{tag}: {e}");
        }
    }

    /// A sign names its wood; a standing or ceiling sign names the wall form
    /// its item places. Neither means anything on another template.
    #[test]
    fn signs_name_their_wood_and_wall_form() {
        let block = |json: &str| {
            let files: &[(&str, &[u8])] =
                &[("data/ruby/horizon/block/notice.json", json.as_bytes())];
            hmod("sign", MANIFEST_TEXT, files)
        };
        let files: &[(&str, &[u8])] = &[
            (
                "data/ruby/horizon/block/notice.json",
                br#"{"template": "minecraft:oak_sign", "wood_type": "ruby:elm", "wall_block": "ruby:notice_wall"}"#,
            ),
            (
                "data/ruby/horizon/block/notice_wall.json",
                br#"{"template": "minecraft:oak_wall_sign", "item": false}"#,
            ),
        ];
        let content = hmod("sign-pair", MANIFEST_TEXT, files).unwrap();
        assert_eq!(content.blocks[0].wood_type.as_deref(), Some("ruby:elm"));
        assert_eq!(
            content.blocks[0].wall_block.as_deref(),
            Some("ruby:notice_wall")
        );
        // Vanilla's woods go by their bare names, as `WoodType.name` does.
        let cherry = r#"{"template": "minecraft:cherry_wall_hanging_sign", "wood_type": "cherry", "item": false}"#;
        assert_eq!(
            block(cherry).unwrap().blocks[0].wood_type.as_deref(),
            Some("cherry")
        );
        assert_eq!(
            block(r#"{"template": "minecraft:oak_sign", "wood_type": "ruby-wood:old/elm"}"#)
                .unwrap()
                .blocks[0]
                .wood_type
                .as_deref(),
            Some("ruby-wood:old/elm")
        );
        for bad in [
            r#"{"template": "minecraft:stone", "wood_type": "ruby:elm"}"#,
            r#"{"template": "minecraft:oak_sign", "wood_type": "Ruby:Elm"}"#,
            r#"{"template": "minecraft:oak_sign", "wood_type": "a:b:c"}"#,
            r#"{"template": "minecraft:oak_sign", "wood_type": "../elm"}"#,
        ] {
            assert!(block(bad).unwrap_err().contains("wood_type"), "{bad}");
        }
        for bad in [
            r#"{"template": "minecraft:stone", "wall_block": "ruby:notice_wall"}"#,
            r#"{"template": "minecraft:oak_wall_sign", "wall_block": "ruby:notice_wall", "item": false}"#,
            r#"{"template": "minecraft:oak_sign", "wall_block": "notice_wall"}"#,
            r#"{"template": "minecraft:oak_sign", "wall_block": ":wall"}"#,
            r#"{"template": "minecraft:oak_sign", "wall_block": "ruby:"}"#,
            r#"{"template": "minecraft:oak_sign", "wall_block": "Ruby:wall"}"#,
            r#"{"template": "minecraft:oak_sign", "wall_block": "ruby:wall:other"}"#,
            r#"{"template": "minecraft:oak_sign", "wall_block": "ruby:notice_wall", "item": false}"#,
        ] {
            assert!(block(bad).unwrap_err().contains("wall_block"), "{bad}");
        }
    }

    /// A sign's wall form is one of the mod's blocks, of the matching wall
    /// template, with no item and no other sign. Anything else refuses this
    /// mod when it is read, and leaves the other mods alone.
    #[test]
    fn a_signs_wall_block_is_its_own_mods_wall_form() {
        let refused = |template: &str, wall_block: &str, wall: &str, wanted: &str| {
            let sign =
                format!(r#"{{"template": "minecraft:{template}", "wall_block": "{wall_block}"}}"#);
            let second =
                format!(r#"{{"template": "minecraft:{template}", "wall_block": "ruby:wall"}}"#);
            let mut files: Vec<(&str, &[u8])> = vec![
                ("data/ruby/horizon/block/sign.json", sign.as_bytes()),
                ("data/ruby/horizon/block/wall.json", wall.as_bytes()),
            ];
            if wanted == "already paired" {
                files.push(("data/ruby/horizon/block/second.json", second.as_bytes()));
            }
            let e = hmod("sign-wall", MANIFEST_TEXT, &files).unwrap_err();
            assert!(e.contains(wanted), "{wanted}: {e}");
        };
        let wall = r#"{"template": "minecraft:oak_wall_sign", "item": false}"#;
        for (template, wall_template) in [
            ("oak_sign", "stone"),
            ("oak_sign", "oak_wall_hanging_sign"),
            ("oak_hanging_sign", "oak_wall_sign"),
        ] {
            let other = format!(r#"{{"template": "minecraft:{wall_template}", "item": false}}"#);
            refused(template, "ruby:wall", &other, "needs a template");
        }
        refused(
            "oak_sign",
            "ruby:wall",
            r#"{"template": "minecraft:oak_wall_sign"}"#,
            "\"item\" is false",
        );
        refused("oak_sign", "ruby:wall", wall, "already paired");
        for absent in ["ruby:elsewhere", "other:wall", "minecraft:oak_wall_sign"] {
            refused("oak_sign", absent, wall, "not one of this mod's blocks");
        }

        // Declared in code, the same; nothing is added.
        let block = |id: &str, template: &str, item, wall_block: Option<&str>| Block {
            id: id.into(),
            template: template.into(),
            item,
            wall_block: wall_block.map(str::to_owned),
            ..Block::default()
        };
        let mut content = Content::declared("ruby");
        let sign = block("ruby:sign", "minecraft:oak_sign", true, Some("ruby:wall"));
        assert!(content.declare(std::slice::from_ref(&sign), &[]).is_err());
        assert!(content.blocks.is_empty());
        let wall = block("ruby:wall", "minecraft:oak_wall_sign", false, None);
        content.declare(&[sign, wall], &[]).unwrap();
        assert_eq!(content.blocks.len(), 2);
    }

    /// A creative tab: a title, an icon and its items, all item ids.
    #[test]
    fn creative_tabs_name_their_items() {
        let tab = |json: &str| {
            let files: &[(&str, &[u8])] =
                &[("data/ruby/horizon/creative_tab/gems.json", json.as_bytes())];
            hmod("tab", MANIFEST_TEXT, files)
        };
        let content =
            tab(r#"{"title": "Ruby Gems", "icon": "ruby:ruby", "items": ["ruby:ruby", "minecraft:diamond"]}"#)
                .unwrap();
        assert_eq!(
            content.creative_tabs,
            [CreativeTab {
                id: "ruby:gems".into(),
                title: "Ruby Gems".into(),
                icon: "ruby:ruby".into(),
                items: vec!["ruby:ruby".into(), "minecraft:diamond".into()],
            }]
        );
        let e = tab(r#"{"title": "Gems", "icon": "ruby", "items": []}"#).unwrap_err();
        assert!(e.contains("item ids"), "{e}");
    }

    #[test]
    fn refuses_content_outside_its_fence() {
        let refused = |name: &str, files: &[(&str, &[u8])]| {
            let e = hmod(name, MANIFEST_TEXT, files).unwrap_err();
            assert!(!e.is_empty());
            e
        };
        let e = refused("other-ns", &[("data/other/horizon/block/a.json", BLOCK)]);
        assert!(e.contains("own namespaces"), "{e}");
        refused(
            "vanilla-asset",
            &[("assets/minecraft/textures/block/stone.png", b"x")],
        );
        refused(
            "vanilla-recipe",
            &[("data/minecraft/recipe/stone.json", b"{}")],
        );
        refused("convention-recipe", &[("data/c/recipe/stone.json", b"{}")]);
        refused("convention-asset", &[("assets/c/lang/en_us.json", b"{}")]);
        let e = refused(
            "modded-template",
            &[(
                "data/ruby/horizon/block/a.json",
                br#"{"template": "ruby:other"}"#,
            )],
        );
        assert!(e.contains("not a vanilla block"), "{e}");
        refused(
            "light",
            &[(
                "data/ruby/horizon/block/a.json",
                br#"{"template": "minecraft:stone", "properties": {"light": 16}}"#,
            )],
        );
        refused(
            "unknown-key",
            &[(
                "data/ruby/horizon/block/a.json",
                br#"{"template": "minecraft:stone", "glow": true}"#,
            )],
        );
        refused("shape-reversed", &[("data/ruby/horizon/block/a.json", br#"{"template":"minecraft:stone","shapes":{"property":"axis","values":{"x":[[1,0,0,0,1,1]]}}}"#)]);
        refused("shape-malformed", &[("data/ruby/horizon/block/a.json", br#"{"template":"minecraft:stone","shapes":{"property":"axis","values":{"x":[[0,0,0,1,1]]}}}"#)]);
        refused(
            "shape-no-values",
            &[(
                "data/ruby/horizon/block/a.json",
                br#"{"template":"minecraft:stone","shapes":{"property":"axis","values":{}}}"#,
            )],
        );
        let e = refused(
            "clash",
            &[
                ("data/ruby/horizon/block/a.json", BLOCK),
                ("data/ruby/horizon/item/a.json", ITEM),
            ],
        );
        assert!(e.contains("also a block's item"), "{e}");
        refused("other-kind", &[("data/ruby/horizon/entity/a.json", b"{}")]);
        refused(
            "stack",
            &[(
                "data/ruby/horizon/item/a.json",
                br#"{"template": "minecraft:stick", "max_stack_size": 100}"#,
            )],
        );
        for (case, components, why) in [
            (
                "bare-component",
                r#"{"max_damage": 3}"#,
                "must be namespaced",
            ),
            (
                "uppercase-path",
                r#"{"minecraft:Foo": 3}"#,
                "must be namespaced and valid",
            ),
            (
                "uppercase-namespace",
                r#"{"Example:charge": 3}"#,
                "must be namespaced and valid",
            ),
            (
                "empty-namespace",
                r#"{":charge": 3}"#,
                "must be namespaced and valid",
            ),
            (
                "empty-path",
                r#"{"example:": 3}"#,
                "must be namespaced and valid",
            ),
            (
                "invalid-removal",
                r#"{"!example:Foo": {}}"#,
                "must be namespaced and valid",
            ),
            (
                "own-key",
                r#"{"minecraft:rarity": "epic"}"#,
                "set rarity instead",
            ),
            (
                "own-removal",
                r#"{"!minecraft:max_stack_size": {}}"#,
                "set max_stack_size",
            ),
            (
                "removal-value",
                r#"{"!minecraft:food": true}"#,
                "a removal's value is {}",
            ),
        ] {
            let item = format!(r#"{{"template": "minecraft:stick", "components": {components}}}"#);
            let e = refused(case, &[("data/ruby/horizon/item/a.json", item.as_bytes())]);
            assert!(e.contains(why), "{case}: {e}");
        }
        let glass = br#"{"template": "minecraft:glass", "item": false}"#;
        for (case, rest) in [
            ("bare-remainder", "bowl"),
            ("missing-remainder", "ruby:bowl"),
            ("other-mod-remainder", "jade:bowl"),
            ("itemless-remainder", "ruby:glass"),
            ("air-remainder", "minecraft:air"),
        ] {
            let item =
                format!(r#"{{"template": "minecraft:stick", "crafting_remainder": "{rest}"}}"#);
            let files: &[(&str, &[u8])] = &[
                ("data/ruby/horizon/item/a.json", item.as_bytes()),
                ("data/ruby/horizon/block/glass.json", glass),
            ];
            let e = refused(case, files);
            assert!(e.contains("crafting_remainder"), "{case}: {e}");
        }
        for (case, block, why) in [
            (
                "block-removal-value",
                r#"{"template": "minecraft:stone", "components": {"!minecraft:lore": []}}"#,
                "a removal's value is {}",
            ),
            (
                "block-stack-zero",
                r#"{"template": "minecraft:stone", "components": {"minecraft:max_stack_size": 0}}"#,
                "integer 1 to 99",
            ),
            (
                "block-stack-100",
                r#"{"template": "minecraft:stone", "components": {"minecraft:max_stack_size": 100}}"#,
                "integer 1 to 99",
            ),
            (
                "block-stack-string",
                r#"{"template": "minecraft:stone", "components": {"minecraft:max_stack_size": "16"}}"#,
                "integer 1 to 99",
            ),
            (
                "block-rarity",
                r#"{"template": "minecraft:stone", "components": {"minecraft:rarity": "bogus"}}"#,
                "common, uncommon",
            ),
            (
                "block-without-item",
                r#"{"template": "minecraft:glass", "item": false,
                    "components": {"minecraft:max_stack_size": 16}}"#,
                "item is false",
            ),
        ] {
            let e = refused(
                case,
                &[("data/ruby/horizon/block/a.json", block.as_bytes())],
            );
            assert!(e.contains(why), "{case}: {e}");
        }
    }

    #[test]
    fn listed_namespaces_hold_content() {
        let manifest = format!("{MANIFEST_TEXT}namespaces = [\"kaisyn\"]\n");
        let files: &[(&str, &[u8])] = &[
            ("data/kaisyn/worldgen/template_pool/a.json", b"{}"),
            ("data/kaisyn/structure/a.nbt", b"nbt"),
            ("assets/kaisyn/lang/en_us.json", b"{}"),
        ];
        let content = hmod("listed", &manifest, files).unwrap();
        assert_eq!(content.files.len(), 3);
        assert_eq!(content.namespaces, ["ruby", "kaisyn"]);
        // Declarations stay in the id's namespace.
        let decl: &[(&str, &[u8])] = &[("data/kaisyn/horizon/block/a.json", BLOCK)];
        let e = hmod("listed-decl", &manifest, decl).unwrap_err();
        assert!(e.contains("declared in data/ruby/horizon/"), "{e}");
        // A namespace it does not list is still outside.
        let other: &[(&str, &[u8])] = &[("data/other/structure/a.nbt", b"nbt")];
        assert!(hmod("listed-other", &manifest, other).is_err());
    }

    #[test]
    fn mods_sharing_a_namespace_clash() {
        let t_and_t = ["t_and_t".to_owned(), "kaisyn".to_owned()];
        let kaisyn = ["kaisyn".to_owned()];
        let ruby = ["ruby".to_owned(), "gems".to_owned()];
        assert_eq!(
            clash(("t_and_t", &t_and_t), ("kaisyn", &kaisyn)).as_deref(),
            Some("mods t_and_t and kaisyn both use the namespace kaisyn; turn one of them off")
        );
        assert_eq!(clash(("t_and_t", &t_and_t), ("ruby", &ruby)), None);
    }

    #[test]
    fn content_needs_the_registries_capability() {
        let code_only = MANIFEST_TEXT
            .replace("api = \"0.2\"", "api = \"0.1\"")
            .replace("capabilities = [\"registries\"]", "capabilities = []");
        // No code and no registries: nothing to load.
        let e = hmod("empty", &code_only, &[]).unwrap_err();
        assert!(e.contains("mod.wasm: missing"), "{e}");
        // Registries on API 0.1.
        let old = MANIFEST_TEXT.replace("api = \"0.2\"", "api = \"0.1\"");
        assert!(Manifest::parse(&old).unwrap_err().contains("needs api"));
        // Content without the capability, in a mod with code.
        let dir = scratch("no-cap");
        let path = dir.join("m.hmod");
        let files: &[(&str, &[u8])] = &[("assets/ruby/lang/en_us.json", b"{}")];
        std::fs::write(
            &path,
            Package::write_files(&code_only, Some(b"\0asm"), files).unwrap(),
        )
        .unwrap();
        let package = Package::read(&path).unwrap();
        let e = Content::read(&path, &package.manifest).err().unwrap();
        assert!(e.contains("not capability registries"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paths_are_plain() {
        assert!(check_path("assets/ruby/textures/block/ruby_block.png").is_ok());
        assert!(check_path("data/ruby/loot_table/blocks/a.json").is_ok());
        assert!(check_path("scripts/init.lua").is_ok());
        assert!(check_path("scripts/ai/stalk.lua").is_ok());
        for bad in [
            "assets/ruby",
            "assets/../x/y",
            "assets/ruby//a",
            "assets/Ruby/a",
            "assets/ruby/a b",
            "/assets/ruby/a",
            "other/ruby/a",
            "assets\\ruby\\a",
            "scripts",
            "scripts/init.txt",
            "scripts/../init.lua",
            "scripts/Init.lua",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }

    const SCRIPTED: &str = r#"
id = "ruby"
name = "Ruby"
version = "0.1.0"
api = "0.2"
side = "both"
capabilities = ["registries", "scripts"]
experimental = ["scripts"]
"#;

    #[test]
    fn scripts_travel_in_the_hmod_with_the_capability() {
        let lang: &[(&str, &[u8])] = &[
            ("assets/ruby/lang/en_us.json", b"{}"),
            ("scripts/init.lua", b"require('grow')"),
            ("scripts/grow.lua", b"return 1"),
        ];
        let content = hmod("scripted", SCRIPTED, lang).unwrap();
        assert_eq!(content.files, ["assets/ruby/lang/en_us.json"]);
        assert_eq!(content.scripts, ["scripts/grow.lua", "scripts/init.lua"]);

        // Scripts without the capability, and the capability without its entry script.
        let e = hmod("scripts-no-cap", MANIFEST_TEXT, &lang[1..]).unwrap_err();
        assert!(e.contains("not capability scripts"), "{e}");
        let e = hmod("scripts-no-init", SCRIPTED, &lang[..1]).unwrap_err();
        assert!(e.contains("no scripts/init.lua"), "{e}");

        // Scripts alone are a mod: no code, no registries.
        let alone = SCRIPTED.replace("[\"registries\", \"scripts\"]", "[\"scripts\"]");
        let content = hmod("scripts-only", &alone, &lang[1..]).unwrap();
        assert!(content.files.is_empty());
        assert_eq!(content.scripts.len(), 2);
    }
}
