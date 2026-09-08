# jellium-desktop — Windows build environment notes

Machine: The-Halfrican (Win 11 Pro 25H2, i9-14900K, RTX 4090)
Repo: `C:\Users\NoahM\Documents\RustProjects\jellium-desktop` @ 28f2cf1 (fork of andrewrabert/jellium-desktop)
Date: 2026-09-07. `just` is NOT installed; every step below calls the scripts / cargo directly.

## Shell prerequisites (every new shell)

Rust is installed but not on PATH:

```bash
export PATH="$HOME/.cargo/bin:$PATH"      # bash
```
```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"   # PowerShell
```

PowerShell 7 is the Store build, NOT at `C:\Program Files\PowerShell\7`:

```
C:\Users\NoahM\AppData\Local\Microsoft\WindowsApps\pwsh.exe
```

## Step 1 — libmpv (MSYS2 CLANG64 + meson)

```
pwsh -ExecutionPolicy Bypass -File dev\windows\build_mpv_source.ps1 -Arch x64
```

Wall time: **3m03s** (13:08:26 → 13:11:29). No elevation needed — the script
downloaded the msys2-base sfx and extracted it to `C:\msys64` as a normal user.

Artifacts (`third_party\mpv-install`, 173 MB total):

| file | size |
|---|---|
| `lib\mpv.lib` | 13,578 B (54 exports, via dumpbin + lib.exe) |
| `lib\libmpv-2.dll` | 14,676,480 B |
| `lib\avcodec.lib` | 43,196 B |
| `lib\*.dll` | 121 files (msys2 runtime closure) |
| `include\` | `mpv`, `libavcodec`, `libavutil` |

Versions pulled by pacman:

```
mpv          0.41.0-UNKNOWN (submodule fork)
ffmpeg       9.0.1-3   -> avcodec-63 / avutil-61 / avformat-63 / avfilter-12
libplacebo   7.360.1-2
libass       0.17.5-1
meson        1.12.0-1
clang/llvm   22.1.8-2  -> libLLVM-22.dll
shaderc      2026.3-1
```

## Step 2 — CEF

```
cargo xtask fetch-cef
```

Wall time: **62s** (13:08:54 → 13:09:56). Runs fine WITHOUT vcvars — rustc finds
`link.exe` on its own.
Result: `.cache\cef\151.3.16\cef_windows_x86_64` (592 MB including the `.tar.bz2`).
CEF 151.3.16 + chromium 151.0.7922.109; the crate reports `151.3.0+151.3.16`.
(Since then the pin moved to CEF 151.3.24: the crate reports `151.8.1+151.3.24`
and the cache holds both versions.)

## Step 3 — app build

```
pwsh -ExecutionPolicy Bypass -File dev\windows\build.ps1
```

Wall time: **1m11s** (13:19:47 → 13:20:58) — dependencies had already been
compiled by the two earlier failed attempts; a true from-scratch cargo build is
roughly 4–5 min.
`build.ps1` dot-sources `dev\windows\env.ps1` (vcvars64 + LIBCLANG_PATH +
EXTERNAL_MPV_DIR) then runs
`cargo xtask build --external-mpv=<repo>\third_party\mpv-install`.

Artifacts in `build\` (572 MB excluding `build\cargo-target`):

| file | size |
|---|---|
| `jellium-desktop.exe` | 6,730,240 B |
| `libcef.dll` | 285,340,672 B |
| `libmpv-2.dll` | 14,676,480 B |

plus 129 DLLs total, `resources.pak`, `chrome_100_percent.pak`,
`chrome_200_percent.pak`, `icudtl.dat`, `v8_context_snapshot.bin`, `locales\`,
`vk_swiftshader_icd.json`, `archive.json`.
`build\cargo-target` is a further ~2.2 GB.

## Step 4 — smoke test: PASSED

Launched `build\jellium-desktop.exe` with cwd = `build\`. Six processes (CEF is
multi-process), main process 353 MB working set, still alive after 12 s.
`%LOCALAPPDATA%\jellium-desktop\Logs\jellium-desktop.log` rotated and the new
session contains:

```
2026-09-07T13:21:35 INFO [Main] jellium-desktop 0.1.0-dev+28f2cf1-dirty
2026-09-07T13:21:35 INFO [Main] [FLOW] calling CefInitialize...
2026-09-07T13:21:35 INFO [Main] [FLOW] CefInitialize returned ok in 82 ms
2026-09-07T13:21:35 INFO [Main] mpv window ready in 104 ms
2026-09-07T13:21:35 INFO [Main] mpv-version mpv v0.41.0-UNKNOWN
2026-09-07T13:21:35 INFO [Main] ffmpeg-version 9.0.1
```

It reached the Jellyfin server at `192.168.50.76:8096` and loaded the web UI.
Zero `ERROR` lines in the session.

Note: `$proc.CloseMainWindow()` does NOT terminate it (CEF owns the top-level
window); `Stop-Process -Force` was required. Nothing was left running afterwards.

## Step 5 — installers (NSIS setup.exe + WiX MSI)

`dev\windows\package.ps1` wraps whatever `build.ps1` staged in `build\` into two
per-user installers. It does not build anything itself; run it after step 3.

```
pwsh -NoProfile -ExecutionPolicy Bypass -File dev\windows\package.ps1
just package                      # same thing, after `just build`
```

Outputs, in `dist\`:

```
Astrofin-<version>-x64-setup.exe   NSIS 3.12, RequestExecutionLevel user
Astrofin-<version>-x64.msi         WiX 5, Scope=perUser (ALLUSERS=2 + MSIINSTALLPERUSER=1)
```

Both install to `%LOCALAPPDATA%\Programs\Astrofin`, write only HKCU, never
prompt for UAC, and never touch `%APPDATA%\astrofin` / `%LOCALAPPDATA%\astrofin`
— the profile survives uninstall by design, and is not even offered as an
option. Sources: `dev\windows\installer\astrofin.nsi` and `astrofin.wxs`; the
script passes the payload dir, version strings and output path in as defines.

Tooling (user scope, no admin):

- **NSIS** — `dev\windows\fetch-nsis.ps1` downloads the portable
  `nsis-3.12.zip` from SourceForge (needs a non-browser User-Agent, otherwise
  SourceForge serves its "your download will start shortly" page), verifies the
  SHA-256 and unpacks it to `.cache\nsis\nsis-3.12\makensis.exe`. `.cache` is
  the same directory CI junction-links to the persistent runner cache, so this
  is a one-off.
- **WiX 5** — `dotnet tool install --global wix --version 5.*` plus
  `wix extension add --global WixToolset.UI.wixext/5.0.2`. `package.ps1` finds
  `wix` on PATH or at `%USERPROFILE%\.dotnet\tools\wix.exe`.

**Version.** The display version is read from the staged `astrofin.exe`'s
VERSIONINFO (`ProductVersion`, written by `src/jfn_rust/build.rs`), so the
installer names exactly the binary it contains, git suffix included:
`0.1.0-dev+818271b`, `-dirty` and all. Windows Installer's `ProductVersion` has
to be numeric `a.b.c` (a,b ≤ 255, c ≤ 65535), so the MSI gets the leading
`0.1.0` and keeps the full string in the file name and the ARP DisplayVersion.
NSIS gets `a.b.c.0` for `VIProductVersion` and the full string everywhere else.

**Timings** (571 MB / 357-file payload, i9-14900K), as measured here:

```
makensis /SOLID zlib     54 s     (-Compressor zlib; the local-iteration setting)
wix build, low cabs       7 s     (-MsiCompression low)
makensis /SOLID lzma    171 s     (default; measured with a CI job competing for the CPU)
wix build, high cabs     44 s     (default; same caveat)
```

The defaults (`lzma` / `high`) are worth it for anything you keep: from
d18712c the zlib/low pair was 238 MB each, the lzma/high pair 174 MB (NSIS)
and 219 MB (MSI) — 27 % and 8 % smaller. Use zlib/low only while iterating on
the installer scripts. `-SkipPayloadRefresh` reuses
`build\installer-payload` from the previous run instead of robocopy-mirroring
`build\` again; `-Only nsis|msi` builds one of the two.

**Add/Remove Programs for a per-user MSI** does *not* live under
`HKCU\...\CurrentVersion\Uninstall\{ProductCode}` — Windows Installer keeps it in
`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Installer\UserData\<SID>\Products\<hash>\InstallProperties`,
and the product only shows up through `Installer.ProductsEx(..., context=2)`,
not the legacy `Installer.Products` collection. Looking in the HKCU key and
concluding the entry is missing is an easy mistake to make; it is there.
The NSIS build writes the conventional `HKCU\...\Uninstall\Astrofin` key.

**Coexistence.** Both packages use the same directory, so each one removes the
other before installing:

- `astrofin.nsi` `.onInit` → `MsiEnumRelatedProductsW` on the MSI's
  UpgradeCode (`7d2f4c61-…`, kept in sync by hand) → `msiexec /x {ProductCode}
  /qn`, then the previous NSIS install's own `uninstall.exe /S _?=<dir>`.
- `astrofin.wxs` → a `RegistrySearch` for the NSIS key's `InstallLocation` and a
  deferred, impersonated `RemoveNsisInstall` custom action right after
  `InstallInitialize` that runs `uninstall.exe /S _?=<dir>`; a `RemoveFile`
  drops the uninstaller it cannot delete itself, and a `RemoveFolder` covers
  the directory Windows Installer then does not consider its own.
- MSI over MSI is `MajorUpgrade` (`AllowSameVersionUpgrades`), NSIS over NSIS
  is the `_?=` path above. Verified matrix (all silent, 2026-09-07): MSI `/x`;
  NSIS `/S` install → shortcut carries `System.AppUserModel.ID =
  io.github.thehalfrican.Astrofin`, HKCU uninstall key present; NSIS over NSIS
  (a planted stale file was removed); `uninstall.exe /S` leaves nothing; MSI
  `/qn` → ARP entry with `InstallLocation`, same AUMID on the shortcut; NSIS
  over MSI → MSI registration gone; MSI over NSIS → NSIS key and
  `uninstall.exe` gone; final `msiexec /x` → nothing left, profile dirs
  byte-count unchanged throughout.

**Code signing** is a hook only: `-SignTool <signtool.exe>` (or
`ASTROFIN_SIGNTOOL`) plus `-SignArgs` / `ASTROFIN_SIGNTOOL_ARGS` signs
`astrofin.exe` before packaging and both installers afterwards. With nothing
set the script prints `signing: skipped` and carries on — there is no
certificate for this project yet, so SmartScreen will warn on first run.

Silent-install switches, for the record: NSIS `/S [/DESKTOP=1]`; MSI
`msiexec /i <msi> /qn [INSTALLDESKTOPSHORTCUT=1]`, or an elevated
`ALLUSERS=1 MSIINSTALLPERUSER=""` for a per-machine install into
`%ProgramFiles%\Astrofin`.

## Gitea CI (self-hosted act_runner on this machine)

`.gitea/workflows/build-windows.yml` runs on the host-mode act_runner
(`C:\Users\NoahM\act_runner\config.yaml`, label `windows-latest`). The job
junction-links `.cache\`, `build\` and `third_party\mpv-install` to a persistent
cache under `C:\Users\NoahM\act_runner\cache\astrofin`, so a normal run should be
an incremental cargo build. Two bugs kept it from ever getting there; both are
fixed on `fix/gitea-ci`.

### 1. "Build and package" died on MAX_PATH (the actual red step)

act_runner's host-mode workspace is

```
C:\Users\NoahM\act_runner\workspaces\<16 hex>\hostexecutor        66 chars
```

and `cef-dll-sys` runs its own CMake/ninja build of `libcef_dll_wrapper` under
the cargo target dir. The longest object it writes is 198 characters below the
repo root:

```
build\cargo-target\release\build\cef-dll-sys-<hash>\out\build\libcef_dll_wrapper\
CMakeFiles\libcef_dll_wrapper.dir\ctocpp\test\
api_version_test_ref_ptr_library_child_child_v2_ctocpp.cc.obj
```

66 + 1 + 198 = **265 > MAX_PATH (260)**. `cl.exe` does not honour this machine's
`LongPathsEnabled=1`, and `-DCMAKE_OBJECT_PATH_MAX=500` (which cef-dll-sys does
pass) only silences CMake's own check. Eight objects — every
`ctocpp/test/*_child_child_*` file — fail with

```
... api_version_test_ref_ptr_library_child_child_v2_ctocpp.cc : fatal error C1083:
Cannot open compiler generated file: '': Invalid argument
ninja: build stopped: subcommand failed.
```

and the build script panics in `cmake-0.1.58`, surfacing only as
`Error: cargo build failed`.

Why the earlier steps were green: `cargo clippy`/`cargo test` write to
`src\target\debug`, ten characters shorter than `build\cargo-target\release`, so
the same files land at 255 and compile. The boundary is that tight — the longest
object that *did* compile in the failing step measures 258.

Fix: the workflow makes a short junction to the workspace
(`SHORT_WORKSPACE: C:\astrofin-ci`) in the linking step and every later step does
`Set-Location $env:SHORT_WORKSPACE` first. Nothing is copied; only the paths
handed to CMake get shorter (same trick as the worktree recipe below). It also
stabilises the cargo fingerprints, which are keyed on absolute source paths.
`cargo xtask build` additionally prints a warning up front when the target dir is
deep enough for this to happen, rather than letting it surface as C1083 several
hundred lines into ninja output.

### 2. libmpv was rebuilt on every run and never cached

`dev\windows\build_mpv_source.ps1` did

```powershell
if (Test-Path $OutputDir) { Remove-Item -Recurse -Force $OutputDir }
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
```

On PowerShell 7 `Remove-Item -Recurse -Force` on a junction removes the *link*
only (it does not follow it — the cache contents were never at risk), so this
replaced the `third_party\mpv-install` junction with a real directory inside the
disposable job workspace. `…\cache\astrofin\mpv-install-<mpv sha>` therefore
stayed empty, the "skipped when the cache exists" guard never fired, and every
run paid ~1–3 min rebuilding libmpv into a workspace that act deletes afterwards.

Fix: clear the directory's *contents* when it already exists and only create it
when it does not. `dev\windows\build.ps1 -Clean` had the same shape against
`build\` and was hardened the same way.

### Not a problem (checked)

Version derivation does **not** use `git describe`: `src/xtask/src/version.rs`
and `src/jfn_rust/build.rs` both read HEAD through `gix` and fall back to the
bare Cargo version, so `actions/checkout`'s shallow clone with no tags is fine
and `fetch-depth: 0` is not needed.

### Other CI notes

- Removing a path that might be a junction: use
  `[IO.Directory]::Delete($path, $false)` on a reparse point and address it with
  `Join-Path`, never `Resolve-Path`/`Convert-Path` (those resolve to the target).
- `package.ps1` mirrors `build\` into `build\installer-payload`, which under CI
  is inside the persistent cache — expect ~1.5 GB of payload/cab scratch to live
  there alongside `cargo-target`.

## Script edit made — `dev\windows\env.ps1` (REVIEW / UPSTREAM CANDIDATE)

Symptom — `cargo xtask build` failed in `jfn-mpv`'s `build.rs` with:

```
Unable to find libclang: "the `libclang` shared library at
C:\msys64\clang64\bin\libclang.dll could not be opened: LoadLibraryExW failed"
```

…even though `LIBCLANG_PATH` was correct and `C:\msys64\clang64\bin` was on PATH.

Root cause: bindgen → clang-sys → libloading calls
`LoadLibraryExW(path, NULL, 0)`, so `libclang.dll`'s own mingw dependencies
(`libLLVM-22`, `libc++`, `libxml2-16`, `libzstd`, `zlib1`, `libiconv-2`,
`libffi-8`) are resolved through PATH — first match wins. `env.ps1` **appended**
`C:\msys64\clang64\bin`, and this machine has

```
%LOCALAPPDATA%\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_...\mingw64\bin
```

earlier on PATH. Bisected down to a single file: that directory's
`libxml2-16.dll` alone reproduces the failure (its own dependencies cannot be
resolved → ERROR_MOD_NOT_FOUND / 126 reported against `libclang.dll`). Git's
`mingw64\bin` and the Vulkan SDK are harmless. CI does not hit this because a
fresh GitHub runner has no competing mingw toolchain.

Fix — one line plus an explanatory comment, in the `LIBCLANG_PATH` block:

```diff
-        $env:PATH = "$env:PATH;$MsysBin"
+        $env:PATH = "$MsysBin;$env:PATH"
```

No other repo file was touched; `git status` shows only `M dev/windows/env.ps1`.
Nothing was committed or pushed. The original file is saved beside this note as
`env.ps1.orig`.

The same reasoning applies to `.github/workflows/build-windows.yml`
(`set "PATH=%PATH%;%MSYS_BIN%"`) should it ever run on a dirtier machine.

## Incremental rebuild after editing Rust sources

From the repo root, with cargo on PATH:

```
pwsh -ExecutionPolicy Bypass -File dev\windows\build.ps1
```

Measured at **3 s** for a no-op rebuild; it reuses `build\cargo-target` and
re-stages `build\`. Equivalent raw command if you are already inside a shell
that has vcvars64 + `LIBCLANG_PATH` + `C:\msys64\clang64\bin` at the FRONT of
PATH:

```
cargo xtask build --external-mpv=third_party\mpv-install
```

Other flags:

- `dev\windows\build.ps1 -Clean` — wipe `build\` first (also drops `cargo-target`,
  so the next build is fully cold)
- `dev\windows\build_mpv_source.ps1 -Force` — rebuild libmpv; without `-Force` it
  no-ops when `third_party\mpv-install\lib\mpv.lib` exists

## Building from a `.claude/worktrees/` agent worktree (MAX_PATH)

`cef-dll-sys`'s CMake/ninja step compiles `libcef_dll/cpptoc/test/*.cc` into
object paths under
`build/cargo-target/release/build/cef-dll-sys-<hash>/out/build/…`. From the
main tree those fit; from a worktree at `.claude/worktrees/<name>/` the extra
~29 characters push the longest of them past 260 and `cl.exe` fails with

```
fatal error C1083: Cannot open compiler generated file: '': Invalid argument
```

for exactly the handful of longest file names. Ninja only reports
`build stopped: subcommand failed`, with the real cause several hundred lines
earlier, so this is easy to misread as a toolchain problem.

Fix: build through a short junction rather than enabling long paths (`cl.exe`
does not reliably honour them).

```powershell
cmd /c mklink /J C:\avm "C:\Users\NoahM\Documents\RustProjects\Astrofin\.claude\worktrees\<name>"
pwsh -NoProfile -ExecutionPolicy Bypass -File C:\avm\dev\windows\build.ps1
```

No elevation needed. Everything still lands in the real worktree (`build/`,
`build/cargo-target/`); only the paths handed to CMake are short. Remove with
`rmdir C:\avm` — deleting a junction never touches its target.

## Warnings / quirks worth knowing

- `vcvars64.bat` from VS 2026 Build Tools v18.7.3 (MSVC 14.51.36231) prints
  `'vswhere.exe' is not recognized as an internal or external command` while
  loading. Harmless — `VSINSTALLDIR`/`INCLUDE`/`LIB` are still set correctly and
  the whole workspace compiles.
- `dev\windows\setup.ps1` still targets `Microsoft.VisualStudio.2022.BuildTools`
  and tells you to open the "x64 Native Tools Command Prompt for VS 2022". The
  installed 2026 Build Tools satisfy its vswhere check, so `setup.ps1` was never
  needed here.
- The user's PowerShell profile runs fastfetch, so every `pwsh -File ...`
  invocation without `-NoProfile` prepends ASCII art to the build log.
- `cmd.exe` on this machine has an AutoRun that also runs fastfetch — avoid
  `cmd /c` for anything whose stdout you need to parse.
- MSYS2 was freshly installed at `C:\msys64` by `build_mpv_source.ps1`
  (~374 MB downloaded, ~2.6 GB installed). The first `bash -lc` afterwards prints
  the one-time "MSYS2 is starting for the first time" setup banner.
- `build\` and the user's installed copy in
  `%LOCALAPPDATA%\Programs\Jellium Desktop` share `%APPDATA%\jellium-desktop`
  (config) and `%LOCALAPPDATA%\jellium-desktop` (logs). Never run both at once.
  Nothing under `%APPDATA%\jellium-desktop` was modified.
- The version string embeds git state: `0.1.0-dev+28f2cf1-dirty` — dirty only
  because of the `env.ps1` edit above.

## Disk consumed

```
C:\msys64                    ~2.6 GB
.cache\cef                    592 MB
third_party\mpv-install       173 MB
third_party\mpv\build         (meson build tree)
build\                        2.8 GB (572 MB staged + ~2.2 GB cargo-target)
```
