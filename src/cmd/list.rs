//! `jsondepnet list` subcommand: flattened forward/reverse dependency list.

use std::path::PathBuf;

use crate::cache::Cache;
use crate::cmd::GlobalOpts;
use crate::cmd::cache::UpdateOpts;
use crate::graph::{Graph, TraversalOpts};
use crate::output::{OutputFormat, PathRenderer};
use crate::paths::PathStyle;

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,

    #[arg(short = 'r', long)]
    pub reverse: bool,

    #[arg(short = 'L', long)]
    pub no_libsonnet: bool,

    #[arg(long)]
    pub skip_update_cache: bool,

    #[arg(short = 'j', long, conflicts_with = "null")]
    pub json: bool,

    #[arg(short = '0', long = "null", conflicts_with = "json")]
    pub null: bool,

    #[arg(long, value_enum, default_value_t = PathStyle::Root)]
    pub path_style: PathStyle,
}

pub fn run(global: &GlobalOpts, args: &ListArgs) -> anyhow::Result<()> {
    if args.skip_update_cache && global.cache_file.is_none() {
        anyhow::bail!(
            "--skip-update-cache requires --cache-file (no persistent cache to read from)"
        );
    }
    let query_targets = crate::cmd::cache::resolve_targets(global, &args.files, false)?;
    let cache = if args.skip_update_cache {
        Cache::load_or_default(
            global
                .cache_file
                .as_ref()
                .expect("checked above that cache_file is Some"),
        )?
    } else {
        // Without a persistent cache, sweep the whole root so indirect
        // dependencies are visible. With one, only refresh the queried files
        // and trust prior `cache --all` runs for the rest.
        let sweep_all = global.cache_file.is_none();
        let update_targets = if sweep_all {
            crate::cmd::cache::resolve_targets(global, &[], true)?
        } else {
            query_targets.clone()
        };
        let cache = crate::cmd::cache::build_or_update_cache(
            global,
            &update_targets,
            UpdateOpts {
                replace: false,
                prune_missing: sweep_all,
                verbose: false,
            },
        )?;
        if let Some(path) = &global.cache_file {
            cache.save_atomic(path)?;
        }
        cache
    };
    let graph = Graph::from_cache(&cache);

    let opts = TraversalOpts {
        include_libsonnet: !args.no_libsonnet,
    };
    let mut items = Vec::with_capacity(query_targets.len());
    for abs in &query_targets {
        let rel = global.root.relativise(abs)?;
        let deps = graph.closure(&rel, args.reverse, opts);
        items.push((rel, deps));
    }

    let cwd = std::env::current_dir()?;
    let renderer = PathRenderer::new(&global.root, &cwd, args.path_style);
    let fmt = if args.json {
        OutputFormat::Json
    } else if args.null {
        OutputFormat::TextNul
    } else {
        OutputFormat::TextNewline
    };
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    crate::output::write_list(&mut handle, &items, &renderer, fmt)?;
    Ok(())
}
