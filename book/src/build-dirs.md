# Copying the tree

By default, cargo-mutants copies your tree to a temporary directory before mutating and building it. This behavior is turned of by the [`--in-place`](in-place.md) option, which builds mutated code in the original source directory.

When the [`--jobs`](parallelism.md) option is used, one build directory is created per job.

Some filters are applied while copying the tree, which can be configured by options.

## Preparing copied trees

If a project generates build inputs outside `build.rs`, configure a command to run after each build directory is copied:

```toml
prepare_command = ["cargo", "xtask", "generate"]
```

The array contains the executable followed by its arguments; no shell is invoked. The command runs with the copied workspace root as its working directory, after relative manifest and Cargo configuration paths are fixed, and a spawn or nonzero-exit failure stops the run. It runs once for each copied directory, including each parallel job's copy; parallel commands may run concurrently and are not timeout-controlled. It does not run with `--in-place`, listing commands, or when no mutants match.

## Troubleshooting tree copies

If the baseline tests fail in the copied directory it is a good first debugging step to try building with `--in-place`.

## `.git` and other version control directories

By default, files or directories matching these patterns are not copied, because they can be large and typically are not needed to build the source:

    .git
    .hg
    .jj
    .bzr
    .svn
    _darcs
    .pijul

If your tree's build or tests require the VCS directory then it can be copied with `--copy-vcs=true` or by setting `copy_vcs = true` in `.cargo/mutants.toml`.

## `.gitignore`

The `--gitignore=true` command line option or `gitignore = true` in `.cargo/mutants.toml` enables gitignore filtering, meaning that files matching gitignore patterns will be excluded from copying from the source tree to the build directory.

This option will make copying slightly faster (and use less temporary space) if your tree contains a large number of ignored files that aren't needed to build the source for mutation testing.

gitignore filtering is only used within trees containing a `.git` directory.

The filter, based on the [`ignore` crate](https://docs.rs/ignore/), also respects global git ignore configuration in the home directory, as well as `.gitignore` files within the tree.

The `target/` directory is excluded by default, regardless of gitignore settings, to avoid copying large build artifacts that are typically not needed for mutation testing. This can be overridden with `--copy-target=true` if your tests depend on existing build artifacts, or by setting `copy_target = true` in `.cargo/mutants.toml`.

Note that if you set `--gitignore=true` and `--copy-target=true` and your `target/` is excluded by gitignore files (which is common) then it will not be copied.

The default for gitignore filtering is off. Prior to cargo-mutants 25.0.2, `gitignore` was on by default.

## `mutants.out`

`mutants.out` and `mutants.out.old` are never copied, even if they're not covered by `.gitignore`.
