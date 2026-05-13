use std::path::PathBuf;

#[derive(clap::Parser)]
#[command(author, version, about = "Jsonnet dependency tree with on-disk cache")]
struct Cli {
    /// Path to the cache JSON file (must be provided either way).
    #[arg(long, env = "JSONDEPNET_CACHE_FILE", global = true)]
    cache_file: Option<PathBuf>,

    /// Project root used for relativising cached paths.
    /// Defaults to the current working directory.
    #[arg(long, env = "JSONDEPNET_ROOT_DIR", global = true)]
    root: Option<PathBuf>,

    /// Silence per-file warnings about imports that could not be resolved
    /// (typical for dynamic imports).
    #[arg(long, global = true)]
    silence_dynamic_imports: bool,

    /// Don't read any ignore files (.gitignore, .ignore, etc.) when scanning
    /// the root directory. Affects `cache --all` and implicit `--all` runs
    /// of `tree`/`list` without a persistent cache file.
    #[arg(long, global = true)]
    no_ignore: bool,

    /// Don't read VCS ignore files (.gitignore, global gitignore,
    /// .git/info/exclude), but still respect `.ignore` files.
    #[arg(long, global = true)]
    no_ignore_vcs: bool,

    /// Include hidden files and directories (e.g. `.dotfiles`) in the scan.
    #[arg(long, global = true)]
    hidden: bool,

    /// Exclude paths matching this gitignore-style glob from the scan.
    /// Pass repeatedly for multiple patterns; prefix with `!` to re-include.
    #[arg(long = "exclude", value_name = "GLOB", global = true)]
    excludes: Vec<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Update the dependency cache for the given files (or --all).
    Cache(jsondepnet::cmd::cache::CacheArgs),
    /// Print a flat list of dependencies (or reverse dependencies).
    List(jsondepnet::cmd::list::ListArgs),
    /// Print a nested dependency tree (or reverse tree).
    Tree(jsondepnet::cmd::tree::TreeArgs),
}

fn main() -> anyhow::Result<()> {
    use clap::Parser as _;
    let cli = Cli::parse();

    let cache_file = cli.cache_file;
    let root_path = match cli.root {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    let root = jsondepnet::paths::Root::new(&root_path)?;

    let global = jsondepnet::cmd::GlobalOpts {
        cache_file,
        root,
        silence_dynamic_imports: cli.silence_dynamic_imports,
        walk: jsondepnet::cmd::WalkOpts {
            no_ignore: cli.no_ignore,
            no_ignore_vcs: cli.no_ignore_vcs,
            hidden: cli.hidden,
            excludes: cli.excludes,
        },
    };

    match &cli.command {
        Commands::Cache(args) => jsondepnet::cmd::cache::run(&global, args),
        Commands::List(args) => jsondepnet::cmd::list::run(&global, args),
        Commands::Tree(args) => jsondepnet::cmd::tree::run(&global, args),
    }
}
