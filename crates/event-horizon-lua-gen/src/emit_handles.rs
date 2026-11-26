//! The handle methods the Lua host binds (issue 74, H1b): for each handle family, its members that
//! only read the world, with their arguments and results converted.
//!
//! [`emit`] takes the families as a parameter ([`candidate_families`] is the default: every `*Ref`
//! owner of the surface). A family none of whose members binds is not generated ([`emit_listing`]
//! lists it). A member is bound when
//! its receiver is `self` or `&self`, its first parameter is `&dyn Host` or there is none, and every
//! other parameter and the return type convert. A `BlockState` argument or result needs the host
//! (it converts through it). Anything else is skipped with a reason, and the coverage text lists
//! each member. A write (`&mut dyn ServerHost`, H2) binds through the turn's server host and is
//! refused by the powerful table ([`powerful`]) when it is powerful; a `&mut dyn ClientHost` write
//! is never bound.
//!
//! The text is one `impl` of the hand-written `Family` trait per family, one function per bound
//! member, and the table `cost`/`run` that the host's scoped function calls.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::emit_impl::scalar_name;
use crate::emit_types::{Plain, snake_case};
use crate::handles::{MemberIr, MemberKind, ParamIr, Receiver};
use crate::types::{Access, FloatKind, TypeIr, map_type};

/// The families the generator starts from when it is not told others: every owner of the surface
/// whose name ends in `Ref` (a handle family, by the mod API's naming), sorted by name. A family
/// that binds no member is then dropped, see the generator's `main`.
pub fn candidate_families(members: &[MemberIr]) -> Vec<String> {
    let mut names: Vec<String> = members
        .iter()
        .filter(|m| !m.from_trait && m.owner.ends_with("Ref"))
        .map(|m| m.owner.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The cost of a read, in budget units.
const READ: u32 = 1;
/// The cost of a heavy read: one that returns a list, or one of the names below.
const HEAVY: u32 = 100;
/// Methods heavy by name, beside every method that returns a list.
const HEAVY_NAMES: &[&str] = &[
    "all_entities",
    "clip",
    "collision_boxes",
    "entities_in",
    "loaded_chunks",
    "online_players",
];
/// Method name prefixes that are heavy.
const HEAVY_PREFIXES: &[&str] = &["datapack_", "find_", "ray_trace"];
/// The cost of a write (a method with `&mut dyn ServerHost`), in budget units.
const WRITE: u32 = 10;

/// The powerful writes, which are never bound here (design rule 7: they wait for the manifest's
/// `powers` flag). This is the one table of them. A write is powerful when any rule matches:
///
/// * its name contains one of [`POWERFUL_NAME_PARTS`] (commands, teleports, NBT, spawns,
///   explosions, lightning, game rules, world borders, passages, map data, capabilities,
///   datapacks, registration, removal, kills, saves, tickets, tick clocks, weather, respawns,
///   random sources, player nicknames, item grants through `give`, `pop_resource` and
///   `award_experience`);
/// * its owner is one of [`POWERFUL_OWNERS`] (every `Registrar`, whatever its name);
/// * its owner and name part match one of [`POWERFUL_OWNER_NAMES`] (a merchant's offers, which
///   carry items);
/// * one of its parameters mentions one of [`POWERFUL_ARG_TYPES`] (NBT, a transaction, an item
///   stack, a permission level).
///
/// The rules read writes only: a read's name is not judged here (a read that merely contains a
/// name part, such as `raiders`, stays bound). When in doubt, add a rule: it is cheap to allow
/// later and expensive to take back.
const POWERFUL_NAME_PARTS: &[&str] = &[
    "run_command",
    "teleport_to_level",
    "nbt",
    "spawn",
    "explo",
    "lightning",
    "game_rule",
    "world_border",
    "passage",
    "map_data",
    "capabilit",
    "datapack",
    "register",
    "remove",
    "discard",
    "kill",
    "command",
    "nickname",
    "ticket",
    "save",
    "relight",
    "raid",
    "give",
    "pop_resource",
    "award_experience",
    "weather",
    "clock",
    "difficulty",
    "respawn",
    "random",
    "disconnect",
    "set_game_mode",
    "award_stat",
    "reset_stat",
];
/// Owners whose every member is powerful.
const POWERFUL_OWNERS: &[&str] = &["Registrar"];
/// Owner and name part: a method of the owner whose name contains the part is powerful.
const POWERFUL_OWNER_NAMES: &[(&str, &str)] = &[("MerchantRef", "offer")];
/// Parameter types that make a write powerful: NBT, a transaction, an item stack, a permission level.
const POWERFUL_ARG_TYPES: &[&str] = &[
    "Compound",
    "Transaction",
    "ItemStack",
    "PermissionLevel",
    "MerchantOffer",
];
/// The reason a powerful write is skipped: the category `powerful (needs manifest power)`, then
/// the rule that matched. [`reason_category`] keeps the category.
fn powerful(member: &MemberIr) -> Option<String> {
    let name = member.name.as_str();
    let owner = member.owner.as_str();
    if READS_WITHOUT_POWER.contains(&format!("{owner}::{name}").as_str()) {
        return None;
    }
    let rule = if POWERFUL_OWNERS
        .iter()
        .any(|powerful| owner.contains(powerful))
    {
        format!("owner {owner}")
    } else if !is_server_write(member) {
        return None;
    } else if let Some(part) = POWERFUL_NAME_PARTS
        .iter()
        .find(|part| name.contains(**part))
    {
        format!("name contains `{part}`")
    } else if let Some((_, part)) = POWERFUL_OWNER_NAMES
        .iter()
        .find(|(powerful, part)| *powerful == owner && name.contains(part))
    {
        format!("{owner} method containing `{part}`")
    } else {
        let param = member.params.iter().find(|param| {
            POWERFUL_ARG_TYPES
                .iter()
                .any(|ty| param.rust_type.contains(ty))
        })?;
        format!("argument `{}`", param.rust_type)
    };
    Some(format!("powerful (needs manifest power): {rule}"))
}

/// Whether a member takes `&mut dyn ServerHost` as its first parameter: an ordinary write.
fn is_server_write(member: &MemberIr) -> bool {
    member.params.first().is_some_and(|first| {
        first
            .rust_type
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            == "&mut dyn ServerHost"
    })
}
/// The first wave of gated writes (issue 74, P2): `(Family::name, power)`. Each is a server write that
/// [`powerful`] would deny, bound behind its manifest power (`powers.rs` checks it at the call). Exactly
/// these: [`check_gated`] fails the generator when a listed write does not bind, and when a write a rule
/// denies binds without being listed.
pub const GATED_WRITES: &[(&str, &str)] = &[
    ("BossBarRef::remove", "world"),
    ("BossBarRef::remove_player", "world"),
    ("EntityRef::discard", "world"),
    ("EntityRef::remove_tag", "world"),
    ("EntityRef::teleport_to_level", "world"),
    ("LevelRef::block_random_tick", "world"),
    ("LevelRef::invalidate_capabilities", "world"),
    ("LevelRef::remove_block", "world"),
    ("LevelRef::request_save", "world"),
    ("LevelRef::set_clock_time", "world"),
    ("LevelRef::set_weather", "world"),
    ("LevelRef::set_weather_levels", "world"),
    ("LevelRef::set_world_border_center", "world"),
    ("LevelRef::set_world_border_damage", "world"),
    ("LevelRef::set_world_border_size", "world"),
    ("LevelRef::set_world_border_warning", "world"),
    ("LevelRef::strike_lightning", "world"),
    ("LivingRef::random_teleport", "world"),
    ("MobRef::remove_goal", "world"),
    ("MobRef::remove_target_goal", "world"),
    ("LevelRef::award_experience", "spawn"),
    ("LevelRef::set_respawn_data", "spawn"),
    ("PlayerRef::respawn", "spawn"),
    ("LevelRef::spawn_mob", "spawn"),
];

/// The explosion reads that carry a power's name part but need no power: a `&mut dyn ServerHost`
/// signature the rule would otherwise deny, `(Family::name)`. They bind as ordinary reads.
const READS_WITHOUT_POWER: &[&str] = &[
    "LevelRef::explosion_resistance",
    "LevelRef::explosion_resistances",
];

/// The manifest names a gated write may need: the names `Power::from_name` gives, without the
/// reserved `network` and `files`, each with the `Power` variant the generated table names.
const GATEABLE_POWERS: &[(&str, &str)] = &[
    ("commands", "Commands"),
    ("nbt", "Nbt"),
    ("spawn", "Spawn"),
    ("registration", "Registration"),
    ("explosions", "Explosions"),
    ("world", "World"),
    ("game_rules", "GameRules"),
    ("transactions", "Transactions"),
];

/// The power a gated write of `allowlist` needs, or `None` when `op` (`Family::name`) is not listed.
fn power_in<'a>(allowlist: &[(&str, &'a str)], op: &str) -> Option<&'a str> {
    allowlist
        .iter()
        .find(|(name, _)| *name == op)
        .map(|(_, power)| *power)
}

/// The arrays a Lua value converts into (`vector.rs`), by element type and length.
const FROM_LUA_ARRAYS: &[(&str, usize)] = &[
    ("i32", 2),
    ("i32", 3),
    ("f64", 2),
    ("f64", 3),
    ("f32", 2),
    ("f32", 3),
];
/// The arrays a Rust value converts to Lua (`vector.rs` and `generated_types.rs`).
const TO_LUA_ARRAYS: &[(&str, usize)] = &[
    ("bool", 3),
    ("bool", 6),
    ("f32", 2),
    ("f32", 3),
    ("f32", 4),
    ("f32", 6),
    ("f32", 12),
    ("f32", 16),
    ("f64", 2),
    ("f64", 3),
    ("f64", 4),
    ("f64", 6),
    ("i32", 2),
    ("i32", 3),
    ("i32", 4),
    ("u8", 2),
    ("u8", 3),
    ("u16", 2),
    ("u32", 2),
    ("u64", 2),
    ("usize", 2),
];
/// Named types that convert both ways by hand in `event-horizon-lua-convert`.
const HAND_NAMED: &[&str] = &[
    "BlockPos",
    "Compound",
    "Duration",
    "Identifier",
    "Rotation",
    "Text",
    "text::Text",
];

/// How one argument reaches the game's method.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Arg {
    /// A value read with `Args::get`, passed as it is or by reference (`&str`, `&Identifier`).
    Get { ty: String, by_ref: bool },
    /// A handle of the family, read with `Args::handle`.
    Handle { ty: String },
    /// An optional handle of the family, read with `Args::opt_handle`.
    OptHandle { ty: String },
    /// A block state, read with `Args::state` through the host.
    State,
    /// An optional string, `Option<&str>`: read as `Option<String>`, passed as `Option<&str>`.
    OptStr,
}

/// How the result becomes a Lua value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ret {
    /// A value with a `ToLuaValue` impl.
    Value,
    /// A string (`String`, `&str`).
    Text,
    /// An optional string.
    OptText,
    /// A handle of the family, as its userdata.
    Family,
    OptFamily,
    ListFamily,
    OptListFamily,
    /// An optional block state, as its `{id, props}` table through the host.
    State,
    /// A world epoch, as its number.
    Epoch,
    /// Nothing: `()`, or no `->`. The Lua call returns nil.
    Unit,
}

/// A member the generator binds.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bound {
    id: String,
    op: String,
    name: String,
    /// The member takes a host: the read host, or the server host when `write`.
    host: bool,
    /// The member takes `&mut dyn ServerHost`: a write, which needs the turn's server host.
    write: bool,
    args: Vec<Arg>,
    ret: Ret,
    /// The member returns `Result<T, E>`: an `Err` raises, and `ret` converts the `Ok` value.
    refusal: bool,
    /// The result is an integer: an id accessor with one of these shows in `tostring`.
    int_ret: bool,
    cost: u32,
    /// The power a gated write needs ([`GATED_WRITES`]); `None` for every other member.
    gated: Option<&'static str>,
}

/// The generator's output for the families asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The file `handles.rs`.
    pub text: String,
    /// The coverage listing, `handles_coverage.txt`.
    pub coverage: String,
    /// The file `powers.rs`: the gated writes' rows, from the same bound members as `handles.rs`.
    pub powers: String,
    /// The gated writes that bound, `(Family::name, power)`, in op order.
    pub gated: Vec<(String, String)>,
    /// The bound members' ids, in family then name order.
    pub bound: Vec<String>,
    /// The heavy members' ids (a subset of `bound`).
    pub heavy: Vec<String>,
    /// Per family: `(members, bound)`.
    pub per_family: Vec<(String, usize, usize)>,
    /// Each member that is not bound, with its reason, in family then name order.
    pub skipped: Vec<(String, String)>,
}

/// The category of a skip reason: the text before its first colon.
pub fn reason_category(reason: &str) -> &str {
    reason.split(':').next().unwrap_or(reason).trim()
}

/// The names of the types the families' members (reads and server writes) take as owned arguments,
/// which the type emitter must decide (a unit enum among them converts from Lua). A family's own
/// handles are left out. This is the superset: [`bound_argument_types`] narrows it to the members
/// that bind.
pub fn argument_types(members: &[MemberIr], families: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for member in members
        .iter()
        .filter(|m| is_family_member(m, families) && reads_only(m))
    {
        for name in named_arguments(member, families) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The types of the arguments of the members (reads and writes) that [`classify`] binds under `plain`: the
/// converters the type emitter must decide, and no more. A bound member's enum argument is always
/// among them, so deciding exactly these leaves every bound member bound.
pub fn bound_argument_types(members: &[MemberIr], families: &[&str], plain: &Plain) -> Vec<String> {
    let mut names = Vec::new();
    for member in members
        .iter()
        .filter(|m| is_family_member(m, families) && reads_only(m))
    {
        let family = member.owner.as_str();
        if classify(member, family, families, plain).is_err() {
            continue;
        }
        for name in named_arguments(member, families) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The named types among a member's owned arguments (`T` or `Option<T>`), other than the families'.
fn named_arguments(member: &MemberIr, families: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for param in &member.params {
        let Ok(mapped) = map_type(&param.rust_type) else {
            continue;
        };
        let inner = match &mapped.ty {
            TypeIr::Option(inner) => inner.as_ref(),
            other => other,
        };
        if let (Access::Owned, TypeIr::Named(name)) = (mapped.access, inner)
            && !families.contains(&name.as_str())
        {
            names.push(name.clone());
        }
    }
    names
}

/// Whether a member takes no host, the read host (`&dyn Host`) or the server host
/// (`&mut dyn ServerHost`) as its first parameter: the members this generator can bind.
fn reads_only(member: &MemberIr) -> bool {
    member.params.first().is_none_or(|first| {
        let text = first
            .rust_type
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        text == "&dyn Host" || text == "&mut dyn ServerHost"
    })
}

fn is_family_member(member: &MemberIr, families: &[&str]) -> bool {
    !member.from_trait && families.contains(&member.owner.as_str())
}

/// Emits the handle file and its coverage for the members of `families`, in the order given.
/// `plain` is the set of plain surface types the type emitter decided; `hash` is the surface's.
pub fn emit(members: &[MemberIr], families: &[&str], plain: &Plain, hash: &str) -> Emitted {
    emit_listing(members, families, &[], plain, hash)
}

/// [`emit`], and the `dropped` families (asked for, but binding no member) listed in the coverage
/// with their members' reasons. Each dropped family is classified as if it were one more family
/// of `families`: a member that would bind with it would say so, and none does (the generator
/// drops a family only when none of its members binds).
pub fn emit_listing(
    members: &[MemberIr],
    families: &[&str],
    dropped: &[&str],
    plain: &Plain,
    hash: &str,
) -> Emitted {
    let mut text = format!(
        "// @generated by event-horizon-lua-gen from surface {hash}: do not edit.\n\
         // Families: {}. Coverage: handles_coverage.txt.\n\
         // Writes (`&mut dyn ServerHost`) run through the turn's server host and are NOT rolled back\n\
         // when the turn later errors: the host keeps them (design rule 5). Only outputs are void.\n",
        families.join(", ")
    );
    let mut coverage = format!(
        "# Handle coverage (issue 74, H2) from surface {hash}, by event-horizon-lua-gen: do not edit.\n\
         # families: {}\n# not generated (no member binds): {}\n",
        families.join(", "),
        if dropped.is_empty() {
            "none".to_owned()
        } else {
            dropped.join(", ")
        }
    );
    let mut bound_all: Vec<Bound> = Vec::new();
    let mut skipped = Vec::new();
    let mut per_family = Vec::new();
    let mut impls = String::new();
    let mut functions = String::new();

    for family in families {
        let family_members = family_members_of(members, family);
        let mut bound_here = Vec::new();
        let _ = writeln!(coverage, "\n{family}: {} members", family_members.len());
        for member in &family_members {
            match classify(member, family, families, plain) {
                Ok(bound) => {
                    match bound.gated {
                        Some(power) => {
                            let _ = writeln!(
                                coverage,
                                "  {} bound, cost {}, gated: {power}",
                                bound.id, bound.cost
                            );
                        }
                        None => {
                            let _ = writeln!(coverage, "  {} bound, cost {}", bound.id, bound.cost);
                        }
                    }
                    bound_here.push(bound);
                }
                Err(reason) => {
                    let _ = writeln!(coverage, "  {} skipped: {reason}", member.id);
                    skipped.push((member.id.clone(), reason));
                }
            }
        }
        per_family.push((family.to_string(), family_members.len(), bound_here.len()));
        impls.push_str(&family_impl(family, &bound_here));
        for bound in &bound_here {
            functions.push_str(&operation(family, bound));
        }
        bound_all.extend(bound_here);
    }
    for family in dropped {
        let with: Vec<&str> = families.iter().copied().chain([*family]).collect();
        let family_members = family_members_of(members, family);
        let _ = writeln!(
            coverage,
            "\n{family}: {} members, not generated: no member binds",
            family_members.len()
        );
        for member in &family_members {
            let reason = match classify(member, family, &with, plain) {
                Ok(_) => format!("bound under its own family, not generated: {}", member.id),
                Err(reason) => reason,
            };
            let _ = writeln!(coverage, "  {} skipped: {reason}", member.id);
            skipped.push((member.id.clone(), reason));
        }
        per_family.push((family.to_string(), family_members.len(), 0));
    }

    text.push_str(&format!(
        "\n// The userdata methods of each family: one call of the scoped function per method.\n{impls}"
    ));
    text.push_str(&functions);
    text.push_str(&table(&bound_all));

    let bound: Vec<String> = bound_all.iter().map(|b| b.id.clone()).collect();
    let heavy: Vec<String> = bound_all
        .iter()
        .filter(|b| b.cost == HEAVY)
        .map(|b| b.id.clone())
        .collect();
    let _ = writeln!(
        coverage,
        "\nsummary: bound {} of {}; heavy {}",
        bound.len(),
        per_family.iter().map(|(_, n, _)| n).sum::<usize>(),
        heavy.len()
    );
    let mut by_category: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, reason) in &skipped {
        *by_category.entry(reason_category(reason)).or_default() += 1;
    }
    for (category, count) in &by_category {
        let _ = writeln!(coverage, "  skipped, {category}: {count}");
    }
    let mut gated_by_power: BTreeMap<&str, usize> = BTreeMap::new();
    let mut gated: Vec<(String, String)> = Vec::new();
    for b in bound_all.iter().filter(|b| b.gated.is_some()) {
        let power = b.gated.unwrap_or_default();
        *gated_by_power.entry(power).or_default() += 1;
        gated.push((b.op.clone(), power.to_owned()));
    }
    gated.sort();
    for (power, count) in &gated_by_power {
        let _ = writeln!(coverage, "  gated: {power} {count}");
    }
    let _ = writeln!(coverage, "heavy (cost {HEAVY}): {}", heavy.join(", "));
    Emitted {
        text,
        coverage,
        powers: powers_text(&gated, hash),
        gated,
        bound,
        heavy,
        per_family,
        skipped,
    }
}

/// The file `powers.rs`: each gated write's op and the power it needs, sorted by op. The op is the
/// string the generated method passes to `host_call`, so the gate and the call name the same op.
fn powers_text(gated: &[(String, String)], hash: &str) -> String {
    let mut out = format!(
        "// @generated by event-horizon-lua-gen from surface {hash}: do not edit.\n\
         // The gated writes (issue 74, P2): each op and the power a mod must hold to call it. Written from\n\
         // the same bound members as handles.rs (emit_handles.rs GATED_WRITES).\n\n\
         /// The gated host operations: `(\"Family::method\", power)`, sorted by op.\n\
         pub const GATED: &[(&str, crate::manifest::Power)] = &[\n"
    );
    for (op, power) in gated {
        let variant = GATEABLE_POWERS
            .iter()
            .find(|(name, _)| name == power)
            .map_or("", |(_, variant)| *variant);
        let _ = writeln!(out, "    (\"{op}\", crate::manifest::Power::{variant}),");
    }
    out.push_str("];\n");
    out
}

/// The hard checks of a gated allowlist against an emitted surface (the generator's `main` runs
/// them on every pass; a failure stops the generator and names the write). Each listed write whose
/// family is generated must be a member, a `&mut dyn ServerHost` write, name a gateable power, and
/// bind. And the bound writes a rule denies must be exactly the listed ones: nothing denied binds
/// without its entry, and no entry goes unbound.
pub fn check_gated(
    allowlist: &[(&str, &str)],
    members: &[MemberIr],
    families: &[&str],
    plain: &Plain,
    emitted: &Emitted,
) -> Result<(), String> {
    for (op, power) in allowlist {
        let Some((family, name)) = op.split_once("::") else {
            return Err(format!("gated write `{op}` is not `Family::name`"));
        };
        if !families.contains(&family) {
            continue;
        }
        let Some(member) = members
            .iter()
            .find(|m| !m.from_trait && m.owner == family && m.name == name)
        else {
            return Err(format!("gated write `{op}` is not a member of the surface"));
        };
        if !is_server_write(member) {
            return Err(format!(
                "gated write `{op}` is not a `&mut dyn ServerHost` write"
            ));
        }
        if !GATEABLE_POWERS.iter().any(|(name, _)| name == power) {
            return Err(format!(
                "gated write `{op}` names power `{power}`, which is not a gateable power"
            ));
        }
        if let Err(reason) = classify(member, family, families, plain) {
            return Err(format!(
                "gated write `{op}` cannot be bound, so it is not generated: {reason}"
            ));
        }
    }
    for member in members {
        let family = member.owner.as_str();
        if member.from_trait || !families.contains(&family) {
            continue;
        }
        let op = format!("{family}::{}", member.name);
        if powerful(member).is_some()
            && emitted.bound.contains(&member.id)
            && power_in(allowlist, &op).is_none()
        {
            return Err(format!(
                "`{op}` is bound although a rule denies it, and it is not in the gated allowlist"
            ));
        }
    }
    for (op, _) in &emitted.gated {
        if power_in(allowlist, op).is_none() {
            return Err(format!("`{op}` is gated but not in the allowlist"));
        }
    }
    Ok(())
}

/// The members of one family, sorted by name (handles only: a trait's method is not a member).
fn family_members_of<'a>(members: &'a [MemberIr], family: &str) -> Vec<&'a MemberIr> {
    let mut found: Vec<&MemberIr> = members
        .iter()
        .filter(|m| !m.from_trait && m.owner == family)
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Classifies one member: bound with its conversions, or the reason it is skipped.
fn classify(
    member: &MemberIr,
    family: &str,
    families: &[&str],
    plain: &Plain,
) -> Result<Bound, String> {
    if member.kind != MemberKind::Function {
        return Err("other: a const item".to_owned());
    }
    if !member.generics.is_empty() {
        return Err(format!("generics: {}", member.generics));
    }
    let op = format!("{family}::{}", member.name);
    let gated = power_in(GATED_WRITES, &op);
    if let (Some(reason), None) = (powerful(member), gated) {
        return Err(reason);
    }
    if matches!(member.receiver, Receiver::None | Receiver::RefMut) {
        return Err("other: no receiver, or &mut self".to_owned());
    }
    let kind = match member.params.first() {
        Some(first) => host_kind(first)?,
        None => None,
    };
    let host = kind.is_some();
    let write = kind == Some(HostKind::Server);
    let params = if host {
        &member.params[1..]
    } else {
        member.params.as_slice()
    };
    let args = params
        .iter()
        .map(|param| arg_of(&param.rust_type, families, plain))
        .collect::<Result<Vec<Arg>, String>>()?;
    if !host && args.contains(&Arg::State) {
        return Err("unconvertible arg: BlockState needs a host".to_owned());
    }
    // `Result<T, E>` binds as its `Ok` value: an `Err` raises (see `ok_or_raise`).
    let (value_text, refusal) = match ok_type(member.ret.as_deref().unwrap_or("()")) {
        Some(ok) => (ok, true),
        None => (member.ret.as_deref().unwrap_or("()"), false),
    };
    let (ret, ret_ty) = match value_text {
        "()" => (Ret::Unit, None),
        "WorldEpoch" => (Ret::Epoch, None),
        _ => {
            let mapped =
                map_type(value_text).map_err(|_| format!("unconvertible return: {value_text}"))?;
            let ret = ret_of(&mapped.ty, families, plain)
                .ok_or_else(|| format!("unconvertible return: {value_text}"))?;
            (ret, Some(mapped.ty))
        }
    };
    if ret == Ret::State && !host {
        return Err("unconvertible return: BlockState needs a host".to_owned());
    }
    if ret == Ret::State && write {
        return Err("unconvertible return: BlockState needs the read host".to_owned());
    }
    let heavy = HEAVY_NAMES.contains(&member.name.as_str())
        || HEAVY_PREFIXES
            .iter()
            .any(|prefix| member.name.starts_with(prefix))
        || ret_ty.as_ref().is_some_and(returns_list)
        || matches!(ret, Ret::ListFamily | Ret::OptListFamily);
    Ok(Bound {
        id: member.id.clone(),
        op,
        gated,
        name: member.name.clone(),
        host,
        write,
        args,
        ret,
        refusal,
        int_ret: ret_ty
            .as_ref()
            .is_some_and(|ty| matches!(ty, TypeIr::Int(_))),
        cost: if heavy {
            HEAVY
        } else if write {
            WRITE
        } else {
            READ
        },
    })
}

/// The success type's text of a `Result<T, E>` return, or `None` for any other return.
pub(crate) fn ok_type(text: &str) -> Option<&str> {
    let inner = text.strip_prefix("Result<")?.strip_suffix('>')?;
    let mut depth = 0i32;
    for (index, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => return Some(inner[..index].trim()),
            _ => {}
        }
    }
    None
}

/// The host a method takes as its first parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostKind {
    /// `&dyn Host`: a read.
    Read,
    /// `&mut dyn ServerHost`: a write on the server.
    Server,
}

/// The host kind of the first parameter, or `None` when it is not a host (a handle argument, say).
/// A client's write is refused by name, as are other hosts: `&dyn ClientHost` is a client read
/// this slice does not bind.
fn host_kind(param: &ParamIr) -> Result<Option<HostKind>, String> {
    let text = param.rust_type.as_str();
    let Ok(mapped) = map_type(text) else {
        return Ok(None);
    };
    match (mapped.access, &mapped.ty) {
        (Access::Ref, TypeIr::Dyn(name)) if name == "Host" => Ok(Some(HostKind::Read)),
        (Access::RefMut, TypeIr::Dyn(name)) if name == "ServerHost" => Ok(Some(HostKind::Server)),
        (Access::RefMut, TypeIr::Dyn(name)) if name == "ClientHost" => {
            Err(format!("client write: {text}"))
        }
        (Access::RefMut, TypeIr::Dyn(_)) => Err(format!("&mut host: {text}")),
        (_, TypeIr::Dyn(_)) => Err(format!("other: host kind {text}")),
        _ => Ok(None),
    }
}

fn returns_list(ty: &TypeIr) -> bool {
    match ty {
        TypeIr::List(_) => true,
        TypeIr::Option(inner) => returns_list(inner),
        _ => false,
    }
}

/// How an argument of type `text` converts, or why it does not.
fn arg_of(text: &str, families: &[&str], plain: &Plain) -> Result<Arg, String> {
    let unconvertible = || format!("unconvertible arg: {text}");
    let mapped = map_type(text).map_err(|_| {
        if text.contains("impl") {
            format!("other: closure or impl Trait: {text}")
        } else {
            format!("other: unparsed type: {text}")
        }
    })?;
    // `Option<&str>` loses its reference in `map_type`, so it is recognised by its text.
    if text.split_whitespace().collect::<String>() == "Option<&str>" {
        return Ok(Arg::OptStr);
    }
    match mapped.access {
        Access::RefMut => return Err(format!("other: &mut argument: {text}")),
        Access::Ref => {
            return match &mapped.ty {
                TypeIr::Str => Ok(Arg::Get {
                    ty: "String".to_owned(),
                    by_ref: true,
                }),
                TypeIr::Named(name) if name == "Identifier" => Ok(Arg::Get {
                    ty: path_of(name),
                    by_ref: true,
                }),
                _ => Err(unconvertible()),
            };
        }
        Access::Owned => {}
    }
    match &mapped.ty {
        TypeIr::Named(name) if families.contains(&name.as_str()) => {
            Ok(Arg::Handle { ty: path_of(name) })
        }
        TypeIr::Option(inner) if is_family(inner, families) => match &**inner {
            TypeIr::Named(name) => Ok(Arg::OptHandle { ty: path_of(name) }),
            _ => Err(unconvertible()),
        },
        TypeIr::Named(name) if name == "BlockState" => Ok(Arg::State),
        ty => spelling(ty, plain)
            .map(|ty| Arg::Get { ty, by_ref: false })
            .ok_or_else(unconvertible),
    }
}

fn is_named(ty: &TypeIr, name: &str) -> bool {
    matches!(ty, TypeIr::Named(found) if found == name)
}

/// Whether a type is one of the generated handle families: a handle of any of them converts.
fn is_family(ty: &TypeIr, families: &[&str]) -> bool {
    matches!(ty, TypeIr::Named(found) if families.contains(&found.as_str()))
}

/// The Rust path of a named type in the generated code.
fn path_of(name: &str) -> String {
    match name {
        "text::Text" | "Text" => "mod_api::Text".to_owned(),
        "Duration" => "std::time::Duration".to_owned(),
        other => format!("mod_api::{other}"),
    }
}

/// The Rust spelling of a type that `FromLuaValue` converts, or `None`.
fn spelling(ty: &TypeIr, plain: &Plain) -> Option<String> {
    Some(match ty {
        TypeIr::Bool => "bool".to_owned(),
        TypeIr::Float(FloatKind::F32) => "f32".to_owned(),
        TypeIr::Float(FloatKind::F64) => "f64".to_owned(),
        TypeIr::Int(_) => scalar_name(ty)?.to_owned(),
        TypeIr::Str => "String".to_owned(),
        TypeIr::Option(inner) => format!("Option<{}>", spelling(inner, plain)?),
        TypeIr::List(elem) => format!("Vec<{}>", spelling(elem, plain)?),
        TypeIr::Array(elem, len) => {
            let name = scalar_name(elem)?;
            if !FROM_LUA_ARRAYS.contains(&(name, *len)) {
                return None;
            }
            format!("[{name}; {len}]")
        }
        TypeIr::Named(name) if HAND_NAMED.contains(&name.as_str()) => path_of(name),
        TypeIr::Named(name) if plain.decides(name) => format!("mod_api::{name}"),
        _ => return None,
    })
}

/// How a result becomes a Lua value, or `None` when it has no conversion.
fn ret_of(ty: &TypeIr, families: &[&str], plain: &Plain) -> Option<Ret> {
    Some(match ty {
        TypeIr::Str => Ret::Text,
        TypeIr::Option(inner) if matches!(**inner, TypeIr::Str) => Ret::OptText,
        ty if is_family(ty, families) => Ret::Family,
        TypeIr::Option(inner) if is_family(inner, families) => Ret::OptFamily,
        TypeIr::List(inner) if is_family(inner, families) => Ret::ListFamily,
        TypeIr::Option(inner) if matches!(&**inner, TypeIr::List(elem) if is_family(elem, families)) => {
            Ret::OptListFamily
        }
        TypeIr::Option(inner) if is_named(inner, "BlockState") => Ret::State,
        ty if value_ret(ty, plain) => Ret::Value,
        _ => return None,
    })
}

/// Whether a value of this type has a `ToLuaValue` impl in the convert crate.
fn value_ret(ty: &TypeIr, plain: &Plain) -> bool {
    match ty {
        TypeIr::Bool | TypeIr::Float(_) | TypeIr::Str => true,
        TypeIr::Int(_) => scalar_name(ty).is_some(),
        TypeIr::Option(inner) | TypeIr::List(inner) => value_ret(inner, plain),
        TypeIr::Array(elem, len) => {
            scalar_name(elem).is_some_and(|name| TO_LUA_ARRAYS.contains(&(name, *len)))
        }
        TypeIr::Named(name) => HAND_NAMED.contains(&name.as_str()) || plain.contains(name),
        _ => false,
    }
}

/// The `impl Family` of one family: its name, its `tostring` and its methods.
fn family_impl(family: &str, bound: &[Bound]) -> String {
    // An id accessor with no host and an integer result shows in `tostring` (`EntityRef(7)`).
    let has_id = bound
        .iter()
        .any(|b| b.name == "id" && !b.host && b.args.is_empty() && b.int_ret);
    let describe = if has_id {
        format!("format!(\"{family}({{}})\", self.id())")
    } else {
        format!("String::from(\"{family}\")")
    };
    let mut methods = String::new();
    for method in bound {
        let _ = writeln!(
            methods,
            "        methods.add_function(\"{name}\", |lua: &mlua::Lua, (this, args): (mlua::AnyUserData, mlua::MultiValue)| {{\n            \
             crate::hostcall::host_call(lua, \"{op}\", this, args)\n        }});",
            name = method.name,
            op = method.op
        );
    }
    let param = if bound.is_empty() {
        "_methods"
    } else {
        "methods"
    };
    format!(
        "impl crate::handles::Family for mod_api::{family} {{\n    \
         const NAME: &'static str = \"{family}\";\n    \
         fn describe(self) -> String {{\n        {describe}\n    }}\n    \
         fn add_methods<M: mlua::UserDataMethods<crate::handles::Token<Self>>>({param}: &mut M) {{\n{methods}    }}\n}}\n\n"
    )
}

/// The function of one bound member: it converts the receiver and the arguments (before the host
/// is touched), calls the game's method, and converts the result.
fn operation(family: &str, bound: &Bound) -> String {
    let fn_name = format!("{}_{}", snake_case(family), bound.name);
    let op = &bound.op;
    let mut body = String::new();
    // A write needs the server host before anything else: a turn without one refuses the call
    // before its arguments are converted or the game is asked anything.
    if bound.write {
        body.push_str("    let server = crate::handles::server_of(host)?;\n");
    }
    if bound.args.is_empty() {
        let _ = writeln!(body, "    crate::handles::Args::new(\"{op}\", args, 0)?;");
    } else {
        let _ = writeln!(
            body,
            "    let args = crate::handles::Args::new(\"{op}\", args, {})?;",
            bound.args.len()
        );
    }
    let _ = writeln!(
        body,
        "    let this: mod_api::{family} = crate::handles::receiver(this, \"{op}\")?;"
    );
    // The host a state argument reads through: the read host, or the server host as a read.
    let mut call_args = Vec::new();
    let read_host = if bound.write {
        "&*server"
    } else {
        if bound.host {
            body.push_str("    let read = crate::handles::read_of(host)?;\n");
        }
        "read"
    };
    if bound.write {
        call_args.push("server".to_owned());
    } else if bound.host {
        call_args.push("read".to_owned());
    }
    for (index, arg) in bound.args.iter().enumerate() {
        let name = format!("a{index}");
        let (line, pass) = match arg {
            Arg::Get { by_ref, .. } => (
                format!("args.get({index})?"),
                if *by_ref {
                    format!("&{name}")
                } else {
                    name.clone()
                },
            ),
            Arg::Handle { .. } => (format!("args.handle({index})?"), name.clone()),
            Arg::OptHandle { .. } => (format!("args.opt_handle({index})?"), name.clone()),
            Arg::State => (format!("args.state({index}, {read_host})?"), name.clone()),
            Arg::OptStr => (format!("args.get({index})?"), format!("{name}.as_deref()")),
        };
        let ty = match arg {
            Arg::Get { ty, .. } | Arg::Handle { ty } => ty.clone(),
            Arg::OptHandle { ty } => format!("Option<{ty}>"),
            Arg::State => "mod_api::BlockState".to_owned(),
            Arg::OptStr => "Option<String>".to_owned(),
        };
        let _ = writeln!(body, "    let {name}: {ty} = {line};");
        call_args.push(pass);
    }
    let call = format!("this.{}({})", bound.name, call_args.join(", "));
    // A `Result` raises its `Err` (`ok_or_raise`). A unit result is a statement, not a binding.
    let (call, unit) = match (bound.ret, bound.refusal) {
        (Ret::Unit, true) => (format!("crate::handles::ok_or_raise({call})?"), true),
        (Ret::Unit, false) => (call, true),
        (_, true) => (format!("crate::handles::ok_or_raise({call})?"), false),
        (_, false) => (call, false),
    };
    if unit {
        let _ = writeln!(body, "    {call};");
    } else {
        let _ = writeln!(body, "    let out = {call};");
    }
    let result = match bound.ret {
        Ret::Unit => "Ok(mlua::Value::Nil)".to_owned(),
        Ret::Value => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)".to_owned(),
        Ret::Text => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out.to_string(), lua)".to_owned(),
        Ret::OptText => {
            "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out.map(|value| value.to_string()), lua)".to_owned()
        }
        Ret::Family => "crate::handles::family_out(lua, out)".to_owned(),
        Ret::OptFamily => "crate::handles::family_opt_out(lua, out)".to_owned(),
        Ret::ListFamily => "crate::handles::family_list_out(lua, out)".to_owned(),
        Ret::OptListFamily => "crate::handles::family_opt_list_out(lua, out)".to_owned(),
        Ret::State => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&crate::stacks::state_in(read, out.as_ref()).flatten(), lua)"
            .to_owned(),
        Ret::Epoch => "event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out.get(), lua)".to_owned(),
    };
    let host_param = if bound.host { "host" } else { "_host" };
    // A unit result converts nothing, so the Lua state is not used.
    let lua_param = if bound.ret == Ret::Unit {
        "_lua"
    } else {
        "lua"
    };
    format!(
        "fn {fn_name}({lua_param}: &mlua::Lua, {host_param}: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {{\n{body}    {result}\n}}\n\n"
    )
}

/// The op table: each operation's cost, and the dispatch to its function.
fn table(bound: &[Bound]) -> String {
    let mut costs = String::new();
    let mut runs = String::new();
    for op in bound {
        let _ = writeln!(costs, "        \"{}\" => {},", op.op, op.cost);
        let fn_name = format!(
            "{}_{}",
            snake_case(op.op.split("::").next().unwrap_or_default()),
            op.name
        );
        let _ = writeln!(
            runs,
            "        \"{}\" => {fn_name}(lua, host, this, args),",
            op.op
        );
    }
    let mut out = String::from(
        "/// What an operation costs in budget units, by its `Family::method` name, or `None` for a name\n\
         /// that is not bound.\n\
         pub fn cost(op: &str) -> Option<u32> {\n    Some(match op {\n",
    );
    out.push_str(&costs);
    out.push_str("        _ => return None,\n    })\n}\n\n");
    out.push_str(
        "/// Runs one bound operation with the turn's host and the handle's userdata, or `None` for a name\n\
         /// that is not bound.\n\
         pub fn run(\n    lua: &mlua::Lua,\n    host: &mut crate::hostcall::TurnHost<'_>,\n    op: &str,\n    this: &mlua::AnyUserData,\n    args: mlua::MultiValue,\n) -> Option<mlua::Result<mlua::Value>> {\n    Some(match op {\n",
    );
    out.push_str(&runs);
    out.push_str("        _ => return None,\n    })\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handles;

    /// A surface with the members the tests name: two families, and handles and methods of each.
    fn members(signatures: &[(&str, &str, &str)]) -> Vec<MemberIr> {
        let handles: Vec<serde_json::Value> = signatures
            .iter()
            .map(|(owner, name, signature)| {
                serde_json::json!({
                    "id": format!("handle:{owner}::{name}"),
                    "hash": "00",
                    "signature": signature,
                    "wasm": true,
                    "refs": []
                })
            })
            .collect();
        let document = serde_json::json!({
            "schema": 2,
            "hash": "ab",
            "handles": handles,
            "traits": []
        });
        handles::load(document.to_string().as_bytes()).expect("the document loads")
    }

    fn plain() -> Plain {
        Plain::deciding(["LeashHolder", "Direction"], ["Direction"])
    }

    /// Classifies the member a signature names (`fn name(...)`) in the `EntityRef` family.
    fn one(signature: &str) -> Result<Bound, String> {
        let name = signature
            .trim_start_matches("fn ")
            .split(['(', '<'])
            .next()
            .unwrap_or_default();
        let member = members(&[("EntityRef", name, signature)]).remove(0);
        classify(&member, "EntityRef", &["EntityRef"], &plain())
    }

    #[test]
    fn a_plain_read_is_bound_with_the_host_and_no_arguments() {
        let bound = one("fn position(self, _: &dyn Host) -> Option<[f64; 3]>").unwrap();
        assert!(bound.host);
        assert!(bound.args.is_empty());
        assert_eq!(bound.ret, Ret::Value);
        assert_eq!(bound.cost, READ);
        assert_eq!(bound.op, "EntityRef::position");
    }

    #[test]
    fn an_option_return_and_a_handle_argument_are_bound() {
        let bound = one("fn f(self, _: &dyn Host, _: EntityRef) -> Option<bool>").unwrap();
        assert_eq!(
            bound.args,
            [Arg::Handle {
                ty: "mod_api::EntityRef".to_owned()
            }]
        );
        assert_eq!(bound.ret, Ret::Value);
    }

    #[test]
    fn a_list_of_the_family_is_bound_and_heavy() {
        let bound = one("fn passengers(self, _: &dyn Host) -> Option<Vec<EntityRef>>").unwrap();
        assert_eq!(bound.ret, Ret::OptListFamily);
        assert_eq!(bound.cost, HEAVY);
        let found = one("fn find_thing(self, _: &dyn Host) -> Option<bool>").unwrap();
        assert_eq!(found.cost, HEAVY);
    }

    #[test]
    fn an_optional_handle_and_a_block_state_result_convert_through_the_host() {
        let level = one("fn level(self, _: &dyn Host) -> Option<LevelRef>");
        assert!(level.is_err(), "LevelRef is not in the family");
        let state =
            one("fn block_state(self, _: &dyn Host, _: [i32; 3]) -> Option<BlockState>").unwrap();
        assert_eq!(state.ret, Ret::State);
        assert_eq!(
            state.args,
            [Arg::Get {
                ty: "[i32; 3]".to_owned(),
                by_ref: false
            }]
        );
        let direction = one("fn signal(self, _: &dyn Host, _: Direction) -> Option<i32>").unwrap();
        assert_eq!(
            direction.args,
            [Arg::Get {
                ty: "mod_api::Direction".to_owned(),
                by_ref: false
            }]
        );
    }

    #[test]
    fn a_str_argument_is_read_as_a_string_and_passed_by_reference() {
        let bound = one("fn tag(self, _: &dyn Host, _: &str) -> Option<bool>").unwrap();
        assert_eq!(
            bound.args,
            [Arg::Get {
                ty: "String".to_owned(),
                by_ref: true
            }]
        );
    }

    #[test]
    fn the_skip_reasons_are_the_named_categories() {
        let cases = [
            (
                "fn set(self, _: &mut dyn ClientHost) -> bool",
                "client write",
            ),
            ("fn f(self, _: &dyn Host, _: &mut Vec<u8>) -> bool", "other"),
            (
                "fn f<T: Copy>(self, _: &dyn Host, _: T) -> Option<bool>",
                "generics",
            ),
            (
                "fn f(self, _: &dyn Host) -> Result<LivingRef, Error>",
                "unconvertible return",
            ),
            (
                "fn f(self, _: &dyn Host) -> Option<LivingRef>",
                "unconvertible return",
            ),
            (
                "fn f(self, _: &dyn Host, _: PlayerRef) -> bool",
                "unconvertible arg",
            ),
            (
                "fn f(self, _: &dyn Host, _: impl FnOnce(u8)) -> bool",
                "other",
            ),
        ];
        for (signature, category) in cases {
            let reason = one(signature).expect_err(signature);
            assert_eq!(reason_category(&reason), category, "{signature}: {reason}");
        }
    }

    #[test]
    fn an_id_accessor_shows_in_tostring_and_a_family_without_one_does_not() {
        let with_id = members(&[("EntityRef", "id", "fn id(self) -> u32")]);
        let text = emit(&with_id, &["EntityRef"], &plain(), "ab").text;
        assert!(
            text.contains("format!(\"EntityRef({})\", self.id())"),
            "{text}"
        );
        let without = emit(
            &members(&[("LevelRef", "epoch", "fn epoch(self) -> WorldEpoch")]),
            &["LevelRef"],
            &plain(),
            "ab",
        )
        .text;
        assert!(without.contains("String::from(\"LevelRef\")"), "{without}");
    }

    #[test]
    fn one_bound_member_emits_its_function_its_registration_and_its_table_row() {
        let all = members(&[(
            "EntityRef",
            "is_in_water",
            "fn is_in_water(self, _: &dyn Host) -> Option<bool>",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        let golden = "fn entity_ref_is_in_water(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    crate::handles::Args::new(\"EntityRef::is_in_water\", args, 0)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::is_in_water\")?;\n    let read = crate::handles::read_of(host)?;\n    let out = this.is_in_water(read);\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(out.text.contains(golden), "{}", out.text);
        assert!(out.text.contains("methods.add_function(\"is_in_water\""));
        assert!(out.text.contains("\"EntityRef::is_in_water\" => 1,"));
        assert_eq!(out.bound, ["handle:EntityRef::is_in_water"]);
        assert!(out.skipped.is_empty());
    }

    #[test]
    fn a_skipped_member_is_listed_with_its_reason_in_the_coverage() {
        let all = members(&[(
            "EntityRef",
            "set",
            "fn set(self, _: &mut dyn ClientHost) -> bool",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        assert!(
            out.coverage
                .contains("handle:EntityRef::set skipped: client write: &mut dyn ClientHost"),
            "{}",
            out.coverage
        );
        assert_eq!(out.per_family, [("EntityRef".to_owned(), 1, 0)]);
    }

    #[test]
    fn the_family_list_is_a_parameter() {
        let all = members(&[
            ("EntityRef", "id", "fn id(self) -> u32"),
            ("LevelRef", "epoch", "fn epoch(self) -> WorldEpoch"),
        ]);
        let only_level = emit(&all, &["LevelRef"], &plain(), "ab");
        assert_eq!(only_level.per_family, [("LevelRef".to_owned(), 1, 1)]);
        assert!(
            !only_level
                .text
                .contains("impl crate::handles::Family for mod_api::EntityRef")
        );
    }

    #[test]
    fn a_vec_of_handles_result_golden() {
        let all = members(&[(
            "EntityRef",
            "passengers",
            "fn passengers(self, _: &dyn Host) -> Option<Vec<EntityRef>>",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        let golden = "fn entity_ref_passengers(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    crate::handles::Args::new(\"EntityRef::passengers\", args, 0)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::passengers\")?;\n    let read = crate::handles::read_of(host)?;\n    let out = this.passengers(read);\n    crate::handles::family_opt_list_out(lua, out)\n}\n";
        assert!(out.text.contains(golden), "{}", out.text);
        assert_eq!(out.heavy, ["handle:EntityRef::passengers"]);
    }

    #[test]
    fn a_handle_argument_golden() {
        let all = members(&[(
            "EntityRef",
            "is_passenger_of_same_vehicle",
            "fn is_passenger_of_same_vehicle(self, _: &dyn Host, _: EntityRef) -> Option<bool>",
        )]);
        let out = emit(&all, &["EntityRef"], &plain(), "ab");
        let golden = "fn entity_ref_is_passenger_of_same_vehicle(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    let args = crate::handles::Args::new(\"EntityRef::is_passenger_of_same_vehicle\", args, 1)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::is_passenger_of_same_vehicle\")?;\n    let read = crate::handles::read_of(host)?;\n    let a0: mod_api::EntityRef = args.handle(0)?;\n    let out = this.is_passenger_of_same_vehicle(read, a0);\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(out.text.contains(golden), "{}", out.text);
    }

    #[test]
    fn only_the_bound_members_argument_types_are_decided() {
        let all = members(&[
            (
                "EntityRef",
                "signal",
                "fn signal(self, _: &dyn Host, _: Direction) -> Option<i32>",
            ),
            (
                "EntityRef",
                "hand",
                "fn hand(self, _: &dyn Host, _: PlayerRef, _: Hand) -> Option<bool>",
            ),
        ]);
        let names = bound_argument_types(
            &all,
            &["EntityRef"],
            &Plain::deciding(["Direction", "Hand"], ["Direction", "Hand"]),
        );
        assert_eq!(names, ["Direction"]);
    }

    #[test]
    fn the_default_families_are_the_ref_owners_and_a_family_that_binds_nothing_is_listed() {
        let all = members(&[
            ("EntityRef", "id", "fn id(self) -> u32"),
            ("LivingRef", "entity", "fn entity(self) -> EntityRef"),
            ("SoundRef", "fade", "fn fade(self, _: &mut dyn ClientHost)"),
            (
                "Config",
                "value",
                "fn value(self, _: &dyn Host) -> Option<bool>",
            ),
        ]);
        assert_eq!(
            candidate_families(&all),
            ["EntityRef", "LivingRef", "SoundRef"]
        );
        let out = emit_listing(
            &all,
            &["EntityRef", "LivingRef"],
            &["SoundRef"],
            &plain(),
            "ab",
        );
        assert_eq!(
            out.per_family,
            [
                ("EntityRef".to_owned(), 1, 1),
                ("LivingRef".to_owned(), 1, 1),
                ("SoundRef".to_owned(), 1, 0),
            ]
        );
        assert!(
            out.coverage
                .contains("# not generated (no member binds): SoundRef"),
            "{}",
            out.coverage
        );
        assert!(
            out.coverage
                .contains("SoundRef: 1 members, not generated: no member binds"),
            "{}",
            out.coverage
        );
        assert!(
            out.coverage
                .contains("handle:SoundRef::fade skipped: client write: &mut dyn ClientHost"),
            "{}",
            out.coverage
        );
        assert!(!out.text.contains("for mod_api::SoundRef"));
    }

    #[test]
    fn an_id_that_needs_the_host_is_not_shown_in_tostring() {
        let all = members(&[(
            "BossBarRef",
            "id",
            "fn id(self, _: &dyn Host) -> Option<String>",
        )]);
        let text = emit(&all, &["BossBarRef"], &plain(), "ab").text;
        assert!(text.contains("String::from(\"BossBarRef\")"), "{text}");
        assert!(!text.contains("self.id()"), "{text}");
    }

    /// The function text of one write, from a one-member family of `EntityRef`.
    fn write_text(signature: &str) -> String {
        let name = signature
            .trim_start_matches("fn ")
            .split(['(', '<'])
            .next()
            .unwrap_or_default();
        let all = members(&[("EntityRef", name, signature)]);
        emit(&all, &["EntityRef"], &plain(), "ab").text
    }

    #[test]
    fn a_bool_write_golden_runs_through_the_server_host_before_its_arguments() {
        let text = write_text("fn set_glowing(self, _: &mut dyn ServerHost, _: bool) -> bool");
        let golden = "fn entity_ref_set_glowing(lua: &mlua::Lua, host: &mut crate::hostcall::TurnHost<'_>, this: &mlua::AnyUserData, args: mlua::MultiValue) -> mlua::Result<mlua::Value> {\n    let server = crate::handles::server_of(host)?;\n    let args = crate::handles::Args::new(\"EntityRef::set_glowing\", args, 1)?;\n    let this: mod_api::EntityRef = crate::handles::receiver(this, \"EntityRef::set_glowing\")?;\n    let a0: bool = args.get(0)?;\n    let out = this.set_glowing(server, a0);\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(text.contains(golden), "{text}");
        assert!(text.contains("\"EntityRef::set_glowing\" => 10,"), "{text}");
    }

    #[test]
    fn an_option_write_golden_passes_an_optional_str_as_a_str() {
        let text = write_text(
            "fn set_name(self, _: &mut dyn ServerHost, _: Option<&str>) -> Option<bool>",
        );
        let golden = "    let a0: Option<String> = args.get(0)?;\n    let out = this.set_name(server, a0.as_deref());\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(text.contains(golden), "{text}");
    }

    #[test]
    fn a_result_write_golden_raises_its_err_and_returns_its_ok() {
        let text = write_text(
            "fn set_data(self, _: &mut dyn ServerHost, _: u32) -> Result<bool, ChunkRefusal>",
        );
        let golden = "    let a0: u32 = args.get(0)?;\n    let out = crate::handles::ok_or_raise(this.set_data(server, a0))?;\n    event_horizon_lua_convert::scalar::ToLuaValue::to_lua_value(&out, lua)\n}\n";
        assert!(text.contains(golden), "{text}");
    }

    #[test]
    fn a_unit_result_write_golden_returns_nil() {
        let text = write_text("fn nudge(self, _: &mut dyn ServerHost) -> Result<(), ChunkRefusal>");
        let golden =
            "    crate::handles::ok_or_raise(this.nudge(server))?;\n    Ok(mlua::Value::Nil)\n}\n";
        assert!(text.contains(golden), "{text}");
        assert!(
            text.contains("fn entity_ref_nudge(_lua: &mlua::Lua"),
            "{text}"
        );
        assert!(text.contains("\"EntityRef::nudge\" => 10,"), "{text}");
    }

    #[test]
    fn a_write_that_returns_a_list_costs_a_heavy_read() {
        let bound =
            one("fn drop_all(self, _: &mut dyn ServerHost) -> Option<Vec<EntityRef>>").unwrap();
        assert!(bound.write);
        assert_eq!(bound.cost, HEAVY);
    }

    #[test]
    fn a_write_with_no_server_host_is_bound_only_with_its_host_kind() {
        let client =
            one("fn set(self, _: &mut dyn ClientHost, _: bool) -> bool").expect_err("client");
        assert_eq!(reason_category(&client), "client write");
        let write = one("fn set(self, _: &mut dyn ServerHost, _: bool) -> bool").unwrap();
        assert!(write.host && write.write);
        assert_eq!(write.cost, WRITE);
    }

    #[test]
    fn a_read_is_not_judged_by_the_powerful_name_parts() {
        let read = one("fn raiders(self, _: &dyn Host) -> Option<bool>").unwrap();
        assert!(!read.write);
    }

    /// The reason a signature (a write of `EntityRef`) is skipped, or the panic when it binds.
    fn powerful_reason(signature: &str) -> String {
        one(signature).expect_err(signature)
    }

    #[test]
    fn the_powerful_table_denies_the_names_the_design_lists() {
        let cases = [
            "fn run_command(self, _: &mut dyn ServerHost, _: &str) -> bool",
            "fn merge_nbt(self, _: &mut dyn ServerHost) -> bool",
            "fn spawn_mob(self, _: &mut dyn ServerHost) -> bool",
            "fn explode(self, _: &mut dyn ServerHost) -> bool",
            "fn set_game_rule(self, _: &mut dyn ServerHost) -> bool",
            "fn remove(self, _: &mut dyn ServerHost) -> bool",
            "fn strike_lightning(self, _: &mut dyn ServerHost) -> bool",
            "fn set_world_border_size(self, _: &mut dyn ServerHost) -> bool",
            "fn disconnect(self, _: &mut dyn ServerHost) -> bool",
            "fn set_game_mode(self, _: &mut dyn ServerHost) -> bool",
            "fn invalidate_capabilities(self, _: &mut dyn ServerHost) -> bool",
            "fn award_stat(self, _: &mut dyn ServerHost) -> bool",
            "fn reset_stat(self, _: &mut dyn ServerHost) -> bool",
            "fn set_nickname(self, _: &mut dyn ServerHost, _: &str) -> bool",
        ];
        for signature in cases {
            let reason = powerful_reason(signature);
            assert_eq!(
                reason_category(&reason),
                "powerful (needs manifest power)",
                "{signature}: {reason}"
            );
        }
    }

    /// The two explosion reads carry the `explo` name part but need no power: `powerful` leaves them
    /// alone, while another `explo` write stays powerful.
    #[test]
    fn the_explosion_reads_need_no_power_and_another_explosion_write_does() {
        let read = members(&[(
            "LevelRef",
            "explosion_resistance",
            "fn explosion_resistance(self, _: &mut dyn ServerHost, _: [i32; 3]) -> Option<bool>",
        )])
        .remove(0);
        assert_eq!(powerful(&read), None);
        let write = members(&[(
            "LevelRef",
            "explosion_clear",
            "fn explosion_clear(self, _: &mut dyn ServerHost) -> bool",
        )])
        .remove(0);
        assert!(powerful(&write).is_some());
    }

    /// The `EntityRef` members of [`GATED_WRITES`] bind behind their power; the same name on a family
    /// the list does not name stays denied.
    #[test]
    fn a_listed_write_binds_with_its_power_and_an_unlisted_one_stays_denied() {
        for (signature, power) in [
            ("fn discard(self, _: &mut dyn ServerHost) -> bool", "world"),
            (
                "fn teleport_to_level(self, _: &mut dyn ServerHost, _: &str) -> bool",
                "world",
            ),
        ] {
            let bound = one(signature).unwrap_or_else(|reason| panic!("{signature}: {reason}"));
            assert_eq!(bound.gated, Some(power), "{signature}");
            assert_eq!(bound.cost, WRITE, "{signature}");
        }
        assert!(
            powerful_reason("fn remove(self, _: &mut dyn ServerHost) -> bool")
                .contains("name contains `remove`")
        );
        let plain_write =
            one("fn set_velocity(self, _: &mut dyn ServerHost, _: [f64; 3]) -> bool").unwrap();
        assert_eq!(plain_write.gated, None);
        let level = members(&[(
            "LevelRef",
            "discard",
            "fn discard(self, _: &mut dyn ServerHost) -> bool",
        )])
        .remove(0);
        assert!(
            classify(&level, "LevelRef", &["LevelRef"], &plain()).is_err(),
            "LevelRef::discard is not listed"
        );
    }

    /// Emits the members of `signatures` (`(owner, name, signature)`) for the families given.
    fn emit_members(
        signatures: &[(&str, &str, &str)],
        families: &[&str],
    ) -> (Emitted, Vec<MemberIr>) {
        let surface = members(signatures);
        let emitted = emit_listing(&surface, families, &[], &plain(), "ab");
        (emitted, surface)
    }

    #[test]
    fn a_gated_write_emits_its_op_and_its_power_row_from_the_same_member() {
        let (emitted, _) = emit_members(
            &[(
                "EntityRef",
                "discard",
                "fn discard(self, _: &mut dyn ServerHost) -> bool",
            )],
            &["EntityRef"],
        );
        assert_eq!(
            emitted.gated,
            [("EntityRef::discard".to_owned(), "world".to_owned())]
        );
        assert!(
            emitted
                .coverage
                .contains("  handle:EntityRef::discard bound, cost 10, gated: world\n"),
            "{}",
            emitted.coverage
        );
        assert!(
            emitted.coverage.contains("  gated: world 1\n"),
            "{}",
            emitted.coverage
        );
        assert!(
            emitted
                .powers
                .contains("    (\"EntityRef::discard\", crate::manifest::Power::World),\n"),
            "{}",
            emitted.powers
        );
        // The op the row names is the string the generated method passes to `host_call`.
        for (op, _) in &emitted.gated {
            assert!(
                emitted.text.contains(&format!(
                    "crate::hostcall::host_call(lua, \"{op}\", this, args)"
                )),
                "{op} is not called by its method"
            );
        }
    }

    #[test]
    fn the_gated_check_passes_for_a_complete_allowlist_and_fails_for_each_hard_error() {
        let discard = [(
            "EntityRef",
            "discard",
            "fn discard(self, _: &mut dyn ServerHost) -> bool",
        )];
        let (emitted, surface) = emit_members(&discard, &["EntityRef"]);
        let plain = plain();
        let families = ["EntityRef"];
        assert!(
            check_gated(
                &[("EntityRef::discard", "world")],
                &surface,
                &families,
                &plain,
                &emitted
            )
            .is_ok()
        );
        let missing = check_gated(
            &[("EntityRef::nope", "world")],
            &surface,
            &families,
            &plain,
            &emitted,
        )
        .expect_err("missing");
        assert!(
            missing.contains("`EntityRef::nope` is not a member"),
            "{missing}"
        );
        let read = [(
            "EntityRef",
            "position",
            "fn position(self, _: &dyn Host) -> Option<[f64; 3]>",
        )];
        let (read_emitted, read_surface) = emit_members(&read, &["EntityRef"]);
        let not_write = check_gated(
            &[("EntityRef::position", "world")],
            &read_surface,
            &families,
            &plain,
            &read_emitted,
        )
        .expect_err("a read");
        assert!(
            not_write.contains("is not a `&mut dyn ServerHost` write"),
            "{not_write}"
        );
        let bad_power = check_gated(
            &[("EntityRef::discard", "network")],
            &surface,
            &families,
            &plain,
            &emitted,
        )
        .expect_err("reserved power");
        assert!(bad_power.contains("names power `network`"), "{bad_power}");
        let denied = [(
            "EntityRef",
            "merge_nbt",
            "fn merge_nbt(self, _: &mut dyn ServerHost) -> bool",
        )];
        let (denied_emitted, denied_surface) = emit_members(&denied, &["EntityRef"]);
        let unbound = check_gated(
            &[("EntityRef::merge_nbt", "nbt")],
            &denied_surface,
            &families,
            &plain,
            &denied_emitted,
        )
        .expect_err("unbound");
        assert!(unbound.contains("cannot be bound"), "{unbound}");
        let unlisted =
            check_gated(&[], &surface, &families, &plain, &emitted).expect_err("unlisted");
        assert!(
            unlisted.contains("is bound although a rule denies it"),
            "{unlisted}"
        );
    }

    #[test]
    fn a_powerful_write_is_denied_by_its_owner_name_or_argument_rule() {
        let registrar = members(&[(
            "Registrar",
            "add",
            "fn add(self, _: &mut dyn ServerHost) -> bool",
        )])
        .remove(0);
        assert_eq!(
            powerful(&registrar).as_deref(),
            Some("powerful (needs manifest power): owner Registrar")
        );
        let offer = members(&[(
            "MerchantRef",
            "set_offer",
            "fn set_offer(self, _: &mut dyn ServerHost, _: u32) -> bool",
        )])
        .remove(0);
        assert_eq!(
            powerful(&offer).as_deref(),
            Some("powerful (needs manifest power): MerchantRef method containing `offer`")
        );
        let transaction =
            powerful_reason("fn put(self, _: &mut dyn ServerHost, _: &Transaction) -> bool");
        assert!(
            transaction.ends_with("argument `&Transaction`"),
            "{transaction}"
        );
    }

    #[test]
    fn the_powerful_table_leaves_an_ordinary_write_bound() {
        assert!(one("fn set_velocity(self, _: &mut dyn ServerHost, _: [f64; 3]) -> bool").is_ok());
    }
}
