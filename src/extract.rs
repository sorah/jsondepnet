//! Static import extraction.
//!
//! Two layers:
//! - [`parse_imports`] is pure: takes source code as a string, returns the
//!   raw import strings discovered by the AST visitor. Unit-tested with
//!   inline fixtures.
//! - [`extract_imports`] is the I/O wrapper: reads from disk, parses, then
//!   resolves each raw import to an absolute path via `FileImportResolver`.

use std::path::{Path, PathBuf};

use jrsonnet_evaluator::ImportResolver;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImport {
    pub raw: String,
    /// `true` for `import` (the imported file is evaluated), `false` for
    /// `importstr`/`importbin` (treated as raw data). Both still represent a
    /// real static file dependency, so we keep both in the cache.
    pub is_code: bool,
}

#[derive(Debug)]
pub struct ExtractResult {
    pub resolved: Vec<PathBuf>,
    pub unresolved: Vec<UnresolvedImport>,
}

#[derive(Debug)]
pub struct UnresolvedImport {
    pub raw: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("failed to read {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path:?}: {message}")]
    Parse { path: PathBuf, message: String },
}

struct FoundImports(Vec<RawImport>);

impl jrsonnet_ir::visit::Visitor for FoundImports {
    fn visit_import(&mut self, as_expression: bool, value: jrsonnet_ir::IStr) {
        self.0.push(RawImport {
            raw: value.to_string(),
            is_code: as_expression,
        });
    }
}

/// Parse jsonnet source code and return the static imports it contains.
///
/// `source_name` is only used for parser error messages (e.g. a path or
/// `<test>`); no I/O is performed.
pub fn parse_imports(source_name: &str, code: &str) -> Result<Vec<RawImport>, String> {
    let code_istr: jrsonnet_ir::IStr = code.into();
    let source = jrsonnet_ir::Source::new_virtual(source_name.into(), code_istr.clone());
    let settings = jrsonnet_ir_parser::ParserSettings { source };
    let parsed = jrsonnet_ir_parser::parse(&code_istr, &settings).map_err(|e| format!("{e}"))?;

    use jrsonnet_ir::visit::Visitor as _;
    let mut found = FoundImports(Vec::new());
    found.visit_expr(&parsed);
    Ok(found.0)
}

/// Resolve a single file's static imports to absolute paths.
///
/// Does NOT recurse; the cache layer composes the dependency tree from
/// per-file extract results. Unresolved imports are recorded as data, not
/// logged — the caller decides whether to warn.
pub fn extract_imports(
    resolver: &dyn ImportResolver,
    abs_path: &Path,
) -> Result<ExtractResult, ExtractError> {
    let canonical = abs_path.canonicalize().map_err(|source| ExtractError::Io {
        path: abs_path.to_path_buf(),
        source,
    })?;
    let source_path = jrsonnet_ir::SourcePath::new(jrsonnet_ir::SourceFile::new(canonical.clone()));
    let bytes = resolver
        .load_file_contents(&source_path)
        .map_err(|e| ExtractError::Io {
            path: canonical.clone(),
            source: std::io::Error::other(format!("{e}")),
        })?;
    let code = std::str::from_utf8(&bytes).map_err(|e| ExtractError::Parse {
        path: canonical.clone(),
        message: format!("invalid utf-8: {e}"),
    })?;

    let raw = parse_imports(&canonical.display().to_string(), code).map_err(|message| {
        ExtractError::Parse {
            path: canonical.clone(),
            message,
        }
    })?;

    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for r in raw {
        match resolver.resolve_from(&source_path, &r.raw.as_str()) {
            Ok(p) => {
                if let Some(sf) = p.downcast_ref::<jrsonnet_ir::SourceFile>() {
                    resolved.push(sf.path().to_path_buf());
                } else {
                    unresolved.push(UnresolvedImport {
                        raw: r.raw,
                        reason: format!("import is not a regular file: {p}"),
                    });
                }
            }
            Err(e) => {
                unresolved.push(UnresolvedImport {
                    raw: r.raw,
                    reason: format!("{e}"),
                });
            }
        }
    }
    Ok(ExtractResult {
        resolved,
        unresolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raws(s: &[RawImport]) -> Vec<&str> {
        s.iter().map(|r| r.raw.as_str()).collect()
    }

    #[test]
    fn empty_object_has_no_imports() {
        let r = parse_imports("<t>", "{}").unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn picks_up_plain_import() {
        let r = parse_imports("<t>", "import 'foo.libsonnet'").unwrap();
        assert_eq!(raws(&r), vec!["foo.libsonnet"]);
        assert!(r[0].is_code);
    }

    #[test]
    fn picks_up_importstr_and_importbin() {
        let code = "{ a: importstr 'x.txt', b: importbin 'y.bin' }";
        let r = parse_imports("<t>", code).unwrap();
        let mut got: Vec<_> = r.iter().map(|x| (x.raw.clone(), x.is_code)).collect();
        got.sort();
        assert_eq!(
            got,
            vec![("x.txt".to_string(), false), ("y.bin".to_string(), false),]
        );
    }

    #[test]
    fn multiple_imports_in_expression() {
        let code = "(import 'a.libsonnet') + (import 'b.libsonnet')";
        let r = parse_imports("<t>", code).unwrap();
        let mut names = raws(&r);
        names.sort();
        assert_eq!(names, vec!["a.libsonnet", "b.libsonnet"]);
    }

    #[test]
    fn dynamic_import_not_picked_up() {
        let code = "local x = 'foo.libsonnet'; import x";
        let r = parse_imports("<t>", code).unwrap();
        assert!(
            r.is_empty(),
            "dynamic import expressions must not produce static imports, got {r:?}"
        );
    }

    #[test]
    fn conditional_imports_both_tracked() {
        let code = "if true then import 'a.libsonnet' else import 'b.libsonnet'";
        let r = parse_imports("<t>", code).unwrap();
        let mut names = raws(&r);
        names.sort();
        assert_eq!(names, vec!["a.libsonnet", "b.libsonnet"]);
    }

    #[test]
    fn extract_imports_resolves_existing_file_and_records_missing() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("a.jsonnet"),
            "(import 'b.libsonnet') + (import 'missing.libsonnet')",
        )
        .unwrap();
        std::fs::write(tmp.path().join("b.libsonnet"), "{}").unwrap();

        let resolver = jrsonnet_evaluator::FileImportResolver::new(Vec::new());
        let result = extract_imports(&resolver, &tmp.path().join("a.jsonnet")).unwrap();

        assert_eq!(result.resolved.len(), 1);
        assert_eq!(
            result.resolved[0].canonicalize().unwrap(),
            tmp.path().join("b.libsonnet").canonicalize().unwrap()
        );
        assert_eq!(result.unresolved.len(), 1);
        assert_eq!(result.unresolved[0].raw, "missing.libsonnet");
    }
}
