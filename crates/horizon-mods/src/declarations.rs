//! A content mod's declarations, and what a reload keeps of them
//! (docs/modpacks.md).
//!
//! The declarations are the registry half of a mod's content: the blocks,
//! items, mob effects, entity types and menu types it declares. They load
//! once, when the game starts, so a root built again while the game runs has
//! to carry the same ones, in the same mods.
use crate::content::{Block, Content, ContentMod, EntityType, Item, MenuType, MobEffect};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// The registry half of a mod's content: what the block states, the item
/// table and the mob effect, entity type and menu type registries are built
/// from. Those load once, when the game starts (`Resources` and the
/// set-once registries read them), so a root built again while the game runs
/// has to carry the same ones, in the same mods: [`keep_startup_declarations`].
/// Its data and assets (loot, recipes, tags, models, lang) are free to change.
#[derive(Clone, Debug, PartialEq)]
pub struct Declarations {
    blocks: Vec<Block>,
    items: Vec<Item>,
    effects: Vec<MobEffect>,
    entity_types: Vec<EntityType>,
    menu_types: Vec<MenuType>,
}
impl Declarations {
    fn of(content: &Content) -> Self {
        Self {
            blocks: content.blocks.clone(),
            items: content.items.clone(),
            effects: content.effects.clone(),
            entity_types: content.entity_types.clone(),
            menu_types: content.menu_types.clone(),
        }
    }
    /// The template of the block `id` this mod declares, if it declares it.
    pub fn block_template(&self, id: &str) -> Option<&str> {
        self.blocks
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.template.as_str())
    }
    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
            && self.items.is_empty()
            && self.effects.is_empty()
            && self.entity_types.is_empty()
            && self.menu_types.is_empty()
    }
    fn put(&self, content: &mut Content) {
        content.blocks.clone_from(&self.blocks);
        content.items.clone_from(&self.items);
        content.effects.clone_from(&self.effects);
        content.entity_types.clone_from(&self.entity_types);
        content.menu_types.clone_from(&self.menu_types);
    }
}

/// Each mod's declarations, by mod id: what the root the game started on was
/// built from, kept for [`keep_startup_declarations`].
pub fn startup_declarations(mods: &[ContentMod]) -> BTreeMap<String, Declarations> {
    mods.iter()
        .map(|m| (m.id.clone(), Declarations::of(&m.content)))
        .collect()
}

/// Makes `mods`, read again while the game runs (a `/reload`, a pack reload),
/// declare what they did at start (`startup`), so the root built from them
/// has the block states and registry ids the running game has:
/// - a mod declaring the same things keeps them, and its data and assets are
///   what it has now;
/// - a mod whose declarations changed is layered with its startup ones, its
///   data and assets as they are now;
/// - a mod of the startup set that is not among `mods` (turned off, or its
///   content failed to read this time) is layered with its startup
///   declarations and no files;
/// - a mod not in the startup set is left out, unless it declares nothing.
///
/// Returns one notice per mod that is not taken as it is, each saying a
/// restart applies the change.
pub fn keep_startup_declarations(
    mods: &mut Vec<ContentMod>,
    startup: &BTreeMap<String, Declarations>,
) -> Vec<String> {
    let mut notices = Vec::new();
    mods.retain_mut(|m| {
        let now = Declarations::of(&m.content);
        match startup.get(&m.id) {
            Some(first) if *first == now => true,
            Some(first) => {
                first.put(&mut m.content);
                notices.push(format!(
                    "mod {}: its blocks, items, mob effects, entity types or menu types changed since the game started; they stay as they were until a restart, its data and assets are reloaded",
                    m.id
                ));
                true
            }
            None if now.is_empty() => true,
            None => {
                notices.push(format!(
                    "mod {}: a mod's blocks, items, mob effects, entity types and menu types load when the game starts; restart the game to load it",
                    m.id
                ));
                false
            }
        }
    });
    for (id, first) in startup {
        if mods.iter().any(|m| &m.id == id) || first.is_empty() {
            continue;
        }
        let mut content = Content::declared(id);
        first.put(&mut content);
        notices.push(format!(
            "mod {id}: its content is not read this time; its blocks, items, mob effects, entity types and menu types stay registered until a restart"
        ));
        mods.push(ContentMod {
            id: id.clone(),
            content,
        });
    }
    notices
}

/// The mod in `all` that declares the block `id`, and the block's template.
pub fn block_declared_by(
    all: &BTreeMap<String, Declarations>,
    id: &str,
) -> Option<(String, String)> {
    all.iter()
        .find_map(|(m, d)| d.block_template(id).map(|t| (m.clone(), t.to_string())))
}

/// In a root the game built: the absolute path of the base it was built over,
/// for [`base_of`].
pub const BASE: &str = "horizon-mod-content-base.txt";

/// The base root `root` stands for: the base a root built by the game's
/// content build was built over, when it still exists, else `root` itself. A
/// server reloading its content starts from the root it plays on, which is
/// the built root when content mods are on: the mods are layered over its
/// base again, not over a root that already has their blocks.
pub fn base_of(root: &Path) -> PathBuf {
    let Ok(recorded) = fs::read_to_string(root.join(BASE)) else {
        return root.to_owned();
    };
    let base = PathBuf::from(recorded.trim_end());
    if base.as_os_str().is_empty() || !base.join("facts.json").is_file() {
        return root.to_owned();
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built again while the game runs, the root carries the declarations the
    /// game started with, mod by mod, and the data the mods have now.
    #[test]
    fn a_rebuild_keeps_the_startup_declarations() {
        let block = |id: &str| Block {
            id: id.into(),
            template: "minecraft:test".into(),
            ..Default::default()
        };
        let item = |id: &str| Item {
            id: id.into(),
            template: "minecraft:test".into(),
            ..Default::default()
        };
        let with = |id: &str, blocks: &[Block], items: &[Item]| {
            let mut content = Content::declared(id);
            content.blocks = blocks.to_vec();
            content.items = items.to_vec();
            ContentMod {
                id: id.into(),
                content,
            }
        };
        let startup = startup_declarations(&[
            with("a", &[block("a:x")], &[]),
            with("b", &[], &[item("b:y")]),
            with("d", &[], &[]),
        ]);
        // a changed, b is gone, c is new with a block, d and e declare nothing.
        let mut now = vec![
            with("a", &[block("a:x"), block("a:z")], &[]),
            with("c", &[block("c:w")], &[]),
            with("d", &[], &[]),
            with("e", &[], &[]),
        ];
        let notices = keep_startup_declarations(&mut now, &startup);
        let ids: Vec<&str> = now.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "d", "e", "b"]);
        assert_eq!(now[0].content.blocks, [block("a:x")]);
        assert_eq!(now[3].content.items, [item("b:y")]);
        assert!(now[3].content.is_empty());
        assert_eq!(notices.len(), 3, "{notices:?}");
        for (notice, id) in notices.iter().zip(["a", "c", "b"]) {
            assert!(notice.starts_with(&format!("mod {id}:")), "{notice}");
            assert!(notice.contains("restart"), "{notice}");
        }
        // The same declarations again: nothing to say.
        let mut same = vec![
            with("a", &[block("a:x")], &[]),
            with("b", &[], &[item("b:y")]),
        ];
        assert!(keep_startup_declarations(&mut same, &startup).is_empty());
        assert_eq!(same.len(), 2);
    }
}
