//! Path helpers: canonicalisation, relativisation, and the [`RelPath`] newtype
//! that represents a path always taken relative to the configured root.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("path is outside of root: {0:?}")]
    OutsideRoot(PathBuf),
    #[error("io error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A path stored in the cache or used internally.
/// Always relative to the configured root, components separated by `/`
/// regardless of the host OS, so cache files survive moving between OSes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(String);

impl RelPath {
    pub fn from_components(parts: &[&str]) -> Self {
        Self(parts.join("/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    /// Convert into a host PathBuf using the platform path separator.
    pub fn to_path_buf(&self) -> PathBuf {
        if std::path::MAIN_SEPARATOR == '/' {
            PathBuf::from(&self.0)
        } else {
            self.0.split('/').collect()
        }
    }

    pub fn has_libsonnet_extension(&self) -> bool {
        self.0
            .rsplit('/')
            .next()
            .map(|name| name.ends_with(".libsonnet"))
            .unwrap_or(false)
    }
}

impl std::fmt::Display for RelPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for RelPath {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RelPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self(s))
    }
}

/// The configured root directory. Always canonicalised when constructed via [`Root::new`].
#[derive(Debug, Clone)]
pub struct Root {
    abs: PathBuf,
}

impl Root {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, PathError> {
        let p = path.as_ref();
        let abs = p.canonicalize().map_err(|source| PathError::Io {
            path: p.to_path_buf(),
            source,
        })?;
        Ok(Self { abs })
    }

    pub fn as_path(&self) -> &Path {
        &self.abs
    }

    /// Resolve a user-provided path (taken relative to cwd) to an absolute,
    /// canonicalised path.
    pub fn canonicalize_input(&self, input: impl AsRef<Path>) -> Result<PathBuf, PathError> {
        let p = input.as_ref();
        let target = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| PathError::Io {
                    path: p.to_path_buf(),
                    source,
                })?
                .join(p)
        };
        target.canonicalize().map_err(|source| PathError::Io {
            path: p.to_path_buf(),
            source,
        })
    }

    /// Make an absolute path relative to root. Fails if it points outside root.
    pub fn relativise(&self, abs: impl AsRef<Path>) -> Result<RelPath, PathError> {
        let abs = abs.as_ref();
        let stripped = abs
            .strip_prefix(&self.abs)
            .map_err(|_| PathError::OutsideRoot(abs.to_path_buf()))?;
        Ok(RelPath(normalise_to_forward_slashes(stripped)))
    }

    /// Inverse of `relativise`: join with root.
    pub fn absolutise(&self, rel: &RelPath) -> PathBuf {
        self.abs.join(rel.to_path_buf())
    }

    /// Render an absolute path according to the requested style.
    /// Used by the output layer; we accept an absolute path because the
    /// caller has already done `absolutise(&RelPath)`.
    pub fn render(&self, abs: &Path, style: PathStyle, cwd: &Path) -> Result<String, PathError> {
        match style {
            PathStyle::Root => Ok(normalise_to_forward_slashes(
                abs.strip_prefix(&self.abs)
                    .map_err(|_| PathError::OutsideRoot(abs.to_path_buf()))?,
            )),
            PathStyle::Cwd => Ok(pathdiff_to_string(cwd, abs)),
            PathStyle::Absolute => Ok(abs.display().to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lower")]
pub enum PathStyle {
    /// Paths relative to `--root`.
    Root,
    /// Paths relative to the current working directory at invocation time.
    Cwd,
    /// Absolute (canonicalised) paths.
    Absolute,
}

fn normalise_to_forward_slashes(p: &Path) -> String {
    let mut buf = String::new();
    let mut first = true;
    for c in p.components() {
        let s: &str = match c {
            Component::Normal(s) => s.to_str().unwrap_or(""),
            Component::ParentDir => "..",
            Component::CurDir => continue,
            Component::RootDir => "",
            Component::Prefix(_) => continue,
        };
        if !first {
            buf.push('/');
        }
        buf.push_str(s);
        first = false;
    }
    buf
}

/// Compute a path expressed relative to `base`, falling back to absolute when
/// the two paths don't share a prefix. Operates purely on the lexical
/// representation (both inputs should already be canonicalised by the caller).
fn pathdiff_to_string(base: &Path, target: &Path) -> String {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();

    let mut i = 0;
    while i < base_components.len()
        && i < target_components.len()
        && base_components[i] == target_components[i]
    {
        i += 1;
    }

    let up = base_components.len() - i;
    let mut parts: Vec<&str> = std::iter::repeat_n("..", up).collect();
    for c in &target_components[i..] {
        if let Component::Normal(s) = c
            && let Some(s) = s.to_str()
        {
            parts.push(s);
        }
    }

    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_libsonnet_detection() {
        assert!(RelPath::from_components(&["a", "b.libsonnet"]).has_libsonnet_extension());
        assert!(!RelPath::from_components(&["a", "b.jsonnet"]).has_libsonnet_extension());
        assert!(!RelPath::from_components(&["libsonnet"]).has_libsonnet_extension());
    }

    #[test]
    fn rel_path_serde_roundtrip() {
        let original = RelPath::from_components(&["foo", "bar.libsonnet"]);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "\"foo/bar.libsonnet\"");
        let back: RelPath = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn normalise_strips_curdir_uses_forward_slash() {
        let p = Path::new("a").join("b").join("c.libsonnet");
        assert_eq!(normalise_to_forward_slashes(&p), "a/b/c.libsonnet");
    }

    #[test]
    fn pathdiff_within_root() {
        let base = Path::new("/a/b");
        let target = Path::new("/a/b/c/d.jsonnet");
        assert_eq!(pathdiff_to_string(base, target), "c/d.jsonnet");
    }

    #[test]
    fn pathdiff_sibling() {
        let base = Path::new("/a/b/c");
        let target = Path::new("/a/b/d/e.jsonnet");
        assert_eq!(pathdiff_to_string(base, target), "../d/e.jsonnet");
    }

    #[test]
    fn pathdiff_equal() {
        let base = Path::new("/a/b");
        let target = Path::new("/a/b");
        assert_eq!(pathdiff_to_string(base, target), ".");
    }

    #[test]
    fn root_relativise_and_back() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/x.jsonnet"), "{}").unwrap();
        let root = Root::new(tmp.path()).unwrap();
        let abs = tmp.path().join("sub/x.jsonnet").canonicalize().unwrap();
        let rel = root.relativise(&abs).unwrap();
        assert_eq!(rel.as_str(), "sub/x.jsonnet");
        assert_eq!(root.absolutise(&rel), root.as_path().join("sub/x.jsonnet"));
    }

    #[test]
    fn root_relativise_outside_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Root::new(tmp.path()).unwrap();
        let outside = Path::new("/").join("etc");
        let err = root.relativise(&outside).unwrap_err();
        assert!(matches!(err, PathError::OutsideRoot(_)));
    }

    #[test]
    fn render_styles() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.jsonnet"), "{}").unwrap();
        let root = Root::new(tmp.path()).unwrap();
        let abs = tmp.path().join("a.jsonnet").canonicalize().unwrap();
        let cwd = root.as_path().to_path_buf();
        assert_eq!(
            root.render(&abs, PathStyle::Root, &cwd).unwrap(),
            "a.jsonnet"
        );
        assert_eq!(
            root.render(&abs, PathStyle::Cwd, &cwd).unwrap(),
            "a.jsonnet"
        );
        assert_eq!(
            root.render(&abs, PathStyle::Absolute, &cwd).unwrap(),
            abs.display().to_string()
        );
    }
}
