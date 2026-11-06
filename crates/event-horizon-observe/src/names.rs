//! The names a server sent, kept once each, up to a bound.

use std::sync::Mutex;

/// A name a server sent, as the `&'static str` the snapshots hold: kept
/// once each, up to a bound, so a server can't fill the client's memory.
pub fn received_name(name: &str) -> &'static str {
    use std::collections::HashSet;
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(known) = names.get(name) {
        return known;
    }
    if names.len() >= 1024 || name.len() > 96 {
        return "?";
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}
