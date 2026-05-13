//! Output formatting for `tree` / `list` subcommands.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::graph::TreeNode;
use crate::paths::{PathStyle, RelPath, Root};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    TextNewline,
    TextNul,
    Json,
}

/// Renders an internal [`RelPath`] (always root-relative) into the user-facing
/// string according to the requested [`PathStyle`]. The renderer owns the
/// canonical `root` and `cwd` so the formatting layer is free of `std::env`.
pub struct PathRenderer<'a> {
    root: &'a Root,
    cwd: PathBuf,
    style: PathStyle,
}

impl<'a> PathRenderer<'a> {
    pub fn new(root: &'a Root, cwd: impl AsRef<Path>, style: PathStyle) -> Self {
        Self {
            root,
            cwd: cwd.as_ref().to_path_buf(),
            style,
        }
    }

    pub fn render(&self, rel: &RelPath) -> String {
        let abs = self.root.absolutise(rel);
        self.root
            .render(&abs, self.style, &self.cwd)
            .unwrap_or_else(|_| abs.display().to_string())
    }
}

#[derive(Debug, serde::Serialize)]
struct ListOutput {
    roots: Vec<ListRoot>,
}

#[derive(Debug, serde::Serialize)]
struct ListRoot {
    path: String,
    deps: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
struct TreeOutput {
    roots: Vec<TreeJson>,
}

#[derive(Debug, serde::Serialize)]
struct TreeJson {
    path: String,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    cycle: bool,
    children: Vec<TreeJson>,
}

fn tree_to_json(node: &TreeNode, renderer: &PathRenderer<'_>) -> TreeJson {
    TreeJson {
        path: renderer.render(&node.path),
        cycle: node.cycle,
        children: node
            .children
            .iter()
            .map(|c| tree_to_json(c, renderer))
            .collect(),
    }
}

/// Emit a flat dependency list per root.
pub fn write_list<W: Write>(
    w: &mut W,
    items: &[(RelPath, Vec<RelPath>)],
    renderer: &PathRenderer<'_>,
    fmt: OutputFormat,
) -> std::io::Result<()> {
    match fmt {
        OutputFormat::Json => {
            let out = ListOutput {
                roots: items
                    .iter()
                    .map(|(root, deps)| ListRoot {
                        path: renderer.render(root),
                        deps: deps.iter().map(|d| renderer.render(d)).collect(),
                    })
                    .collect(),
            };
            serde_json::to_writer_pretty(&mut *w, &out)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            w.write_all(b"\n")?;
        }
        OutputFormat::TextNewline | OutputFormat::TextNul => {
            let sep: u8 = if matches!(fmt, OutputFormat::TextNul) {
                0
            } else {
                b'\n'
            };
            for (_, deps) in items {
                for d in deps {
                    w.write_all(renderer.render(d).as_bytes())?;
                    w.write_all(&[sep])?;
                }
            }
        }
    }
    Ok(())
}

/// Emit a tree per root.
pub fn write_tree<W: Write>(
    w: &mut W,
    roots: &[TreeNode],
    renderer: &PathRenderer<'_>,
    fmt: OutputFormat,
) -> std::io::Result<()> {
    match fmt {
        OutputFormat::Json => {
            let out = TreeOutput {
                roots: roots.iter().map(|r| tree_to_json(r, renderer)).collect(),
            };
            serde_json::to_writer_pretty(&mut *w, &out)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            w.write_all(b"\n")?;
        }
        OutputFormat::TextNewline => {
            for r in roots {
                write_tree_text(w, r, renderer, 0)?;
            }
        }
        OutputFormat::TextNul => {
            // CLI parsing rejects this combination; treat as bug if reached.
            return Err(std::io::Error::other(
                "tree output does not support NUL separator",
            ));
        }
    }
    Ok(())
}

fn write_tree_text<W: Write>(
    w: &mut W,
    node: &TreeNode,
    renderer: &PathRenderer<'_>,
    depth: usize,
) -> std::io::Result<()> {
    for _ in 0..depth {
        w.write_all(b"  ")?;
    }
    w.write_all(renderer.render(&node.path).as_bytes())?;
    if node.cycle {
        w.write_all(b" (cycle)")?;
    }
    w.write_all(b"\n")?;
    for c in &node.children {
        write_tree_text(w, c, renderer, depth + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rp(s: &str) -> RelPath {
        RelPath::from_components(&s.split('/').collect::<Vec<_>>())
    }

    fn setup_renderer(tmp: &tempfile::TempDir) -> (Root, PathBuf) {
        let root = Root::new(tmp.path()).unwrap();
        let cwd = root.as_path().to_path_buf();
        (root, cwd)
    }

    #[test]
    fn list_newline_separated() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, cwd) = setup_renderer(&tmp);
        let renderer = PathRenderer::new(&root, &cwd, PathStyle::Root);
        let items = vec![(rp("a.jsonnet"), vec![rp("b.libsonnet"), rp("c.libsonnet")])];
        let mut buf = Vec::new();
        write_list(&mut buf, &items, &renderer, OutputFormat::TextNewline).unwrap();
        assert_eq!(buf, b"b.libsonnet\nc.libsonnet\n");
    }

    #[test]
    fn list_null_separated() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, cwd) = setup_renderer(&tmp);
        let renderer = PathRenderer::new(&root, &cwd, PathStyle::Root);
        let items = vec![(rp("a.jsonnet"), vec![rp("b.libsonnet")])];
        let mut buf = Vec::new();
        write_list(&mut buf, &items, &renderer, OutputFormat::TextNul).unwrap();
        assert_eq!(buf, b"b.libsonnet\0");
    }

    #[test]
    fn list_json_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, cwd) = setup_renderer(&tmp);
        let renderer = PathRenderer::new(&root, &cwd, PathStyle::Root);
        let items = vec![(rp("a.jsonnet"), vec![rp("b.libsonnet")])];
        let mut buf = Vec::new();
        write_list(&mut buf, &items, &renderer, OutputFormat::Json).unwrap();

        #[derive(serde::Deserialize)]
        struct R {
            roots: Vec<RootEntry>,
        }
        #[derive(serde::Deserialize)]
        struct RootEntry {
            path: String,
            deps: Vec<String>,
        }
        let parsed: R = serde_json::from_slice(&buf).unwrap();
        assert_eq!(parsed.roots.len(), 1);
        assert_eq!(parsed.roots[0].path, "a.jsonnet");
        assert_eq!(parsed.roots[0].deps, vec!["b.libsonnet"]);
    }

    #[test]
    fn tree_text_indentation() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, cwd) = setup_renderer(&tmp);
        let renderer = PathRenderer::new(&root, &cwd, PathStyle::Root);
        let tree = TreeNode {
            path: rp("a.jsonnet"),
            cycle: false,
            children: vec![TreeNode {
                path: rp("b.libsonnet"),
                cycle: false,
                children: vec![TreeNode {
                    path: rp("c.libsonnet"),
                    cycle: true,
                    children: vec![],
                }],
            }],
        };
        let mut buf = Vec::new();
        write_tree(&mut buf, &[tree], &renderer, OutputFormat::TextNewline).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "a.jsonnet\n  b.libsonnet\n    c.libsonnet (cycle)\n");
    }

    #[test]
    fn tree_text_nul_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, cwd) = setup_renderer(&tmp);
        let renderer = PathRenderer::new(&root, &cwd, PathStyle::Root);
        let tree = TreeNode {
            path: rp("a.jsonnet"),
            cycle: false,
            children: vec![],
        };
        let mut buf = Vec::new();
        let res = write_tree(&mut buf, &[tree], &renderer, OutputFormat::TextNul);
        assert!(res.is_err());
    }
}
