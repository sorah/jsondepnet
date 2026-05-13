# jsondepnet: Jsonnet dependency tree

`jsonnet-deps` (`jrsonnet-deps`) provides a list of dependencies for a single Jsonnet file. However we often need to lookup reverse dependencies, for instance, when a pull request has changes to certain libsonnet files, and we want to know which jsonnet files have to be re-evaluated. jsondepnet provides a cacheable dependency tree for multiple jsonnet files, and keep them tracked.

## Usage

```
export JSONDEPNET_CACHE_FILE=tmp/jsondepnet_cache.json # or specify via --cache-file flag
export JSONDEPNET_ROOT_DIR=path/to/dir # or specify via --root flag. Defaults to current working directory. This is to calculate relative paths in cache file. Command line arguments will still be given in relative from cwd.

jsondepnet cache --all # All *.jsonnet files
jsondepnet cache path/to/some.jsonnet path/to/some.libsonnet # Specific *.jsonnet files
jsondepnet cache --replace path/to/other.libsonnet # ditto, but replace the entire cache

# Both commands support:
# - `--reverse` (`-r`) flag to get reverse dependencies instead of forward dependencies.
# - `--no-libsonnet` (`-L`) flag to hide libsonnet files from the output.
# - `--skip-update-cache` flag to skip updating the cache file. Otherwise it implicitly runs `jsondepnet cache FILES...` internally to update the cache.
# - `--json` (`-j`) flag to output in JSON format instead of plain text.
# - `--null` (`-0`) flag to separate output with null character instead of newlines, for better handling of file paths with special characters.
jsondepnet tree path/to/some.jsonnet ... # Get the dependency tree for given files
jsondepnet list path/to/some.jsonnet ... # Get the list of dependencies for given files
```

## Caveats

- If `import` argument uses a dynamical expression, such dependencies will not be tracked. Conditional dependencies are okay, as we track `import` statements by statically parsing the AST.

## License

MIT License
