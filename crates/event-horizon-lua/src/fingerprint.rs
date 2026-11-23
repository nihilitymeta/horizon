//! The declaration fingerprint of a Lua mod: what its declaration pass said, by name.
//!
//! The game registers what a mod declares when it starts (its hooks, blocks, items, parts, commands,
//! settings and so on). A live swap of the mod's scripts can change the handlers, but not those
//! registrations, so a fresh state must declare exactly the same things. The fingerprint is what
//! `swap_state` compares. Every list is sorted, so the order a mod declared in does not matter.

use crate::declare::Declaration;
use crate::settings::Schema;

/// What a mod declared, by name and count. Two states with equal fingerprints register the same
/// things with the game, so one may replace the other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    id: String,
    version: String,
    setting_changed: bool,
    /// The hook names the mod subscribed to.
    hooks: Vec<String>,
    /// One entry per hook: its name, priority and whether it receives cancelled events.
    options: Vec<String>,
    /// `id (template)` for each block and item.
    blocks: Vec<String>,
    items: Vec<String>,
    /// `id: hook, hook` for each part, one list per kind of part.
    parts: Vec<String>,
    item_parts: Vec<String>,
    block_entity_parts: Vec<String>,
    fluid_parts: Vec<String>,
    entity_parts: Vec<String>,
    menu_parts: Vec<String>,
    block_entity_types: Vec<String>,
    fluids: Vec<String>,
    /// `entity name priority [hook, hook]` for each goal and behavior (behaviors add their duration).
    goals: Vec<String>,
    behaviors: Vec<String>,
    client_commands: Vec<String>,
    /// `usage level N` for each server command.
    commands: Vec<String>,
    /// `mod_id file` for each config.
    configs: Vec<String>,
    /// `id registry` for each data map type, and the key of each data pack file.
    data_map_types: Vec<String>,
    data_pack_files: Vec<String>,
    /// `key kind` for each setting of the mod's schema.
    settings: Vec<String>,
}

/// A swap that was refused: the names of what the fresh state declares differently, as the
/// messages say them (`blocks: added `mod:thing``).
#[cfg_attr(not(test), allow(dead_code))] // 5f-c2 is the first production caller
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SwapRefused {
    pub(crate) what: Vec<String>,
}

fn sorted(mut entries: Vec<String>) -> Vec<String> {
    entries.sort();
    entries
}

fn joined<T: AsRef<str>>(names: &[T]) -> String {
    names
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(", ")
}

fn part_entries(parts: &[crate::declare::PartRecord]) -> Vec<String> {
    sorted(
        parts
            .iter()
            .map(|p| format!("{}: {}", p.id, joined(&p.hooks)))
            .collect(),
    )
}

impl Fingerprint {
    /// The fingerprint of a mod with this id and version, whose declaration pass gave `declaration`
    /// and whose settings schema is `schema`.
    pub(crate) fn of(
        id: &str,
        version: &str,
        declaration: &Declaration,
        schema: &Schema,
    ) -> Fingerprint {
        let d = declaration;
        Fingerprint {
            id: id.to_string(),
            version: version.to_string(),
            setting_changed: d.setting_changed,
            hooks: sorted(d.hooks.clone()),
            options: sorted(
                d.options
                    .iter()
                    .map(|(hook, o)| {
                        format!(
                            "{hook} priority {} receive_cancelled {}",
                            o.priority, o.receive_cancelled
                        )
                    })
                    .collect(),
            ),
            blocks: sorted(
                d.blocks
                    .iter()
                    .map(|b| format!("{} ({})", b.id, b.template))
                    .collect(),
            ),
            items: sorted(
                d.items
                    .iter()
                    .map(|i| format!("{} ({})", i.id, i.template))
                    .collect(),
            ),
            parts: part_entries(&d.parts),
            item_parts: part_entries(&d.item_parts),
            block_entity_parts: part_entries(&d.block_entity_parts),
            fluid_parts: part_entries(&d.fluid_parts),
            entity_parts: part_entries(&d.entity_parts),
            menu_parts: part_entries(&d.menu_parts),
            block_entity_types: sorted(
                d.block_entity_types
                    .iter()
                    .map(|t| format!("{} on {}", t.id, joined(&t.blocks)))
                    .collect(),
            ),
            fluids: sorted(d.fluids.iter().map(|f| f.id.clone()).collect()),
            goals: sorted(
                d.goals
                    .iter()
                    .map(|g| {
                        format!(
                            "{} {} priority {} [{}]",
                            g.entity,
                            g.name,
                            g.priority,
                            joined(&g.hooks)
                        )
                    })
                    .collect(),
            ),
            behaviors: sorted(
                d.behaviors
                    .iter()
                    .map(|b| {
                        format!(
                            "{} {} priority {} duration {}..{} [{}]",
                            b.entity,
                            b.name,
                            b.priority,
                            b.min_duration,
                            b.max_duration,
                            joined(&b.hooks)
                        )
                    })
                    .collect(),
            ),
            client_commands: sorted(d.client_commands.iter().map(|c| c.usage.clone()).collect()),
            commands: sorted(
                d.commands
                    .iter()
                    .map(|c| format!("{} level {}", c.usage, c.level))
                    .collect(),
            ),
            configs: sorted(
                d.configs
                    .iter()
                    .map(|c| format!("{} {}", c.mod_id, c.file))
                    .collect(),
            ),
            data_map_types: sorted(
                d.data_map_types
                    .iter()
                    .map(|t| format!("{} {}", t.id, t.registry))
                    .collect(),
            ),
            data_pack_files: sorted(
                d.data_pack_files
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect(),
            ),
            settings: sorted(
                schema
                    .settings()
                    .iter()
                    .map(|s| format!("{} {:?}", s.key, s.kind))
                    .collect(),
            ),
        }
    }

    /// The differences from `fresh` (the fingerprint of the state a swap would install), one line
    /// each. Empty when the two are equal.
    #[cfg_attr(not(test), allow(dead_code))] // used by swap_state, see there
    pub(crate) fn differences(&self, fresh: &Fingerprint) -> Vec<String> {
        let mut out = Vec::new();
        if self.id != fresh.id {
            out.push(format!("id: `{}` is now `{}`", self.id, fresh.id));
        }
        if self.version != fresh.version {
            out.push(format!(
                "version: `{}` is now `{}`",
                self.version, fresh.version
            ));
        }
        if self.setting_changed != fresh.setting_changed {
            out.push(format!(
                "on_setting_changed: {}",
                if fresh.setting_changed {
                    "added"
                } else {
                    "removed"
                }
            ));
        }
        let lists: [(&str, &[String], &[String]); 20] = [
            ("hooks", &self.hooks, &fresh.hooks),
            ("hook options", &self.options, &fresh.options),
            ("blocks", &self.blocks, &fresh.blocks),
            ("items", &self.items, &fresh.items),
            ("parts", &self.parts, &fresh.parts),
            ("item parts", &self.item_parts, &fresh.item_parts),
            (
                "block entity parts",
                &self.block_entity_parts,
                &fresh.block_entity_parts,
            ),
            ("fluid parts", &self.fluid_parts, &fresh.fluid_parts),
            ("entity parts", &self.entity_parts, &fresh.entity_parts),
            ("menu parts", &self.menu_parts, &fresh.menu_parts),
            (
                "block entity types",
                &self.block_entity_types,
                &fresh.block_entity_types,
            ),
            ("fluids", &self.fluids, &fresh.fluids),
            ("goals", &self.goals, &fresh.goals),
            ("behaviors", &self.behaviors, &fresh.behaviors),
            (
                "client commands",
                &self.client_commands,
                &fresh.client_commands,
            ),
            ("commands", &self.commands, &fresh.commands),
            ("configs", &self.configs, &fresh.configs),
            (
                "data map types",
                &self.data_map_types,
                &fresh.data_map_types,
            ),
            (
                "data pack files",
                &self.data_pack_files,
                &fresh.data_pack_files,
            ),
            ("settings", &self.settings, &fresh.settings),
        ];
        for (label, old, new) in lists {
            for entry in new.iter().filter(|e| !old.contains(e)) {
                out.push(format!("{label}: added `{entry}`"));
            }
            for entry in old.iter().filter(|e| !new.contains(e)) {
                out.push(format!("{label}: removed `{entry}`"));
            }
        }
        out
    }
}
