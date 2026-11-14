//! Every item and recipe the game knows, made once for the mods that read
//! them (native mods' `native_recipes`, capability `recipes` of WebAssembly
//! mods, docs/mods.md), and the recipe book's known displays; placing a
//! recipe and creative gifts. The recipes are the window's own data — the
//! content root's — with the server's recipes of the types its native mods
//! had it send (`horizon:recipe_content`) in their place, while the known
//! displays follow the server's book as the recipe book does.
use super::{
    Game,
    native_hooks::{
        NativeItem as RecipeItem, NativeRecipe as Recipe, NativeRecipeKind as RecipeKind,
        NativeRecipeStack as RecipeStack, NativeRecipes as Recipes,
    },
};
use horizon_client::{
    crafting::{Ingredient, recipe_id},
    items::ItemStack,
    recipe_book::{Category, Display},
    stations::Smithing,
};
use std::{collections::HashMap, sync::Arc};

/// The tables, made once, and the recipe book's known displays with the
/// book's version they are of.
/// The most recipes a server's recipe content may carry.
const MAX_SYNCED_RECIPES: usize = 1_000_000;

#[derive(Default)]
pub(super) struct RecipeTables {
    tables: Option<Arc<Recipes>>,
    pub(super) categories: Option<HashMap<String, horizon_client::recipe_book::RecipeBookCategory>>,
    known: (Option<u64>, Arc<[i32]>),
    /// The server's recipes of the types its native mods sent (`horizon:
    /// recipe_content`) over the window's own: what `Game::recipes` reads on
    /// a client once they came.
    synced: Option<horizon_client::crafting::Recipes>,
    /// The display numbers of those recipes: the server numbers its book's
    /// displays over its own recipe set, a world's data pack recipes
    /// included, so the book numbers them the same (`Game::display_index`).
    pub(super) synced_index: Option<horizon_client::recipe_book::RecipeIndex>,
    /// The recipe content's parts so far, until its last.
    incoming: Vec<(String, serde_json::Value)>,
    /// The types that came before the player did: the mods hear of them
    /// once the helpers can answer (`recipes_received_now`).
    received: Option<Vec<String>>,
    /// The stacks mods asked the recipe viewer to show
    /// (`native_show_recipes`), heard next frame (`native_recipe_lookup`).
    pub(super) lookups: std::sync::Mutex<
        Vec<(
            horizon_client::mod_api::NativeStack,
            horizon_client::mod_api::NativeRecipeLookup,
        )>,
    >,
    /// Bumped each time a mod's recipe categories change
    /// (`native_recipe_categories_changed`).
    pub(super) categories_version: std::sync::atomic::AtomicU64,
}

/// Item id -> its index in [`Recipes::items`].
struct Index(HashMap<Arc<str>, u32>);
impl Index {
    fn stack(&self, item: &str, count: u32, components: String) -> Option<RecipeStack> {
        Some(RecipeStack {
            item: *self.0.get(item)?,
            count,
            components,
        })
    }
    fn of(&self, s: &ItemStack) -> Option<RecipeStack> {
        self.stack(&s.item, s.count, components(s))
    }
    fn any(&self, i: &Ingredient) -> Vec<RecipeStack> {
        i.order
            .iter()
            .filter_map(|id| self.stack(id, 1, String::new()))
            .collect()
    }
}

/// A stack's components beyond its item's, as vanilla's item syntax writes
/// them (`[minecraft:potion_contents={potion:"minecraft:swiftness"}]`), or "".
pub(super) fn components(s: &ItemStack) -> String {
    use horizon_world::nbt::Tag;
    let Some(Tag::Compound(entries)) = horizon_client::item_data::encode_components(s) else {
        return String::new();
    };
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                k.to_string_lossy(),
                horizon_client::snbt::to_snbt(v)
            )
        })
        .collect();
    format!("[{}]", parts.join(","))
}

fn recipe(id: String, kind: RecipeKind) -> Recipe {
    Recipe {
        id,
        kind,
        category: String::new(),
        group: String::new(),
        width: 0,
        height: 0,
        slots: Vec::new(),
        results: Vec::new(),
        cooking_time: 0,
        experience: 0.0,
        book_id: -1,
    }
}

impl Game<'_> {
    pub(super) fn registered_recipe_book_category(
        &mut self,
        kind: &str,
    ) -> Option<horizon_client::recipe_book::RecipeBookCategory> {
        if self.recipe_tables.categories.is_none() {
            let mut declarations = Vec::new();
            self.native_recipe_book_categories(&mut declarations);
            self.recipe_tables.categories = Some(horizon_client::recipe_book::mod_categories(
                declarations,
                self.recipes(),
                self.items(),
            ));
        }
        self.recipe_tables.categories.as_ref()?.get(kind).cloned()
    }
    /// `native_give`: `count` of `item`, refused outside a world, unless
    /// creative and for an item the game doesn't have.
    pub(super) fn recipe_give(&mut self, item: &str, count: u32) -> Result<(), String> {
        if self.player.is_none() || self.res.rest.get().is_none() {
            return Err("not in a world".into());
        }
        if !self.creative {
            return Err("only in creative mode".into());
        }
        if self.res.items().get(item).is_none() {
            return Err(format!("no item {item}"));
        }
        self.mod_give(item, count.clamp(1, 99), "");
        Ok(())
    }
    /// `native_transfer_recipe`, and with `all` as many sets as the
    /// inventory (and a mod crafting grid's sources) allow, as Shift on the
    /// recipe book places: every grid slot gets the same count, capped at the
    /// ingredients' stack size, alternatives (tags) shared out as vanilla's
    /// `StackedItemContents` does. A server from before `all` ignores it.
    /// API @API:transfer_recipe_all@.
    pub(super) fn native_transfer_recipe_all(&mut self, book_id: i32, all: bool) {
        self.transfer_viewer_recipe(book_id, all);
    }
    /// `native_place_recipe`: refused as `recipes.place` refuses it.
    pub(super) fn recipe_place(&mut self, book_id: i32, all: bool) -> Result<(), String> {
        if !self.mod_recipe_screen() {
            return Err("no screen that takes recipes is open".into());
        }
        if !self.known_displays().contains(&book_id) {
            return Err(format!(
                "recipe {book_id} is not in the player's recipe book"
            ));
        }
        self.mod_place_recipe(book_id, all);
        Ok(())
    }
    /// `recipes.give`: a creative player takes the stack as from the
    /// creative inventory; a server hears it as `SetCreativeModeSlot`
    /// through the inventory's sync, and refuses it unless creative.
    pub(super) fn mod_give(&mut self, item: &str, count: u32, components: &str) {
        let Some(max) = self.items().get(item).map(|i| i.max_stack) else {
            return;
        };
        if !self.creative {
            return;
        }
        let (damage, extra) = horizon_client::item_data::command_components(components)
            .map_or((0, None), |tag| {
                horizon_client::item_data::decode_components(Some(&tag))
            });
        let _ = self.give(ItemStack {
            item: item.into(),
            // One stack at most, as the creative inventory hands out.
            count: count.min(max),
            damage,
            extra,
        });
    }
    /// The tables, made on the first call; none while the world resources
    /// they come from are still loading behind the title on a slow start.
    pub(super) fn made_recipes(&mut self) -> Option<Arc<Recipes>> {
        self.res.rest.get()?;
        if self.recipe_tables.tables.is_none() {
            self.recipe_tables.tables = Some(Arc::new(self.make_mod_recipes()));
        }
        self.recipe_tables.tables.clone()
    }
    /// The `book_id`s the player's recipe book holds now.
    pub(super) fn known_displays(&mut self) -> Arc<[i32]> {
        if self.res.rest.get().is_none() {
            return Arc::from([]);
        }
        let (version, known) = self.recipe_book_displays();
        if self.recipe_tables.known.0 != Some(version) {
            self.recipe_tables.known = (Some(version), known.into());
        }
        self.recipe_tables.known.1.clone()
    }

    fn make_mod_recipes(&mut self) -> Recipes {
        let names = &self.pack.names;
        let items = self.items();
        let mut tags: HashMap<&str, Vec<String>> = HashMap::new();
        for (tag, members) in &items.item_tags {
            for m in members {
                tags.entry(m).or_default().push(tag.clone());
            }
        }
        let table: Vec<RecipeItem> = items
            .registry_order
            .iter()
            .map(|id| RecipeItem {
                id: id.to_string(),
                name: names.display_item(id),
                tags: tags.remove(&**id).map_or_else(Vec::new, |mut t| {
                    t.sort();
                    t
                }),
            })
            .collect();
        let index = Index(
            items
                .registry_order
                .iter()
                .enumerate()
                .map(|(i, id)| (id.clone(), i as u32))
                .collect(),
        );
        let mut out = Vec::new();
        let cooking: HashMap<String, (u32, f32)> = self
            .recipes()
            .cooking
            .iter()
            .map(|c| (recipe_id(&c.id), (c.time, c.experience)))
            .collect();
        for e in self.recipe_book_entries() {
            let furnace = |c: Category| match c.name().split('_').next() {
                Some("blast") => RecipeKind::Blasting,
                Some("smoker") => RecipeKind::Smoking,
                _ => RecipeKind::Smelting,
            };
            let kind = match &e.display {
                Display::Shaped { .. } => RecipeKind::CraftingShaped,
                Display::Shapeless(_) => RecipeKind::CraftingShapeless,
                Display::Furnace(_) => furnace(e.category),
            };
            let mut r = recipe(e.id.clone(), kind);
            r.category = e.category.name().to_owned();
            r.group = e.group.clone().unwrap_or_default();
            r.book_id = e.display_id;
            r.slots = match &e.display {
                Display::Shaped {
                    width,
                    height,
                    cells,
                } => {
                    (r.width, r.height) = (*width as u32, *height as u32);
                    cells
                        .iter()
                        .map(|c| c.as_ref().map_or_else(Vec::new, |i| index.any(i)))
                        .collect()
                }
                Display::Shapeless(list) => list.iter().map(|i| index.any(i)).collect(),
                Display::Furnace(i) => vec![index.any(i)],
            };
            // Requirements shown as other stacks (an arrow's potions): the
            // requirement's index counts the filled slots only.
            let filled: Vec<usize> = (0..r.slots.len())
                .filter(
                    |&k| !matches!(&e.display, Display::Shaped { cells, .. } if cells[k].is_none()),
                )
                .collect();
            for (req, stacks) in &e.stacks {
                if let Some(&k) = filled.get(*req) {
                    r.slots[k] = stacks.iter().filter_map(|s| index.of(s)).collect();
                }
            }
            r.results = e.results().iter().filter_map(|s| index.of(s)).collect();
            if let Some(&(time, xp)) = cooking.get(&e.id) {
                (r.cooking_time, r.experience) = (time, xp);
            }
            out.push(r);
        }
        for c in &self.recipes().cooking {
            if c.kind == "minecraft:campfire_cooking" {
                let mut r = recipe(recipe_id(&c.id), RecipeKind::Campfire);
                r.group = c.group.clone();
                r.slots = vec![index.any(&c.ingredient)];
                r.results = index.of(&c.result).into_iter().collect();
                (r.cooking_time, r.experience) = (c.time, c.experience);
                out.push(r);
            }
        }
        let stations = self.stations();
        for s in &stations.recipes.stonecutting {
            let mut r = recipe(s.id.clone(), RecipeKind::Stonecutting);
            r.slots = vec![index.any(&s.input)];
            r.results = index.of(&s.result).into_iter().collect();
            out.push(r);
        }
        for s in &stations.recipes.smithing {
            let (id, template, base, addition, results) = match s {
                Smithing::Transform {
                    id,
                    template,
                    base,
                    addition,
                    result,
                } => (
                    id,
                    template.as_ref(),
                    base,
                    addition.as_ref(),
                    result
                        .create()
                        .as_ref()
                        .and_then(|s| index.of(s))
                        .into_iter()
                        .collect(),
                ),
                Smithing::Trim {
                    id,
                    template,
                    base,
                    addition,
                    ..
                } => (id, Some(template), base, Some(addition), index.any(base)),
            };
            let mut r = recipe(id.clone(), RecipeKind::Smithing);
            r.slots = vec![
                template.map(|i| index.any(i)).unwrap_or_default(),
                index.any(base),
                addition.map(|i| index.any(i)).unwrap_or_default(),
            ];
            r.results = results;
            out.push(r);
        }
        for b in &stations.brewing.recipes {
            let potion = |p: &horizon_client::brewing::PotionIngredient| -> Vec<RecipeStack> {
                let mut item_ids: Vec<&String> = p.items.iter().collect();
                item_ids.sort();
                let mut potions: Vec<&String> = p.potions.iter().flatten().collect();
                potions.sort();
                item_ids
                    .into_iter()
                    .flat_map(|item| {
                        let one = |c: String| index.stack(item, 1, c);
                        if potions.is_empty() {
                            one(String::new()).into_iter().collect::<Vec<_>>()
                        } else {
                            potions
                                .iter()
                                .filter_map(|p| {
                                    one(format!("[minecraft:potion_contents={{potion:\"{p}\"}}]"))
                                })
                                .collect()
                        }
                    })
                    .collect()
            };
            let mut r = recipe(b.id.clone(), RecipeKind::Brewing);
            r.slots = vec![potion(&b.input), potion(&b.reagent)];
            r.results = index.of(&b.output).into_iter().collect();
            out.push(r);
        }
        Recipes {
            items: table,
            recipes: out,
        }
    }
}

impl Game<'_> {
    /// The server's recipes a client was sent, over its own; None on a
    /// server and until a server sends any.
    pub(super) fn synced_recipes(&self) -> Option<&horizon_client::crafting::Recipes> {
        self.recipe_tables.synced.as_ref()
    }
    /// The display numbers the recipe book reads: the server's recipes',
    /// once a server sent them, else the world's own.
    pub(super) fn display_index(&self) -> &horizon_client::recipe_book::RecipeIndex {
        match &self.recipe_tables.synced_index {
            Some(index) => index,
            None => &self.stations().recipe_index,
        }
    }
    /// A new play session (`Login`): what the last server sent is gone.
    pub(super) fn forget_synced_recipes(&mut self) {
        self.recipe_tables.synced = None;
        self.recipe_tables.synced_index = None;
        self.recipe_tables.incoming.clear();
        self.recipe_tables.received = None;
        self.recipe_tables.tables = None;
        self.recipe_ui_forget_server_entries();
        self.forget_book();
    }
    /// `native_recipes_received` for the types that came, once the window
    /// has its player: the recipes of a join come before it, and the helpers
    /// answer nothing until then. Once a frame.
    pub(super) fn recipes_received_now(&mut self) {
        if self.player.is_some()
            && let Some(types) = self.recipe_tables.received.take()
        {
            self.native_recipes_received(&types);
        }
    }
    /// `OnDatapackSyncEvent.sendRecipes`: the recipe types the native mods
    /// want their clients to have, and the types a mod's `data_pack_files`
    /// recipe file or a recipe filter changed in this load (`changed_types`),
    /// from this load's tables, become what the server sends its Horizon
    /// clients at a join and after `/reload`. Called once a load's tables are
    /// built, before any player hears of them.
    pub(super) fn sync_recipe_content(&mut self) {
        use horizon_client::lan::recipes;
        let mut types = Vec::new();
        self.native_datapack_sync(&mut types);
        // A Lua mod's `data_pack_files` recipe file, or a recipe filter, changed
        // this load's recipes: the types of those recipes go too, so a client's
        // table agrees with the server's without a native mod having asked. A
        // world's data packs are covered the same way: a client never has them.
        types.extend(recipes::changed_types(
            &self.options.resources,
            &self.pack_files(),
        ));
        types.sort();
        types.dedup();
        let sources = if types.is_empty() {
            Vec::new()
        } else {
            horizon_client::crafting::Recipes::sources(&self.options.resources, &self.pack_files())
                .unwrap_or_else(|e| {
                    log::warn!("Horizon: the recipes for the clients were not read: {e}");
                    Vec::new()
                })
        };
        recipes::set_content(recipes::encode_content(
            &types,
            &sources,
            recipes::sent_before(),
        ));
    }
    /// A `horizon:recipe_content` part the server sent the window. On the
    /// last, the window's tables take the server's recipes of those types in
    /// place of its own, and the mods hear of it (`RecipesReceivedEvent`).
    pub(super) fn remote_recipe_content(&mut self, data: &[u8]) {
        let part: horizon_client::lan::recipes::ContentPart = match serde_json::from_slice(data) {
            Ok(part) => part,
            Err(e) => {
                log::warn!("Horizon: bad recipe content from the server: {e}");
                return;
            }
        };
        self.recipe_tables.incoming.extend(part.recipes);
        // The server is trusted to end its parts; one that never does is
        // dropped, not kept growing.
        if self.recipe_tables.incoming.len() > MAX_SYNCED_RECIPES {
            log::warn!("Horizon: the server sent more than {MAX_SYNCED_RECIPES} recipes; dropped");
            self.recipe_tables.incoming = Vec::new();
            return;
        }
        let Some(types) = part.last else {
            return;
        };
        let received = std::mem::take(&mut self.recipe_tables.incoming);
        // No type any more: back on the window's own recipes.
        if types.is_empty() {
            self.recipe_tables.synced_index = None;
            if self.recipe_tables.synced.take().is_some() {
                self.recipe_tables.tables = None;
                self.forget_book();
                self.recipe_tables.received = Some(types);
                self.recipes_received_now();
            }
            return;
        }
        let root = &self.options.resources;
        let overlay = horizon_world::loot::PackOverlay::NONE;
        let built = horizon_client::crafting::Recipes::sources(root, &overlay).and_then(|own| {
            let of = |v: &serde_json::Value| types.iter().any(|t| v["type"].as_str() == Some(t));
            let mut all: Vec<_> = own.into_iter().filter(|(_, v)| !of(v)).collect();
            all.extend(received);
            // The server numbers the displays over these same recipes: a
            // world's data pack recipes shift the numbers of every recipe
            // after them, and the book must agree (the server's entries go
            // over its own by number).
            let index = horizon_client::recipe_book::RecipeIndex::load_from_sources(&all);
            horizon_client::crafting::Recipes::from_sources(root, self.items(), &overlay, all)
                .map(|recipes| (recipes, index))
        });
        match built {
            Ok((recipes, index)) => {
                self.recipe_tables.synced = Some(recipes);
                self.recipe_tables.synced_index = Some(index);
                // What was made of the window's own recipes is made again.
                self.recipe_tables.tables = None;
                self.forget_book();
                self.recipe_tables.received = Some(types);
                self.recipes_received_now();
            }
            Err(e) => log::warn!("Horizon: the server's recipes were not taken: {e}"),
        }
    }
}

impl Game<'_> {
    /// `native_loot_odds`: the odds of the world's loot table `table`.
    pub(super) fn loot_odds(
        &mut self,
        table: &str,
        tool: Option<&str>,
    ) -> Option<Vec<horizon_client::mod_api::NativeLootOdds>> {
        let loot = horizon_client::structure_blocks::loot_data(
            &self.options.resources,
            &self.pack_files(),
        )?;
        let odds = loot.odds(table, tool)?;
        Some(
            odds.into_iter()
                .map(|o| horizon_client::mod_api::NativeLootOdds {
                    item: o.item,
                    components: String::new(),
                    chance: o.chance,
                    min: o.min,
                    max: o.max,
                    conditions: o.conditions,
                })
                .collect(),
        )
    }
}

impl Game<'_> {
    /// The recipe lookups mods asked for since the last frame
    /// (`native_show_recipes`), heard by the recipe viewers.
    pub(super) fn recipe_lookups_now(&mut self) {
        let lookups = std::mem::take(
            &mut *self
                .recipe_tables
                .lookups
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for (stack, lookup) in lookups {
            let mut handled = false;
            self.native_recipe_lookup(&stack, &lookup, &mut handled);
        }
    }
}

#[cfg(test)]
#[path = "recipe_viewer_tests.rs"]
mod recipe_viewer_tests;

#[cfg(test)]
mod recipe_content_tests {
    use super::*;
    use horizon_client::lan::recipes::{ContentPart, changed_types, encode_content};
    use serde_json::json;

    const STICK: &str = r##"{"type":"minecraft:crafting_shaped","key":{"#":"#minecraft:planks"},"pattern":["#","#"],"result":{"id":"minecraft:stick","count":4}}"##;
    const PLANKS: &str = r##"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:oak_log"],"result":{"id":"minecraft:oak_planks","count":4}}"##;

    /// A resource root with vanilla's stick and planks recipes.
    fn scratch_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "horizon-recipe-content-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let recipes = dir.join("data/minecraft/recipe");
        std::fs::create_dir_all(&recipes).unwrap();
        std::fs::write(recipes.join("stick.json"), STICK).unwrap();
        std::fs::write(recipes.join("oak_planks.json"), PLANKS).unwrap();
        dir
    }

    fn part(payloads: &[Vec<u8>]) -> ContentPart {
        serde_json::from_slice(payloads.last().expect("one payload")).unwrap()
    }

    #[test]
    fn a_pushed_recipe_file_sends_its_type_and_the_file_not_vanillas() {
        let root = scratch_root("replaced");
        let replaced = json!({"type": "minecraft:crafting_shaped",
            "key": {"#": "#minecraft:planks"}, "pattern": ["#", "#"],
            "result": {"id": "minecraft:stick", "count": 7}});
        let files = vec![(
            "recipe/minecraft:stick".to_owned(),
            vec![replaced.to_string().into_bytes()],
        )];
        let overlay = horizon_world::loot::PackOverlay { files: &files };
        assert_eq!(
            changed_types(&root, &overlay),
            ["minecraft:crafting_shaped"]
        );
        let sources = horizon_client::crafting::Recipes::sources(&root, &overlay).unwrap();
        let types = changed_types(&root, &overlay);
        let sent = part(&encode_content(&types, &sources, false));
        let (_, stick) = sent
            .recipes
            .iter()
            .find(|(id, _)| id == "minecraft:stick")
            .expect("the replaced stick recipe is sent");
        assert_eq!(
            stick["result"]["count"], 7,
            "the pushed file, not vanilla's 4"
        );
    }

    #[test]
    fn a_replacement_of_another_type_sends_both_types() {
        let root = scratch_root("retyped");
        // Vanilla's shaped stick becomes the server's shapeless one.
        let retyped = json!({"type": "minecraft:crafting_shapeless",
            "ingredients": ["#minecraft:planks", "#minecraft:planks"],
            "result": {"id": "minecraft:stick", "count": 4}});
        let files = vec![(
            "recipe/minecraft:stick".to_owned(),
            vec![retyped.to_string().into_bytes()],
        )];
        let overlay = horizon_world::loot::PackOverlay { files: &files };
        assert_eq!(
            changed_types(&root, &overlay),
            ["minecraft:crafting_shaped", "minecraft:crafting_shapeless"]
        );
        let sources = horizon_client::crafting::Recipes::sources(&root, &overlay).unwrap();
        let sent = part(&encode_content(
            &changed_types(&root, &overlay),
            &sources,
            false,
        ));
        assert_eq!(
            sent.last.expect("the types"),
            ["minecraft:crafting_shaped", "minecraft:crafting_shapeless"]
        );
        let (_, stick) = sent
            .recipes
            .iter()
            .find(|(id, _)| id == "minecraft:stick")
            .expect("the server's stick recipe is sent");
        assert_eq!(stick["type"], "minecraft:crafting_shapeless");
    }

    #[test]
    fn an_added_recipe_is_sent_and_a_filter_removed_one_is_not() {
        let root = scratch_root("added-removed");
        let added = json!({"type": "minecraft:crafting_shapeless",
            "ingredients": ["minecraft:dirt"], "result": {"id": "minecraft:diamond", "count": 1}});
        let filter = json!({"filters": [{"id": "minecraft:stick", "remove": true}]});
        let files = vec![
            (
                "recipe/mymod:probe_item".to_owned(),
                vec![added.to_string().into_bytes()],
            ),
            (
                "recipe_filter/mymod:no_stick".to_owned(),
                vec![filter.to_string().into_bytes()],
            ),
        ];
        let overlay = horizon_world::loot::PackOverlay { files: &files };
        let types = changed_types(&root, &overlay);
        assert_eq!(
            types,
            ["minecraft:crafting_shaped", "minecraft:crafting_shapeless"]
        );
        let sources = horizon_client::crafting::Recipes::sources(&root, &overlay).unwrap();
        let sent = part(&encode_content(&types, &sources, false));
        let ids: Vec<&str> = sent.recipes.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"mymod:probe_item"),
            "the added recipe: {ids:?}"
        );
        assert!(
            !ids.contains(&"minecraft:stick"),
            "the removed one: {ids:?}"
        );
        assert_eq!(sent.last.as_deref(), Some(types.as_slice()));
    }

    #[test]
    fn a_load_that_changes_no_recipe_sends_nothing() {
        let root = scratch_root("unchanged");
        // Its own file back: no change, though the overlay is not empty.
        let files = vec![(
            "recipe/minecraft:stick".to_owned(),
            vec![STICK.as_bytes().to_vec()],
        )];
        let overlay = horizon_world::loot::PackOverlay { files: &files };
        assert!(changed_types(&root, &overlay).is_empty());
        // A file of another kind only: not a recipe, nothing to compare.
        let files = vec![(
            "loot_table/minecraft:blocks/stone".to_owned(),
            vec![b"{}".to_vec()],
        )];
        let overlay = horizon_world::loot::PackOverlay { files: &files };
        let types = changed_types(&root, &overlay);
        assert!(types.is_empty());
        let sources = horizon_client::crafting::Recipes::sources(&root, &overlay).unwrap();
        assert!(encode_content(&types, &sources, false).is_empty());
    }

    /// A server whose world pack replaces the stick recipe with 7, and the
    /// window that joins it: the window's own table is the root's, and it
    /// crafts the server's 7 sticks once the content arrives.
    #[test]
    #[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
    fn a_client_crafts_the_servers_replaced_recipe_after_the_content_arrives() {
        use crate::game::{Audio, Options, Progress, Settings, execution_role, load_resources};
        let resources =
            std::path::PathBuf::from(std::env::var_os("HORIZON_TEST_RESOURCES").unwrap());
        let settings = Settings::default();
        let loaded = load_resources(
            resources.clone(),
            None,
            &resources,
            &settings,
            std::time::Instant::now(),
            false,
            true,
            &Progress::new(),
            &mut |_, _| unreachable!(),
        )
        .unwrap();
        let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
        options.resources = resources.clone();
        options.execution_role = execution_role::ExecutionRole::IntegratedServer;
        let mut game = Game::new(
            &loaded.resources,
            loaded.pack,
            loaded.gui.unwrap(),
            options,
            settings,
            Audio::silent(&resources),
            None,
        )
        .unwrap();
        game.build_registries();
        let dir = std::env::temp_dir().join(format!("horizon-recipe-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pack = dir.join("datapacks/model");
        let recipes = pack.join("data/minecraft/recipe");
        std::fs::create_dir_all(&recipes).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            r#"{"pack":{"description":"fixture"}}"#,
        )
        .unwrap();
        let replaced = json!({"type": "minecraft:crafting_shaped",
            "key": {"#": "#minecraft:planks"}, "pattern": ["#", "#"],
            "result": {"id": "minecraft:stick", "count": 7}});
        std::fs::write(recipes.join("stick.json"), replaced.to_string()).unwrap();
        std::fs::write(
            dir.join("datapacks.json"),
            r#"{"enabled":["file/model"],"disabled":[]}"#,
        )
        .unwrap();
        game.world_root = dir;
        game.world_tables = game.build_world_tables(false, &[]).unwrap().0;
        assert!(game.world_tables.is_some(), "the world's pack is loaded");
        game.sync_recipe_content();
        let payloads = horizon_client::lan::recipes::content();
        assert!(!payloads.is_empty(), "the server sends the changed type");
        let last: ContentPart = serde_json::from_slice(payloads.last().unwrap()).unwrap();
        assert_eq!(
            last.last.as_deref(),
            Some(&["minecraft:crafting_shaped".to_owned()][..])
        );
        // The window that joins: no world tables of its own, so its table is
        // the root's until the server's content comes.
        game.world_tables = None;
        let planks = ItemStack::new("minecraft:oak_planks", 1);
        let grid = [Some(planks.clone()), None, Some(planks), None];
        let before = game.recipes().craft(&grid, 2).expect("the vanilla stick");
        assert_eq!(before.count, 4, "the window's own recipe first");
        for payload in payloads.iter() {
            game.remote_recipe_content(payload);
        }
        let after = game
            .recipes()
            .craft(&grid, 2)
            .expect("the server's stick recipe");
        assert_eq!((after.item.as_ref(), after.count), ("minecraft:stick", 7));
    }
}
