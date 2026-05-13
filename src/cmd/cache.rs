//! `jsondepnet cache` subcommand: scan jsonnet files, extract imports, and
//! persist the result to the cache file (when one is configured).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::cache::{Cache, FileEntry};
use crate::cmd::GlobalOpts;
use crate::extract;
use crate::paths::RelPath;

#[derive(Debug, clap::Args)]
pub struct CacheArgs {
    /// Specific files to (re)cache. Mutually exclusive with --all.
    #[arg(conflicts_with = "all")]
    pub files: Vec<PathBuf>,

    /// Scan every *.jsonnet and *.libsonnet file under --root.
    #[arg(long)]
    pub all: bool,

    /// Discard the existing cache and write only the newly-collected entries.
    #[arg(long)]
    pub replace: bool,

    /// Print each enumerated file to stderr as it's processed.
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run(global: &GlobalOpts, args: &CacheArgs) -> anyhow::Result<()> {
    if !args.all && args.files.is_empty() {
        anyhow::bail!("expected at least one file or --all");
    }
    if global.cache_file.is_none() {
        anyhow::bail!(
            "`cache` subcommand requires --cache-file (or JSONDEPNET_CACHE_FILE) to persist results"
        );
    }
    let targets = resolve_targets(global, &args.files, args.all)?;
    let cache = build_or_update_cache(
        global,
        &targets,
        UpdateOpts {
            replace: args.replace,
            prune_missing: args.all,
            verbose: args.verbose,
        },
    )?;
    if let Some(path) = &global.cache_file {
        cache.save_atomic(path)?;
    }
    Ok(())
}

/// Resolve `--all` or positional file inputs into the set of absolute paths to
/// (re-)cache. Exposed for `tree`/`list` to share the same logic.
pub fn resolve_targets(
    global: &GlobalOpts,
    files: &[PathBuf],
    all: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    if all {
        let mut builder = ignore::WalkBuilder::new(global.root.as_path());
        let w = &global.walk;
        let respect_ignore = !w.no_ignore;
        let respect_vcs = respect_ignore && !w.no_ignore_vcs;
        builder
            .hidden(!w.hidden)
            .ignore(respect_ignore)
            .parents(respect_ignore)
            .git_ignore(respect_vcs)
            .git_global(respect_vcs)
            .git_exclude(respect_vcs)
            .follow_links(false);
        if !w.excludes.is_empty() {
            let mut overrides = ignore::overrides::OverrideBuilder::new(global.root.as_path());
            for pat in &w.excludes {
                // gitignore semantics: a leading `!` re-includes. Our default
                // (no `!`) is an exclusion, so flip the polarity for the
                // `OverrideBuilder`, whose default is *inclusion*.
                let flipped = if let Some(rest) = pat.strip_prefix('!') {
                    rest.to_owned()
                } else {
                    format!("!{pat}")
                };
                overrides.add(&flipped)?;
            }
            builder.overrides(overrides.build()?);
        }
        let mut out = Vec::new();
        for entry in builder.build() {
            let entry = entry?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "jsonnet" || ext == "libsonnet" {
                out.push(path.canonicalize()?);
            }
        }
        Ok(out)
    } else {
        let mut out = Vec::with_capacity(files.len());
        for f in files {
            out.push(global.root.canonicalize_input(f)?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateOpts {
    /// Start from an empty cache instead of loading the existing one.
    pub replace: bool,
    /// After updating, drop cache entries whose source file no longer exists.
    /// Only meaningful when `targets` was a full root sweep.
    pub prune_missing: bool,
    /// Log every enumerated file to stderr, including whether it was
    /// extracted afresh or served from the cache by matching mtime.
    pub verbose: bool,
}

/// Build (or update) the dependency cache for `targets` and return it.
/// Loading the existing cache from disk and writing the result back are
/// concerns of the caller; this function performs no I/O on the cache file.
pub fn build_or_update_cache(
    global: &GlobalOpts,
    targets: &[PathBuf],
    opts: UpdateOpts,
) -> anyhow::Result<Cache> {
    let mut cache = if opts.replace {
        Cache::new()
    } else if let Some(path) = &global.cache_file {
        Cache::load_or_default(path)?
    } else {
        Cache::new()
    };

    let resolver = jrsonnet_evaluator::FileImportResolver::new(Vec::new());
    let target_rels: BTreeSet<RelPath> = targets
        .iter()
        .map(|p| global.root.relativise(p))
        .collect::<Result<_, _>>()?;

    for abs in targets {
        let rel = global.root.relativise(abs)?;
        let mtime = mtime_of(abs)?;
        if !opts.replace
            && let Some(existing) = cache.get(&rel)
            && existing.mtime == mtime
        {
            if opts.verbose {
                eprintln!("jsondepnet: cached {rel}");
            }
            continue;
        }
        if opts.verbose {
            eprintln!("jsondepnet: extract {rel}");
        }
        let res = match extract::extract_imports(&resolver, abs) {
            Ok(r) => r,
            Err(e) => {
                if !global.silence_dynamic_imports {
                    eprintln!("jsondepnet: warning: {rel}: failed to extract imports: {e}");
                }
                continue;
            }
        };
        if !global.silence_dynamic_imports {
            for u in &res.unresolved {
                eprintln!(
                    "jsondepnet: warning: {rel}: cannot resolve import '{}': {}",
                    u.raw, u.reason
                );
            }
        }
        let mut imports = Vec::with_capacity(res.resolved.len());
        for imp in res.resolved {
            match global.root.relativise(&imp) {
                Ok(r) => imports.push(r),
                Err(e) => {
                    if !global.silence_dynamic_imports {
                        eprintln!(
                            "jsondepnet: warning: {rel}: import resolved outside of root: {e}"
                        );
                    }
                }
            }
        }
        imports.sort();
        imports.dedup();
        cache.upsert(FileEntry {
            path: rel,
            mtime,
            imports,
        });
    }

    if opts.prune_missing && !opts.replace {
        let to_drop: Vec<RelPath> = cache
            .paths()
            .filter(|p| !target_rels.contains(*p) && !global.root.absolutise(p).exists())
            .cloned()
            .collect();
        for p in to_drop {
            cache.remove(&p);
        }
    }

    Ok(cache)
}

fn mtime_of(p: &Path) -> anyhow::Result<i64> {
    let meta = std::fs::metadata(p)?;
    let mtime = meta.modified()?;
    let dur = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|e| -(e.duration().as_secs() as i64));
    Ok(dur)
}
