use cargo_metadata::{DependencyKind, Metadata, Node, Package, PackageId};
use clippy_utils::diagnostics::span_lint;
use itertools::Itertools as _;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_lint::LateContext;
use rustc_span::DUMMY_SP;

use super::MULTIPLE_CRATE_VERSIONS;

pub(super) fn check(cx: &LateContext<'_>, metadata: &Metadata, allowed_duplicate_crates: &FxHashSet<String>) {
    let local_name = cx.tcx.crate_name(LOCAL_CRATE);
    let mut packages = metadata.packages.clone();
    packages.sort_by(|a, b| a.name.cmp(&b.name));

    if let Some(resolve) = &metadata.resolve
        && let Some(local_id) = packages.iter().find_map(|p| {
            // p.name contains the original crate names with dashes intact
            // local_name contains the crate name as a namespace, with the dashes converted to underscores
            // the code below temporarily rectifies this discrepancy
            if p.name
                .as_bytes()
                .iter()
                .map(|b| if b == &b'-' { &b'_' } else { b })
                .eq(local_name.as_str().as_bytes())
            {
                Some(&p.id)
            } else {
                None
            }
        })
    {
        let normal_deps = normal_dependencies(&resolve.nodes, local_id);
        for (name, group) in &packages
            .iter()
            .filter(|p| !allowed_duplicate_crates.contains(p.name.as_str()))
            .chunk_by(|p| &p.name)
        {
            let group: Vec<&Package> = group.collect();

            if group.len() <= 1 {
                continue;
            }

            if group.iter().all(|p| normal_deps.contains(&p.id)) {
                let mut versions: Vec<_> = group.into_iter().map(|p| &p.version).collect();
                versions.sort();
                let versions = versions.iter().join(", ");

                span_lint(
                    cx,
                    MULTIPLE_CRATE_VERSIONS,
                    DUMMY_SP,
                    format!("multiple versions for dependency `{name}`: {versions}"),
                );
            }
        }
    }
}

fn normal_dependencies(nodes: &[Node], local_id: &PackageId) -> FxHashSet<PackageId> {
    let normal_deps_by_id: FxHashMap<PackageId, Vec<PackageId>> = nodes
        .iter()
        .map(|node| {
            let deps = node
                .deps
                .iter()
                .filter(|dep| {
                    dep.dep_kinds
                        .iter()
                        .any(|info| matches!(info.kind, DependencyKind::Normal))
                })
                .map(|dep| dep.pkg.clone())
                .collect();

            (node.id.clone(), deps)
        })
        .collect();

    reachable_normal_dependencies(&normal_deps_by_id, local_id)
}

fn reachable_normal_dependencies(
    normal_deps_by_id: &FxHashMap<PackageId, Vec<PackageId>>,
    local_id: &PackageId,
) -> FxHashSet<PackageId> {
    let mut reachable = FxHashSet::default();
    let mut pending = vec![local_id.clone()];

    while let Some(id) = pending.pop() {
        if let Some(deps) = normal_deps_by_id.get(&id) {
            for dep in deps {
                if reachable.insert(dep.clone()) {
                    pending.push(dep.clone());
                }
            }
        }
    }

    reachable
}

#[cfg(test)]
mod test {
    use super::reachable_normal_dependencies;
    use cargo_metadata::PackageId;
    use rustc_data_structures::fx::FxHashMap;

    fn package_id(name: &str) -> PackageId {
        PackageId { repr: name.to_string() }
    }

    #[test]
    fn test_layered_shared_dependency_paths() {
        const LAYERS: usize = 30;

        let local = package_id("local");
        let mut graph = FxHashMap::default();

        let layers: Vec<_> = (0..LAYERS)
            .map(|i| [package_id(&format!("layer-{i}-a")), package_id(&format!("layer-{i}-b"))])
            .collect();

        graph.insert(local.clone(), layers[0].to_vec());

        for (current, next) in layers.iter().zip(layers.iter().skip(1)) {
            for node in current {
                graph.insert(node.clone(), next.to_vec());
            }
        }

        for node in layers.last().unwrap() {
            graph.insert(node.clone(), vec![]);
        }

        let reachable = reachable_normal_dependencies(&graph, &local);

        assert_eq!(reachable.len(), LAYERS * 2);
    }

    #[test]
    fn test_shared_dependency_paths() {
        // local -> a -> c
        //      -> b -> c
        let local = package_id("local");
        let a = package_id("a");
        let b = package_id("b");
        let c = package_id("c");

        let graph = FxHashMap::from_iter([
            (local.clone(), vec![a.clone(), b.clone()]),
            (a.clone(), vec![c.clone()]),
            (b.clone(), vec![c.clone()]),
            (c.clone(), vec![]),
        ]);

        let reachable = reachable_normal_dependencies(&graph, &local);

        assert_eq!(reachable.len(), 3);
        assert!(reachable.contains(&a));
        assert!(reachable.contains(&b));
        assert!(reachable.contains(&c));
    }

    #[test]
    fn test_dependency_cycle() {
        // local -> a -> b -> a
        let local = package_id("local");
        let a = package_id("a");
        let b = package_id("b");

        let graph = FxHashMap::from_iter([
            (local.clone(), vec![a.clone()]),
            (a.clone(), vec![b.clone()]),
            (b.clone(), vec![a.clone()]),
        ]);

        let reachable = reachable_normal_dependencies(&graph, &local);

        assert_eq!(reachable.len(), 2);
        assert!(reachable.contains(&a));
        assert!(reachable.contains(&b));
    }
}
