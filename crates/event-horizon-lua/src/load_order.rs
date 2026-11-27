//! The load order of the Lua mods the scanner accepted: a dependency loads before the mods that
//! need it, and a mod whose dependencies fail is refused with the reason.
//!
//! The planning is pure: it sees ids, versions, requirements and what each id is (enabled,
//! disabled, or absent), and nothing of the files. Every step is iterative, so a long chain or a
//! deep cycle in untrusted manifests costs memory, not stack.
//!
//! The steps: direct checks of each mod's dependencies; refusal spread to every dependent along
//! the reverse edges; cycles found (iterative Tarjan) and refused; the spread again; then a Kahn
//! order of what is left, ties by (dev before installed, then id).

use std::collections::{BTreeSet, HashMap};

use crate::discover::Fault;

/// One dependency of a mod, as its manifest states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    pub id: String,
    pub requirement: String,
    pub optional: bool,
}

/// A mod the scanner accepted and that is enabled (a dev mod always is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub dev: bool,
    pub version: String,
    pub deps: Vec<Dep>,
}

/// What the scanner knows of an id: an accepted, enabled entry (its index in the list), or a mod
/// that is installed and disabled. An id with neither is absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Enabled(usize),
    Disabled,
}

/// The result: the entry indexes to load, in order, and the entries refused with their reason.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub order: Vec<usize>,
    pub refused: Vec<(usize, Fault)>,
}

/// Plans the load order of `entries`. `presence` maps every id the scanner knows.
pub fn plan(entries: &[Entry], presence: &HashMap<String, Presence>) -> Plan {
    let n = entries.len();
    let mut fault: Vec<Option<Fault>> = vec![None; n];
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];

    // 1. Each mod's own dependencies: missing, disabled, wrong version. An optional dependency
    //    that is absent or disabled is ignored; a present one is ordered before its dependent.
    for (i, entry) in entries.iter().enumerate() {
        for dep in &entry.deps {
            let refusal = match presence.get(&dep.id) {
                None | Some(Presence::Disabled) if dep.optional => None,
                None => Some(Fault::DependencyMissing {
                    id: dep.id.clone(),
                    requirement: dep.requirement.clone(),
                }),
                Some(Presence::Disabled) => Some(Fault::DependencyDisabled { id: dep.id.clone() }),
                Some(Presence::Enabled(j)) => {
                    let found = &entries[*j].version;
                    if satisfies(found, &dep.requirement) {
                        edges[i].push(*j);
                        None
                    } else {
                        Some(Fault::DependencyVersion {
                            id: dep.id.clone(),
                            requirement: dep.requirement.clone(),
                            found: found.clone(),
                        })
                    }
                }
            };
            if let Some(refusal) = refusal {
                fault[i] = Some(refusal);
                break;
            }
        }
    }

    // 2. A mod that depends on a refused mod is refused too, and so on along the reverse edges.
    let dependents = reverse(&edges);
    spread(entries, &mut fault, &dependents);

    // 3. A cycle among the mods still standing: every member is refused, then the spread again.
    let standing: Vec<bool> = fault.iter().map(Option::is_none).collect();
    for (i, cyclic) in cyclic_nodes(&edges, &standing).into_iter().enumerate() {
        if cyclic && fault[i].is_none() {
            fault[i] = Some(Fault::DependencyCycle);
        }
    }
    spread(entries, &mut fault, &dependents);

    // 4. Kahn's order over the mods still standing. Ties: dev before installed, then by id.
    let standing: Vec<bool> = fault.iter().map(Option::is_none).collect();
    let mut waiting = vec![0usize; n];
    for i in (0..n).filter(|&i| standing[i]) {
        waiting[i] = edges[i].iter().filter(|&&j| standing[j]).count();
    }
    let rank = |i: usize| (u8::from(!entries[i].dev), entries[i].id.as_str(), i);
    let mut ready: BTreeSet<(u8, &str, usize)> = (0..n)
        .filter(|&i| standing[i] && waiting[i] == 0)
        .map(rank)
        .collect();
    let mut order = Vec::new();
    while let Some(key) = ready.pop_first() {
        let i = key.2;
        order.push(i);
        for &d in &dependents[i] {
            if standing[d] {
                waiting[d] -= 1;
                if waiting[d] == 0 {
                    ready.insert(rank(d));
                }
            }
        }
    }
    // Unreachable once the cycles are gone, but a mod that did not get an order is refused rather
    // than dropped without a word.
    for i in (0..n).filter(|&i| standing[i] && !order.contains(&i)) {
        fault[i] = Some(Fault::DependencyCycle);
    }

    let refused = fault
        .into_iter()
        .enumerate()
        .filter_map(|(i, f)| f.map(|f| (i, f)))
        .collect();
    Plan { order, refused }
}

/// Whether `version` satisfies `requirement`, by the same semver rules the packages use. A version
/// or requirement that does not parse satisfies nothing.
fn satisfies(version: &str, requirement: &str) -> bool {
    let Ok(found) = horizon_mod_package::dependencies::parse_version(version) else {
        return false;
    };
    // The requirement is read as the packages read their own: a string is a version requirement.
    let parsed: Result<horizon_mod_package::manifest::Dependency, _> =
        serde_json::from_value(serde_json::Value::String(requirement.to_owned()));
    match parsed {
        Ok(dep) => dep.details().version.matches(&found),
        Err(_) => false,
    }
}

/// The reverse of `edges`: for each mod, the mods that depend on it.
fn reverse(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut back = vec![Vec::new(); edges.len()];
    for (i, targets) in edges.iter().enumerate() {
        for &j in targets {
            back[j].push(i);
        }
    }
    back
}

/// Refuses every standing mod that reaches a refused one along `dependents`, with the reason that
/// names the refused dependency. Iterative: a queue of refused mods, not recursion.
fn spread(entries: &[Entry], fault: &mut [Option<Fault>], dependents: &[Vec<usize>]) {
    let mut queue: Vec<usize> = (0..fault.len()).filter(|&j| fault[j].is_some()).collect();
    while let Some(j) = queue.pop() {
        for &i in &dependents[j] {
            if fault[i].is_none() {
                fault[i] = Some(Fault::DependencyRefused {
                    id: entries[j].id.clone(),
                });
                queue.push(i);
            }
        }
    }
}

/// Which nodes lie on a cycle of `edges`, among the nodes `alive` marks. Iterative Tarjan: a node
/// is on a cycle when its strongly connected component has more than one node, or it depends on
/// itself.
fn cyclic_nodes(edges: &[Vec<usize>], alive: &[bool]) -> Vec<bool> {
    const UNSEEN: usize = usize::MAX;
    let n = edges.len();
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut cyclic = vec![false; n];
    let mut next = 0usize;
    for root in 0..n {
        if !alive[root] || index[root] != UNSEEN {
            continue;
        }
        // Each frame: a node and how many of its edges are done.
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(v, done)) = work.last() {
            if let Some(&w) = edges[v].get(done) {
                if let Some(frame) = work.last_mut() {
                    frame.1 += 1;
                }
                if !alive[w] {
                    continue;
                }
                if index[w] == UNSEEN {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == index[v] {
                let mut component = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                if component.len() > 1 || edges[v].contains(&v) {
                    for w in component {
                        cyclic[w] = true;
                    }
                }
            }
        }
    }
    cyclic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str, deps: &[(&str, &str, bool)]) -> Entry {
        Entry {
            id: id.into(),
            dev: false,
            version: version.into(),
            deps: deps
                .iter()
                .map(|(id, requirement, optional)| Dep {
                    id: (*id).into(),
                    requirement: (*requirement).into(),
                    optional: *optional,
                })
                .collect(),
        }
    }

    /// The presence map for `entries`: each is enabled at its index.
    fn enabled(entries: &[Entry]) -> HashMap<String, Presence> {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), Presence::Enabled(i)))
            .collect()
    }

    fn ids(entries: &[Entry], plan: &Plan) -> Vec<String> {
        plan.order.iter().map(|&i| entries[i].id.clone()).collect()
    }

    fn refused_ids(entries: &[Entry], plan: &Plan) -> Vec<(String, Fault)> {
        plan.refused
            .iter()
            .map(|(i, f)| (entries[*i].id.clone(), f.clone()))
            .collect()
    }

    #[test]
    fn a_chain_loads_dependencies_first() {
        // c needs b, b needs a: whatever the listing order, a then b then c.
        let entries = vec![
            entry("c", "1.0.0", &[("b", "^1", false)]),
            entry("b", "1.0.0", &[("a", ">=1", false)]),
            entry("a", "1.2.0", &[]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert!(plan.refused.is_empty());
        assert_eq!(ids(&entries, &plan), ["a", "b", "c"]);
    }

    #[test]
    fn a_diamond_loads_the_shared_base_once_and_first() {
        let entries = vec![
            entry(
                "top",
                "1.0.0",
                &[("left", "1", false), ("right", "1", false)],
            ),
            entry("right", "1.0.0", &[("base", "1", false)]),
            entry("left", "1.0.0", &[("base", "1", false)]),
            entry("base", "1.0.0", &[]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert!(plan.refused.is_empty());
        // Ties by id: left before right once base is out.
        assert_eq!(ids(&entries, &plan), ["base", "left", "right", "top"]);
    }

    #[test]
    fn a_missing_dependency_refuses_the_mod_with_its_requirement() {
        let entries = vec![entry("needs", "1.0.0", &[("gone", ">=2, <3", false)])];
        let plan = plan(&entries, &enabled(&entries));
        assert!(plan.order.is_empty());
        assert_eq!(
            refused_ids(&entries, &plan),
            [(
                "needs".to_string(),
                Fault::DependencyMissing {
                    id: "gone".into(),
                    requirement: ">=2, <3".into(),
                },
            )]
        );
    }

    #[test]
    fn a_wrong_version_refuses_with_the_version_found() {
        let entries = vec![
            entry("needs", "1.0.0", &[("base", "^2", false)]),
            entry("base", "1.4.0", &[]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert_eq!(ids(&entries, &plan), ["base"]);
        assert_eq!(
            refused_ids(&entries, &plan),
            [(
                "needs".to_string(),
                Fault::DependencyVersion {
                    id: "base".into(),
                    requirement: "^2".into(),
                    found: "1.4.0".into(),
                },
            )]
        );
    }

    #[test]
    fn a_disabled_dependency_refuses_and_an_optional_one_is_ignored() {
        let entries = vec![
            entry("needs", "1.0.0", &[("off", "1", false)]),
            entry("lenient", "1.0.0", &[("off", "1", true)]),
        ];
        let mut presence = enabled(&entries);
        presence.insert("off".into(), Presence::Disabled);
        let plan = plan(&entries, &presence);
        assert_eq!(ids(&entries, &plan), ["lenient"]);
        assert_eq!(
            refused_ids(&entries, &plan),
            [(
                "needs".to_string(),
                Fault::DependencyDisabled { id: "off".into() },
            )]
        );
    }

    #[test]
    fn a_missing_optional_dependency_is_ignored_and_a_present_one_loads_first() {
        let entries = vec![
            entry(
                "host",
                "1.0.0",
                &[("extra", "1", true), ("ghost", "1", true)],
            ),
            entry("extra", "1.0.0", &[]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert!(plan.refused.is_empty());
        assert_eq!(ids(&entries, &plan), ["extra", "host"]);
    }

    #[test]
    fn a_refusal_spreads_to_every_mod_that_depends_on_it() {
        let entries = vec![
            entry("top", "1.0.0", &[("middle", "1", false)]),
            entry("middle", "1.0.0", &[("broken", "1", false)]),
            entry("broken", "1.0.0", &[("nowhere", "1", false)]),
            entry("fine", "1.0.0", &[]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert_eq!(ids(&entries, &plan), ["fine"]);
        // In entry order: top, middle, broken.
        let refused = refused_ids(&entries, &plan);
        assert_eq!(
            refused,
            [
                (
                    "top".to_string(),
                    Fault::DependencyRefused {
                        id: "middle".into()
                    }
                ),
                (
                    "middle".to_string(),
                    Fault::DependencyRefused {
                        id: "broken".into()
                    }
                ),
                (
                    "broken".to_string(),
                    Fault::DependencyMissing {
                        id: "nowhere".into(),
                        requirement: "1".into(),
                    }
                ),
            ]
        );
    }

    #[test]
    fn a_cycle_of_two_is_refused_and_does_not_hang() {
        let entries = vec![
            entry("ping", "1.0.0", &[("pong", "1", false)]),
            entry("pong", "1.0.0", &[("ping", "1", false)]),
            entry("alone", "1.0.0", &[]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert_eq!(ids(&entries, &plan), ["alone"]);
        let refused = refused_ids(&entries, &plan);
        assert_eq!(refused.len(), 2);
        assert!(refused.iter().all(|(_, f)| *f == Fault::DependencyCycle));
    }

    #[test]
    fn a_cycle_of_three_refuses_its_members_and_their_dependents() {
        let entries = vec![
            entry("a", "1.0.0", &[("b", "1", false)]),
            entry("b", "1.0.0", &[("c", "1", false)]),
            entry("c", "1.0.0", &[("a", "1", false)]),
            entry("after", "1.0.0", &[("c", "1", false)]),
        ];
        let plan = plan(&entries, &enabled(&entries));
        assert!(plan.order.is_empty());
        let refused = refused_ids(&entries, &plan);
        assert_eq!(refused.len(), 4);
        assert_eq!(refused[0].1, Fault::DependencyCycle);
        assert_eq!(refused[2].1, Fault::DependencyCycle);
        assert_eq!(
            refused[3],
            (
                "after".to_string(),
                Fault::DependencyRefused { id: "c".into() }
            )
        );
    }

    #[test]
    fn a_mod_that_depends_on_itself_is_a_cycle() {
        let entries = vec![entry("loop", "1.0.0", &[("loop", "1", false)])];
        let plan = plan(&entries, &enabled(&entries));
        assert_eq!(
            refused_ids(&entries, &plan),
            [("loop".to_string(), Fault::DependencyCycle)]
        );
    }

    #[test]
    fn a_long_chain_is_planned_without_recursion() {
        // A chain of 1024 mods, each needing the one before: no stack depth problem.
        let mut entries = vec![entry("m0000", "1.0.0", &[])];
        for i in 1..1024 {
            let dep = format!("m{:04}", i - 1);
            let me = format!("m{i:04}");
            entries.push(Entry {
                id: me,
                dev: false,
                version: "1.0.0".into(),
                deps: vec![Dep {
                    id: dep,
                    requirement: "1".into(),
                    optional: false,
                }],
            });
        }
        let plan = plan(&entries, &enabled(&entries));
        assert!(plan.refused.is_empty());
        assert_eq!(plan.order.len(), 1024);
        assert_eq!(plan.order, (0..1024).collect::<Vec<_>>());
    }

    #[test]
    fn dev_mods_come_first_among_equals() {
        let mut entries = vec![entry("aaa", "1.0.0", &[]), entry("zzz", "1.0.0", &[])];
        entries[1].dev = true;
        let plan = plan(&entries, &enabled(&entries));
        assert_eq!(ids(&entries, &plan), ["zzz", "aaa"]);
    }

    #[test]
    fn the_requirement_is_matched_with_semver_rules() {
        assert!(satisfies("1.2.3", "^1"));
        assert!(!satisfies("2.0.0", "^1"));
        assert!(satisfies("1.2.9", "~1.2"));
        assert!(!satisfies("1.3.0", "~1.2"));
        assert!(satisfies("0.2.5", "^0.2"));
        assert!(!satisfies("0.3.0", "^0.2"));
        assert!(satisfies("1.5", ">=1.4, <2"));
        assert!(!satisfies("1.0.0-beta", ">=1"));
        assert!(!satisfies("not-a-version", "1"));
    }
}
