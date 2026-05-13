//! Forward/reverse dependency graph derived from a [`Cache`].

use std::collections::{BTreeMap, BTreeSet};

use crate::cache::Cache;
use crate::paths::RelPath;

#[derive(Debug, Clone, Copy, Default)]
pub struct TraversalOpts {
    /// When false, `*.libsonnet` entries are omitted from output.
    /// Note: they are still walked through (a libsonnet may be the only path
    /// from a jsonnet root to deeper jsonnet deps), only filtered at emit.
    pub include_libsonnet: bool,
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub path: RelPath,
    pub children: Vec<TreeNode>,
    /// `true` if traversal stopped here because this node was already visited
    /// higher up the current path (cycle break).
    pub cycle: bool,
}

pub struct Graph {
    forward: BTreeMap<RelPath, Vec<RelPath>>,
    reverse: BTreeMap<RelPath, Vec<RelPath>>,
}

impl Graph {
    pub fn from_cache(cache: &Cache) -> Self {
        let mut forward: BTreeMap<RelPath, Vec<RelPath>> = BTreeMap::new();
        for (path, entry) in cache.iter() {
            let mut deps = entry.imports.clone();
            deps.sort();
            deps.dedup();
            forward.insert(path.clone(), deps);
        }
        let reverse = cache.build_reverse();
        Self { forward, reverse }
    }

    fn neighbours<'a>(&'a self, path: &RelPath, reverse: bool) -> &'a [RelPath] {
        let map = if reverse {
            &self.reverse
        } else {
            &self.forward
        };
        map.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn closure(&self, start: &RelPath, reverse: bool, opts: TraversalOpts) -> Vec<RelPath> {
        let mut seen: BTreeSet<RelPath> = BTreeSet::new();
        let mut stack: Vec<RelPath> = self.neighbours(start, reverse).to_vec();
        while let Some(p) = stack.pop() {
            if !seen.insert(p.clone()) {
                continue;
            }
            for n in self.neighbours(&p, reverse) {
                if !seen.contains(n) {
                    stack.push(n.clone());
                }
            }
        }
        let mut out: Vec<RelPath> = seen
            .into_iter()
            .filter(|p| opts.include_libsonnet || !p.has_libsonnet_extension())
            .collect();
        out.sort();
        out
    }

    pub fn tree(&self, start: &RelPath, reverse: bool, opts: TraversalOpts) -> TreeNode {
        let mut on_path: BTreeSet<RelPath> = BTreeSet::new();
        self.tree_inner(start, reverse, opts, &mut on_path)
    }

    fn tree_inner(
        &self,
        path: &RelPath,
        reverse: bool,
        opts: TraversalOpts,
        on_path: &mut BTreeSet<RelPath>,
    ) -> TreeNode {
        if !on_path.insert(path.clone()) {
            return TreeNode {
                path: path.clone(),
                children: Vec::new(),
                cycle: true,
            };
        }
        let mut children = Vec::new();
        for n in self.neighbours(path, reverse) {
            let child = self.tree_inner(n, reverse, opts, on_path);
            if opts.include_libsonnet || !n.has_libsonnet_extension() {
                children.push(child);
            } else {
                children.extend(child.children);
            }
        }
        on_path.remove(path);
        TreeNode {
            path: path.clone(),
            children,
            cycle: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::FileEntry;

    fn rp(s: &str) -> RelPath {
        RelPath::from_components(&s.split('/').collect::<Vec<_>>())
    }

    fn build(entries: &[(&str, &[&str])]) -> Cache {
        let mut cache = Cache::new();
        for (path, imports) in entries {
            cache.upsert(FileEntry {
                path: rp(path),
                mtime: 0,
                imports: imports.iter().map(|s| rp(s)).collect(),
            });
        }
        cache
    }

    #[test]
    fn forward_closure_chain() {
        let cache = build(&[
            ("a.jsonnet", &["b.libsonnet"]),
            ("b.libsonnet", &["c.libsonnet"]),
            ("c.libsonnet", &[]),
        ]);
        let g = Graph::from_cache(&cache);
        let out = g.closure(
            &rp("a.jsonnet"),
            false,
            TraversalOpts {
                include_libsonnet: true,
            },
        );
        assert_eq!(
            out.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["b.libsonnet", "c.libsonnet"]
        );
    }

    #[test]
    fn forward_closure_diamond_dedups() {
        let cache = build(&[
            ("a.jsonnet", &["b.libsonnet", "c.libsonnet"]),
            ("b.libsonnet", &["d.libsonnet"]),
            ("c.libsonnet", &["d.libsonnet"]),
            ("d.libsonnet", &[]),
        ]);
        let g = Graph::from_cache(&cache);
        let out = g.closure(
            &rp("a.jsonnet"),
            false,
            TraversalOpts {
                include_libsonnet: true,
            },
        );
        assert_eq!(
            out.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["b.libsonnet", "c.libsonnet", "d.libsonnet"]
        );
    }

    #[test]
    fn forward_closure_terminates_on_cycle() {
        let cache = build(&[
            ("a.libsonnet", &["b.libsonnet"]),
            ("b.libsonnet", &["a.libsonnet"]),
        ]);
        let g = Graph::from_cache(&cache);
        let out = g.closure(
            &rp("a.libsonnet"),
            false,
            TraversalOpts {
                include_libsonnet: true,
            },
        );
        assert_eq!(
            out.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["a.libsonnet", "b.libsonnet"]
        );
    }

    #[test]
    fn reverse_closure_mirrors_forward() {
        let cache = build(&[
            ("a.jsonnet", &["b.libsonnet"]),
            ("b.libsonnet", &["c.libsonnet"]),
            ("c.libsonnet", &[]),
            ("other.jsonnet", &["b.libsonnet"]),
        ]);
        let g = Graph::from_cache(&cache);
        let out = g.closure(
            &rp("c.libsonnet"),
            true,
            TraversalOpts {
                include_libsonnet: true,
            },
        );
        assert_eq!(
            out.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["a.jsonnet", "b.libsonnet", "other.jsonnet"]
        );
    }

    #[test]
    fn no_libsonnet_filters_output() {
        let cache = build(&[
            ("a.jsonnet", &["b.libsonnet"]),
            ("b.libsonnet", &["c.jsonnet"]),
            ("c.jsonnet", &[]),
        ]);
        let g = Graph::from_cache(&cache);
        let out = g.closure(
            &rp("a.jsonnet"),
            false,
            TraversalOpts {
                include_libsonnet: false,
            },
        );
        assert_eq!(
            out.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["c.jsonnet"],
            "libsonnet must be filtered but its descendants kept"
        );
    }

    #[test]
    fn tree_marks_cycles() {
        let cache = build(&[
            ("a.libsonnet", &["b.libsonnet"]),
            ("b.libsonnet", &["a.libsonnet"]),
        ]);
        let g = Graph::from_cache(&cache);
        let t = g.tree(
            &rp("a.libsonnet"),
            false,
            TraversalOpts {
                include_libsonnet: true,
            },
        );
        assert!(!t.cycle);
        assert_eq!(t.children.len(), 1);
        assert_eq!(t.children[0].path.as_str(), "b.libsonnet");
        assert_eq!(t.children[0].children.len(), 1);
        assert!(t.children[0].children[0].cycle);
        assert_eq!(t.children[0].children[0].path.as_str(), "a.libsonnet");
    }
}
