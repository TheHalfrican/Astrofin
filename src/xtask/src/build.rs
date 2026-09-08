use crate::{BuildArgs, cef, fs as xfs, mpv, paths, version};
use anyhow::{Context, Result, bail};
use std::process::Command;

pub fn run(args: &BuildArgs) -> Result<()> {
    let out = std::path::absolute(&args.out)?;
    std::fs::create_dir_all(&out)?;

    let cef_info = match &args.cef_path {
        Some(dir) => cef::explicit(dir)?,
        None => cef::discover(&args.external_cef)?,
    };
    println!("Found CEF: {}", cef_info.version);

    let (mpv_info, used_external_mpv) = if let Some(dir) = &args.external_mpv {
        println!("Using external mpv from: {}", dir.display());
        (mpv::external(dir)?, true)
    } else {
        (mpv::build(&out, args.mpv_cli)?, false)
    };

    // Cargo invocation — mirror the env CMake passes today.
    let target_dir = paths::cargo_target_dir(&out);
    #[cfg(target_os = "windows")]
    warn_if_target_dir_too_deep(&target_dir);
    let manifest = paths::workspace_manifest();
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("astrofin")
        .arg("--manifest-path")
        .arg(&manifest);
    if args.no_kde_palette {
        cmd.arg("--no-default-features");
    }
    cmd.env("CARGO_TARGET_DIR", &target_dir);

    let _cef_proxy;
    if cef_info.link_external {
        let (tmp, proxy) = cef::sdk_proxy(&cef_info.root)?;
        _cef_proxy = Some(tmp);
        cmd.env("CEF_PATH", &proxy);
        cmd.env("CEF_RESOURCES_DIR", &cef_info.root);
    } else {
        _cef_proxy = None;
        cmd.env("CEF_PATH", &cef_info.root);
        cmd.env_remove("CEF_RESOURCES_DIR");
    }

    // Single source of truth for the embedded commit hash. xtask always runs
    // (never cargo-cached), so it recomputes every build; the build scripts
    // read these via cargo:rerun-if-env-changed for exact invalidation.
    let (git_hash, git_dirty) = version::git_info();
    cmd.env("JFN_GIT_HASH", git_hash.unwrap_or_default());
    cmd.env("JFN_GIT_DIRTY", if git_dirty { "1" } else { "0" });

    if let Some(dir) = &args.external_mpv {
        cmd.env("EXTERNAL_MPV_DIR", dir);
        cmd.env_remove("JFN_MPV_INCLUDE_DIR");
        cmd.env_remove("JFN_MPV_LIB_DIR");
    } else {
        cmd.env_remove("EXTERNAL_MPV_DIR");
        cmd.env(
            "JFN_MPV_INCLUDE_DIR",
            paths::mpv_source_dir().join("include"),
        );
        cmd.env("JFN_MPV_LIB_DIR", &mpv_info.build_dir);
    }

    // Linux: rpath system / out-of-tree lib dirs into the binary so it
    // resolves DT_NEEDED entries that aren't shipped alongside it.
    // In-tree builds (.cache/cef + meson mpv) stay relocatable —
    // libs are staged next to the binary and $ORIGIN handles them.
    if cfg!(target_os = "linux") {
        let mut rpaths: Vec<String> = Vec::new();
        if cef_info.link_external {
            rpaths.push(cef_info.dir.to_string_lossy().into_owned());
        }
        if let Some(dir) = &args.external_mpv {
            rpaths.push(dir.join("lib").to_string_lossy().into_owned());
        }
        if rpaths.is_empty() {
            cmd.env_remove("JFN_EXTRA_RPATH");
        } else {
            cmd.env("JFN_EXTRA_RPATH", rpaths.join(":"));
        }
    }

    println!("Building astrofin (Rust binary)...");
    let status = cmd.status().context("spawn cargo build")?;
    if !status.success() {
        bail!("cargo build failed");
    }

    let bin_name = if cfg!(target_os = "windows") {
        "astrofin.exe"
    } else {
        "astrofin"
    };
    let bin_src = target_dir.join("release").join(bin_name);
    let bin_dst = out.join(bin_name);
    xfs::copy_file(&bin_src, &bin_dst)?;

    crate::platform::stage_cef(&out, &cef_info)?;
    crate::platform::stage_mpv(&out, &mpv_info, used_external_mpv, &bin_dst)?;
    stage_shaders(&out)?;
    Ok(())
}

/// Longest path cef-dll-sys' bundled CMake/ninja build writes below the cargo
/// target directory (measured on CEF 151.3.24):
///
/// ```text
/// release\build\cef-dll-sys-<16 hex>\out\build\libcef_dll_wrapper\CMakeFiles\
/// libcef_dll_wrapper.dir\ctocpp\test\
/// api_version_test_ref_ptr_library_child_child_v2_ctocpp.cc.obj
/// ```
#[cfg(target_os = "windows")]
const CEF_LONGEST_TARGET_RELPATH: usize = 179;

/// Windows `MAX_PATH` (260) less the terminating NUL.
#[cfg(target_os = "windows")]
const MAX_PATH_CHARS: usize = 259;

/// cl.exe does not honour the machine's `LongPathsEnabled`, so a deep target
/// directory makes the longest ~30 `libcef_dll_wrapper` objects fail with
/// `fatal error C1083: Cannot open compiler generated file: ''` — and ninja
/// reports only `build stopped: subcommand failed`, several hundred lines
/// after the real cause. Say so up front instead.
#[cfg(target_os = "windows")]
fn warn_if_target_dir_too_deep(target_dir: &std::path::Path) {
    let len = target_dir.as_os_str().len();
    let longest = len + 1 + CEF_LONGEST_TARGET_RELPATH;
    if longest > MAX_PATH_CHARS {
        eprintln!(
            "warning: {} is {len} characters deep, so cef-dll-sys' CMake build \
             would write object paths of up to {longest} characters — more than \
             the {MAX_PATH_CHARS} a Windows path can hold. Expect `C1083: Cannot \
             open compiler generated file` from cl.exe. Build through a short \
             junction instead, e.g. `cmd /c mklink /J C:\\astrofin <repo>`.",
            target_dir.display(),
        );
    }
}

/// Copy `resources/shaders/` next to the binary. The runtime resolver
/// (`jfn_paths::resource_dir`) looks for it there on every platform; the macOS
/// installer moves it into `Contents/Resources/` afterwards.
///
/// The destination is cleared first so a shader dropped upstream does not
/// linger in an incremental build and keep resolving.
pub fn stage_shaders(out: &std::path::Path) -> Result<()> {
    let src = paths::shaders_source_dir();
    if !src.is_dir() {
        println!("No bundled shaders at {} — skipping", src.display());
        return Ok(());
    }
    let dst = out.join("shaders");
    if dst.exists() {
        std::fs::remove_dir_all(&dst)
            .with_context(|| format!("remove_dir_all {}", dst.display()))?;
    }
    xfs::copy_dir_recursive(&src, &dst)?;
    Ok(())
}
