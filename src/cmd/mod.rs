pub mod cache;
pub mod list;
pub mod tree;

/// Top-level options shared by every subcommand. Built once in `main` from
/// the parsed CLI flags and passed by reference into each `run` function.
///
/// `cache_file` is optional: when unset the cache is built in memory for the
/// current run only and never written to disk.
#[derive(Debug, Clone)]
pub struct GlobalOpts {
    pub cache_file: Option<std::path::PathBuf>,
    pub root: crate::paths::Root,
    pub silence_dynamic_imports: bool,
    pub walk: WalkOpts,
}

/// Controls how `--all` (explicit on `cache`, or implicit on `tree`/`list`
/// without `--cache-file`) walks the root directory.
///
/// Defaults match `ripgrep`/`fd`: gitignore / `.ignore` / global gitignore are
/// respected and hidden entries are skipped, which keeps generated trees
/// out of the cache automatically.
#[derive(Debug, Clone, Default)]
pub struct WalkOpts {
    pub no_ignore: bool,
    pub no_ignore_vcs: bool,
    pub hidden: bool,
    /// Extra glob patterns to exclude from the scan, gitignore-style.
    /// A leading `!` re-includes a previously excluded entry.
    pub excludes: Vec<String>,
}
