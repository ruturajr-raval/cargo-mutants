// Copyright 2021-2026 Martin Pool

//! A directory containing mutated source to run cargo builds and tests.

#![warn(clippy::pedantic)]

use std::{
    fs::{symlink_metadata, write},
    process::{Command, Stdio},
};

use anyhow::{Context, bail};
use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;
use tracing::info;

use crate::{
    Result,
    console::Console,
    copy_tree::copy_tree,
    manifest::{fix_cargo_config, fix_manifest},
    options::Options,
    workspace::Workspace,
};

/// A directory containing source, that can be mutated, built, and tested.
///
/// Depending on how its constructed, this might be a copy in a tempdir
/// or the original source directory.
#[derive(Debug)]
pub struct BuildDir {
    /// The path of the root of the build directory.
    path: Utf8PathBuf,
    /// Holds a reference to the temporary directory, so that it will be deleted when this
    /// object is dropped. If None, there's nothing to clean up.
    #[allow(dead_code)]
    temp_dir: Option<TempDir>,
}

impl BuildDir {
    /// Make the build dir for the baseline.
    ///
    /// Depending on the options, this might be either a copy of the source directory
    /// or in-place.
    pub fn for_baseline(
        workspace: &Workspace,
        options: &Options,
        console: &Console,
    ) -> Result<BuildDir> {
        if options.in_place {
            BuildDir::in_place(workspace.root())
        } else {
            BuildDir::copy_from(workspace.root(), options, console)
        }
    }

    /// Make a new build dir, copying from a source directory, subject to exclusions.
    pub fn copy_from(source: &Utf8Path, options: &Options, console: &Console) -> Result<BuildDir> {
        // TODO: This would be nicer with cfg_match, but we support MSRV 1.88 and it's stable in 1.95. Update this later.
        #[cfg(windows)]
        let name_base = "mutants-".to_string(); // Avoid the 260-character path limit.
        #[cfg(not(windows))]
        let name_base = if let Some(name) = source.file_name() {
            format!("cargo-mutants-{name}-")
        } else {
            "cargo-mutants-".to_string()
        };
        let source_abs = source
            .canonicalize_utf8()
            .context("canonicalize source path")?;
        let temp_dir = copy_tree(source, &name_base, options, console)?;
        let path: Utf8PathBuf = temp_dir
            .path()
            .to_owned()
            .try_into()
            .context("tempdir path to UTF-8")?;
        fix_manifest(&path.join("Cargo.toml"), &source_abs)?;
        fix_cargo_config(&path, &source_abs)?;
        run_prepare_command(&path, &options.prepare_command)?;
        let temp_dir = if options.leak_dirs {
            let _ = temp_dir.keep();
            info!(?path, "Build directory will be leaked for inspection");
            None
        } else {
            Some(temp_dir)
        };
        let build_dir = BuildDir { path, temp_dir };
        Ok(build_dir)
    }

    /// Make a build dir that works in-place on the source directory.
    pub fn in_place(source_path: &Utf8Path) -> Result<BuildDir> {
        Ok(BuildDir {
            temp_dir: None,
            path: source_path
                .canonicalize_utf8()
                .context("canonicalize source path")?,
        })
    }

    pub fn path(&self) -> &Utf8Path {
        self.path.as_path()
    }

    pub fn overwrite_file(&self, relative_path: &Utf8Path, code: &str) -> Result<()> {
        let full_path = self.path.join(relative_path);
        match symlink_metadata(&full_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("{full_path:?} is a symlink, refusing to overwrite it")
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                bail!(
                    "{full_path:?} is not a regular file (type is {:?}), refusing to overwrite it",
                    metadata.file_type()
                );
            }
            Ok(_) => write(&full_path, code.as_bytes())
                .with_context(|| format!("failed to overwrite {full_path:?}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                bail!("{full_path:?} does not exist, refusing to create it")
            }
            Err(e) => bail!("failed to stat {full_path:?}: {e}"),
        }
    }
}

fn run_prepare_command(path: &Utf8Path, command: &[String]) -> Result<()> {
    let Some((program, arguments)) = command.split_first() else {
        return Ok(());
    };
    let status = Command::new(program)
        .args(arguments)
        .current_dir(path)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("run prepare command {command:?} in {path}"))?;
    if !status.success() {
        bail!("prepare command {command:?} failed in {path} with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use std::fs::create_dir;

    use crate::test_util::copy_of_testdata;

    use super::*;

    #[test]
    fn build_dir_copy_from() {
        let tmp = copy_of_testdata("factorial");
        let workspace = Workspace::open(tmp.path()).unwrap();
        let options = Options {
            in_place: false,
            gitignore: true,
            leak_dirs: false,
            ..Default::default()
        };
        let build_dir = BuildDir::copy_from(workspace.root(), &options, &Console::new()).unwrap();
        let debug_form = format!("{build_dir:?}");
        println!("debug form is {debug_form:?}");
        assert!(debug_form.starts_with("BuildDir { path: "));
        assert!(build_dir.path().is_dir());
        assert!(build_dir.path().join("Cargo.toml").is_file());
        assert!(build_dir.path().join("src").is_dir());
    }

    #[test]
    fn build_dir_copy_from_runs_prepare_command_in_copy() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        std::fs::write(tmp.path().join("prepare.rs"), "pub fn prepared() {}")?;
        let workspace = Workspace::open(tmp.path())?;
        let options = Options {
            prepare_command: rustc_prepare_command("prepare.rs", "prepared.rmeta"),
            ..Default::default()
        };

        let build_dir = BuildDir::copy_from(workspace.root(), &options, &Console::new())?;

        assert!(build_dir.path().join("prepared.rmeta").is_file());
        assert!(!workspace.root().join("prepared.rmeta").exists());
        Ok(())
    }

    #[test]
    fn build_dir_copy_from_reports_prepare_command_failure() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        std::fs::write(tmp.path().join("invalid.rs"), "not valid Rust")?;
        let workspace = Workspace::open(tmp.path())?;
        let options = Options {
            prepare_command: rustc_prepare_command("invalid.rs", "prepared.rmeta"),
            ..Default::default()
        };

        let err = BuildDir::copy_from(workspace.root(), &options, &Console::new()).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("prepare command"));
        assert!(message.contains("invalid.rs"));
        assert!(message.contains("failed in"));
        Ok(())
    }

    #[test]
    fn build_dir_copy_from_reports_prepare_command_spawn_failure() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        let workspace = Workspace::open(tmp.path())?;
        let options = Options {
            prepare_command: vec!["cargo-mutants-command-that-does-not-exist".to_owned()],
            ..Default::default()
        };

        let err = BuildDir::copy_from(workspace.root(), &options, &Console::new()).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("cargo-mutants-command-that-does-not-exist"));
        assert!(message.contains("run prepare command"));
        Ok(())
    }

    #[test]
    fn for_baseline_in_place_skips_prepare_command() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        let workspace = Workspace::open(tmp.path())?;
        let options = Options {
            in_place: true,
            prepare_command: vec!["cargo-mutants-command-that-does-not-exist".to_owned()],
            ..Default::default()
        };
        let build_dir = BuildDir::for_baseline(&workspace, &options, &Console::new())?;
        assert_eq!(
            build_dir.path().canonicalize_utf8()?,
            workspace.root().canonicalize_utf8()?
        );
        assert!(build_dir.temp_dir.is_none());
        Ok(())
    }

    fn rustc_prepare_command(source: &str, output: &str) -> Vec<String> {
        vec![
            std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned()),
            source.to_owned(),
            "--crate-type=lib".to_owned(),
            "--emit=metadata".to_owned(),
            "-o".to_owned(),
            output.to_owned(),
        ]
    }

    #[test]
    fn for_baseline_copied() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        let workspace = Workspace::open(tmp.path())?;
        let options = Options {
            in_place: false,
            ..Default::default()
        };
        let build_dir = BuildDir::for_baseline(&workspace, &options, &Console::new())?;
        assert!(build_dir.path().is_dir());
        assert!(build_dir.path().join("Cargo.toml").is_file());
        assert!(build_dir.path().join("src").is_dir());
        assert!(build_dir.temp_dir.is_some());
        assert_ne!(
            build_dir.path().canonicalize_utf8()?,
            workspace.root().canonicalize_utf8()?
        );
        Ok(())
    }

    #[test]
    fn build_dir_in_place() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        let workspace = Workspace::open(tmp.path())?;
        let build_dir = BuildDir::in_place(workspace.root())?;
        // On Windows e.g. the paths might not have the same form, but they
        // should point to the same place.
        assert_eq!(
            build_dir.path().canonicalize_utf8()?,
            workspace.root().canonicalize_utf8()?
        );
        Ok(())
    }

    /// This shouldn't happen unless we're confused about which files to mutate, but let's make sure
    /// we give a clear error.
    #[test]
    fn fail_to_overwrite_dir() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        let tmp_path: &Utf8Path = tmp.path().try_into().unwrap();
        let build_dir = BuildDir::in_place(tmp_path)?;
        create_dir(tmp.path().join("foo"))?;

        let err = build_dir
            .overwrite_file(Utf8Path::new("foo"), "code")
            .expect_err("expected overwrite_file to fail when the destination is a dir");
        println!("error message is {err:?}");
        assert!(
            err.to_string().contains("is not a regular file"),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    /// This shouldn't normally happen, but if the destination contains a symlink, we shouldn't overwrite it because
    /// it might be pointing outside the scratch directory, and we don't want to mess with the user's files.
    #[test]
    #[cfg(unix)]
    fn fail_to_overwrite_symlink() -> Result<()> {
        use std::os::unix::fs::symlink;

        let tmp = copy_of_testdata("factorial");
        let tmp_path: &Utf8Path = tmp.path().try_into().unwrap();
        let build_dir = BuildDir::in_place(tmp_path)?;
        symlink("foo", tmp.path().join("foo"))?;

        let err = build_dir
            .overwrite_file(Utf8Path::new("foo"), "code")
            .expect_err("expected overwrite_file to fail when the destination is a symlink");
        println!("error message is {err:?}");
        assert!(
            err.to_string()
                .contains("is a symlink, refusing to overwrite it"),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    /// We only ever overwrite existing files in the build dir, and if they don't existing then
    /// something surprising has happened.
    ///
    /// (We could relax this if we ever expect to create new files, but we don't need to today.)
    #[test]
    fn dont_create_new_files() -> Result<()> {
        let tmp = copy_of_testdata("factorial");
        let tmp_path: &Utf8Path = tmp.path().try_into().unwrap();
        let build_dir = BuildDir::in_place(tmp_path)?;

        let err = build_dir
            .overwrite_file(Utf8Path::new("foo"), "code")
            .expect_err("expected overwrite_file to fail when the destination does not exist");
        println!("error message is {err:?}");
        assert!(
            err.to_string().contains("does not exist"),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    /// Returns false when the current process can bypass Unix DAC permission checks
    /// (e.g. running as root or with a privilege like Illumos `PRIV_FILE_DAC_WRITE`).
    /// Tests that depend on mode-0 restrictions making writes fail must skip in that case.
    #[cfg(unix)]
    fn unix_dac_permissions_enforced() -> bool {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe");
        std::fs::write(&path, b"test").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::write(&path, b"x").is_err()
    }

    /// Test reporting of generic failures to write into the build dir, by making them unwriteable.
    #[test]
    #[cfg(unix)]
    fn fail_to_overwrite() -> Result<()> {
        use std::{fs::set_permissions, os::unix::fs::PermissionsExt};

        if !unix_dac_permissions_enforced() {
            // Running with privileges that bypass file permissions (e.g. root); skip.
            return Ok(());
        }

        let tmp = copy_of_testdata("factorial");
        let tmp_path: &Utf8Path = tmp.path().try_into().unwrap();
        let build_dir = BuildDir::in_place(tmp_path)?;
        let relpath = Utf8Path::new("src/bin/factorial.rs");
        set_permissions(
            tmp.path().join(relpath),
            std::fs::Permissions::from_mode(0o000),
        )?;

        let err = build_dir.overwrite_file(relpath, "code").unwrap_err();
        println!("error message is {err:?}");
        assert!(
            err.to_string().contains("failed to overwrite"),
            "unexpected error message: {err}"
        );
        Ok(())
    }

    /// An edge case: can't even stat the destination file.
    #[test]
    #[cfg(unix)]
    fn fail_to_overwrite_dir_permission_denied() -> Result<()> {
        use std::{fs::set_permissions, os::unix::fs::PermissionsExt};

        if !unix_dac_permissions_enforced() {
            // Running with privileges that bypass file permissions (e.g. root); skip.
            return Ok(());
        }

        let tmp = copy_of_testdata("factorial");
        let tmp_path: &Utf8Path = tmp.path().try_into().unwrap();
        let build_dir = BuildDir::in_place(tmp_path)?;
        let relpath = Utf8Path::new("src/bin/factorial.rs");
        set_permissions(
            tmp.path().join(relpath.parent().unwrap()),
            std::fs::Permissions::from_mode(0o000),
        )?;

        let err = build_dir.overwrite_file(relpath, "code").unwrap_err();
        println!("error message is {err:?}");
        assert!(
            err.to_string().contains("failed to stat"),
            "unexpected error message: {err}"
        );
        Ok(())
    }
}
