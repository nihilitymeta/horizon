// exports: none (reached inside the crate only)
/// The package inventory carries the same namespace set as content files and
/// the package loader's collision fence. A package always owns its own ID.
pub fn owns_namespace(owner: &str, namespace: &str) -> bool {
    owner == namespace
        || crate::LoadedMods::new()
            .get(owner)
            .is_some_and(|package| package.namespaces().any(|owned| owned == namespace))
        || (granted(owner, namespace)
            && (namespace == owner || crate::LoadedMods::new().get(namespace).is_none()))
}

/// Namespaces the package inventory cannot list because they are found at runtime (the Lua
/// host's mods, whose ids are read from their folders): `(owner, namespace)`, append-only.
static GRANTS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// The namespaces no grant may take: the vanilla and client ones, and Horizon's own.
const RESERVED_GRANTS: [&str; 3] = ["minecraft", "c", "horizon"];

fn granted(owner: &str, namespace: &str) -> bool {
    GRANTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|(o, n)| o == owner && n == namespace)
}

/// Lets `owner` declare content in `namespace`, which it does not list in its package. Only a
/// namespace of 1 to 64 lowercase letters, digits or underscores (a Lua mod id) can be granted:
/// not a reserved one, not the id of another loaded package, and not one another owner already
/// holds. Granting the same pair again changes nothing. A grant is also dropped at check time
/// when a package with that id is loaded, because a host may grant before the packages are known.
pub fn grant_namespace(owner: &str, namespace: &str) -> Result<(), String> {
    let valid = (1..=64).contains(&namespace.len())
        && namespace
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        return Err(format!(
            "namespace {namespace:?} must be 1 to 64 lowercase letters, digits or underscores"
        ));
    }
    if RESERVED_GRANTS.contains(&namespace) {
        return Err(format!("namespace {namespace:?} is reserved"));
    }
    if namespace != owner && crate::LoadedMods::new().get(namespace).is_some() {
        return Err(format!(
            "namespace {namespace:?} is the id of another loaded package"
        ));
    }
    let mut grants = GRANTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match grants.iter().find(|(_, n)| n == namespace) {
        Some((holder, _)) if holder == owner => Ok(()),
        Some((holder, _)) => Err(format!(
            "namespace {namespace:?} is already granted to {holder:?}"
        )),
        None => {
            grants.push((owner.to_owned(), namespace.to_owned()));
            Ok(())
        }
    }
}


/// `name` with its namespace: a bare name is in mod `owner`'s own, for a declared id and
/// for the block and item references a declaration names (`block_item`, `pick_item`,
/// `wall_block`, `base_block`, `crafting_remainder`, `Item::block`) alike.
pub fn qualified(owner: &str, name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("{owner}:{name}")
    }
}

/// Validate and qualify a declaration using the package namespace inventory.
pub fn owned_declaration_id(owner: &str, name: &str) -> Result<String, String> {
    let id = qualified(owner, name);
    let parsed = crate::Identifier::parse_namespaced(&id)
        .ok_or_else(|| format!("mod {owner} declares invalid identifier {id:?}"))?;
    let namespace = parsed.namespace();
    if !owns_namespace(owner, namespace) {
        return Err(format!(
            "mod {owner} declares {id:?} in unowned namespace {namespace:?}"
        ));
    }
    Ok(id)
}

/// A declaration a mod's `blocks` or `items` hook adds: its id, and the names of other
/// content it references.
pub(crate) trait Declaration {
    /// The declared id and its optional references.
    fn names(&mut self) -> (&mut String, impl IntoIterator<Item = &mut Option<String>>);
}

impl Declaration for crate::Block {
    fn names(&mut self) -> (&mut String, impl IntoIterator<Item = &mut Option<String>>) {
        (
            &mut self.id,
            [
                &mut self.block_item,
                &mut self.pick_item,
                &mut self.wall_block,
                &mut self.base_block,
            ],
        )
    }
}

impl Declaration for crate::Item {
    fn names(&mut self) -> (&mut String, impl IntoIterator<Item = &mut Option<String>>) {
        (
            &mut self.id,
            [&mut self.crafting_remainder, &mut self.block],
        )
    }
}

/// Make `declaration` mod `owner`'s: each bare reference names its own content, and
/// the id must be in a namespace it owns. On an error the id is left as given.
pub(crate) fn own_declaration(
    owner: &str,
    declaration: &mut impl Declaration,
) -> Result<(), String> {
    let (id, references) = declaration.names();
    for reference in references.into_iter().flatten() {
        *reference = qualified(owner, reference);
    }
    *id = owned_declaration_id(owner, id)?;
    Ok(())
}

impl crate::RegistrationContext {
    /// Refuse this mod's registration with a diagnostic, without panicking.
    /// The first reason wins. Reasons exceeding 4096 UTF-8 bytes are truncated
    /// at a character boundary; shorter reasons are preserved verbatim.
    /// All this mod's declarations from this registration are discarded, including
    /// later declarations, without adding their validation errors to the report.
    /// Other mods' declarations are unchanged. Native and wasm use the same
    /// admission path. Returning from `Hooks::register` immediately is recommended.
    pub fn refuse(&self, reason: &str) {
        let mut reason_slot = self.self_refusal.lock().unwrap_or_else(|p| p.into_inner());
        if reason_slot.is_none() {
            *reason_slot = Some(super::registration_refusal::bounded_text(reason).to_owned());
        }
    }

    /// Report a loading problem that doesn't stop this mod: a missing optional integration,
    /// a setting it corrected. `key` is a translation key in the mod's own `lang/` files and
    /// `args` its `%s` arguments; the Mods screen shows it in the player's language, and the
    /// log names the key. The first 16 warnings are kept, each cut as `refuse` cuts.
    pub fn warn(&self, key: &str, args: &[&str]) {
        let mut warnings = self.warnings.lock().unwrap_or_else(|p| p.into_inner());
        if warnings.len() < super::registration_refusal::MAX_LOADING_WARNINGS {
            warnings.push(super::registration_refusal::LoadingMessage::new(key, args));
        }
    }

    pub(crate) fn refuse_declaration(&self, reason: String) {
        self.declaration_refusals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(reason);
    }
}

/// The engine consumes block/item declaration ownership refusals before loading
/// resources. Guest dispatch retains invalid output so the owning host boundary
/// sees and refuses it as well, rather than silently dropping a declaration.
// hidden: engine admission plumbing, re-exported through __private for the loader.
#[doc(hidden)]
pub fn take_declaration_refusals(context: &crate::RegistrationContext) -> Vec<String> {
    std::mem::take(
        &mut *context
            .declaration_refusals
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
    )
}

#[cfg(test)]
mod grant_tests {
    use super::*;

    #[test]
    fn a_granted_namespace_is_owned_by_its_holder_only() {
        grant_namespace("grant_host_a", "grant_mod_a").unwrap();
        assert!(owns_namespace("grant_host_a", "grant_mod_a"));
        assert!(!owns_namespace("grant_host_b", "grant_mod_a"));
        assert!(!owns_namespace("grant_host_a", "grant_mod_other"));
    }

    #[test]
    fn granting_the_same_pair_again_is_fine() {
        grant_namespace("grant_host_c", "grant_mod_c").unwrap();
        grant_namespace("grant_host_c", "grant_mod_c").unwrap();
    }

    #[test]
    fn another_owner_cannot_take_a_granted_namespace() {
        grant_namespace("grant_host_d", "grant_mod_d").unwrap();
        let err = grant_namespace("grant_host_e", "grant_mod_d").unwrap_err();
        assert!(err.contains("already granted"), "got {err}");
    }

    #[test]
    fn a_malformed_namespace_is_refused() {
        for bad in ["", "Mod", "my-mod", "my.mod", "my mod", "mod:x"] {
            let err = grant_namespace("grant_host_f", bad).unwrap_err();
            assert!(err.contains("lowercase letters"), "{bad:?}: got {err}");
        }
        let long = "a".repeat(65);
        assert!(grant_namespace("grant_host_f", &long).is_err());
        let exact = "a".repeat(64);
        grant_namespace("grant_host_f", &exact).unwrap();
    }

    #[test]
    fn a_reserved_namespace_is_refused() {
        for reserved in RESERVED_GRANTS {
            let err = grant_namespace("grant_host_g", reserved).unwrap_err();
            assert!(err.contains("reserved"), "{reserved}: got {err}");
        }
    }

    #[test]
    fn the_id_of_another_loaded_package_is_refused() {
        crate::world::phase::with_test_packages(&["grant_loaded_pkg"], || {
            let err = grant_namespace("grant_host_h", "grant_loaded_pkg").unwrap_err();
            assert!(err.contains("another loaded package"), "got {err}");
            // The owner's own id is not "another" package.
            crate::world::phase::with_test_packages(&["grant_host_i"], || {
                grant_namespace("grant_host_i", "grant_host_i").unwrap();
            });
        });
    }
}
