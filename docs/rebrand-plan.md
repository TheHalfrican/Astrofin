# Astrofin rebrand — mechanical rename checklist

> **Status: implemented** on branch `feat/rebrand-astrofin` — §6d steps 1-5 landed as
> `ce433b5` (paths + migration), `193cc84` (binary name), `288240a` (runtime identity),
> `c5846d3` (resources + packaging) and the commit that adds this note (CI, README, NOTICE,
> About-panel attribution). Of §6d step 6 the ObjC/window-class hygiene renames, the memfd
> name and the AppUserModelID hardening are **done** on branch `chore/hygiene-renames`;
> artwork pixels landed as fe9b7f1 (`feat/ui-assets`). Everything is merged to `main`; see §6b.

Repo surveyed: `C:\Users\NoahM\Documents\RustProjects\jellium-desktop` (read-only survey, nothing modified).
Search scope: everything except `third_party/`, `.cache/`, `target/`, `build/`, `dist/`, `.git/`.
Raw hit count: **141 occurrences of `jellium|nullsum` (case-insensitive) across 49 files**, plus
`JellyfinCefInput` / `Jellyfin*` ObjC class names, `jmp-*` CSS/DOM prefixes, and `andrewrabert/*` URLs.
Every one of them is listed below.

## Target values (from the decisions handed down)

| Concept | Old | New |
| --- | --- | --- |
| Product / display name | `Jellium Desktop` / `Jellium` | `Astrofin` |
| Binary | `jellium-desktop[.exe]` | `astrofin[.exe]` |
| Per-user dir | `jellium-desktop` | `astrofin` |
| Log file | `jellium-desktop.log` | `astrofin.log` |
| Reverse-DNS id | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` |
| Env-var prefix | `JELLIUM_DESKTOP_*` | `ASTROFIN_*` |
| UA product token | `jellium-desktop/` , `JelliumDesktop/` | `Astrofin/` |
| MPRIS bus | `org.mpris.MediaPlayer2.JelliumDesktop` | `org.mpris.MediaPlayer2.Astrofin` |
| macOS app dir | `Jellium Desktop.app` | `Astrofin.app` |
| Artifact stem | `JelliumDesktop-` | `Astrofin-` |
| Repo URLs | `andrewrabert/jellium-desktop` | `TheHalfrican/Astrofin` |
| Internal crates (`jfn-*` / `jfn_*`) | — | **KEEP** (upstream-merge friction) |

---

# 1. Exhaustive occurrence table

Categories: **UV** = user-visible string · **FS** = filesystem path/dir name · **IPC** = IPC/instance/window-class/bus identifier · **APPID** = reverse-DNS app id · **PKG** = packaging/CI · **DOC** = docs/license/comment · **CODE** = code identifier that merely contains the word.

## 1a. `src/paths` — the root of all filesystem identity

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `src/paths/src/lib.rs` | 17 | `const APP_DIR_NAME: &str = "jellium-desktop";` | FS | `"astrofin"` |
| `src/paths/src/lib.rs` | 18 | `const LOG_FILE_NAME: &str = "jellium-desktop.log";` | FS | `"astrofin.log"` |

`APP_DIR_NAME` is consumed by `imp_linux.rs:23`, `imp_macos.rs:17`, `imp_windows.rs:19`, and by
`lib.rs:94` (config), `:101` (cache), `:120` (`/tmp/{APP_DIR_NAME}-{uid}` runtime dir),
`:155` (`\\.\pipe\{APP_DIR_NAME}-{id}`). All follow the constant — no per-site edits needed.

## 1b. Rust — user-visible strings, identity, IPC

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `src/jfn_rust/Cargo.toml` | 12 | `name = "jellium-desktop"` (`[[bin]]`) | PKG | `name = "astrofin"` |
| `src/jfn_rust/build.rs` | 1 | `//! …for the jellium-desktop binary.` | DOC | `astrofin` |
| `src/jfn_rust/build.rs` | 33 | comment `jellium-desktop-libmpv-git` | DOC | `astrofin-libmpv-git` (AUR pkg that does not exist yet — reword) |
| `src/jfn_rust/build.rs` | 34 | comment `/opt/jellium-desktop/libmpv/lib` | DOC | `/opt/astrofin/libmpv/lib` |
| `src/jfn_rust/src/cli.rs` | 1 | `//! Argv parser for jellium-desktop` | DOC | `astrofin` |
| `src/jfn_rust/src/cli.rs` | 8 | `ENV_LOG_LEVEL = "JELLIUM_DESKTOP_LOG_LEVEL"` | IPC/UV | `"ASTROFIN_LOG_LEVEL"` |
| `src/jfn_rust/src/cli.rs` | 9 | `ENV_LOG_FILE = "JELLIUM_DESKTOP_LOG_FILE"` | IPC/UV | `"ASTROFIN_LOG_FILE"` |
| `src/jfn_rust/src/cli.rs` | 10 | `ENV_CONFIG_DIR = "JELLIUM_DESKTOP_CONFIG_DIR"` | IPC/UV | `"ASTROFIN_CONFIG_DIR"` |
| `src/jfn_rust/src/cli.rs` | 11 | `ENV_CACHE_DIR = "JELLIUM_DESKTOP_CACHE_DIR"` | IPC/UV | `"ASTROFIN_CACHE_DIR"` |
| `src/jfn_rust/src/cli.rs` | 16 | doc `/// jellium-desktop — Jellyfin native desktop client.` | UV (`--help`) | `/// astrofin — Jellyfin native desktop client.` |
| `src/jfn_rust/src/cli.rs` | 18 | doc `a JELLIUM_DESKTOP_* environment variable` | UV (`--help`) | `ASTROFIN_*` |
| `src/jfn_rust/src/cli.rs` | 24 | `name = "jellium-desktop"` (clap command name) | UV | `name = "astrofin"` |
| `src/jfn_rust/src/app.rs` | 74 | `"jellium-desktop {}\n\nCEF {}\n"` (`--version`) | UV | `"astrofin {}…"` |
| `src/jfn_rust/src/app.rs` | 97 | `tracing::info!(… "jellium-desktop {APP_VERSION_FULL}")` | UV (log) | `astrofin` |
| `src/jfn_rust/src/app.rs` | 241 | `format!("JelliumDesktop/{}", APP_VERSION_FULL)` — **mpv user-agent** | UV/IPC | `format!("Astrofin/{}", …)` |
| `src/jfn_rust/src/app.rs` | 334 | `"Jellium Desktop"` → `build_device_profile(… device_name …)` | UV | `"Astrofin"` |
| `src/jfn_cef/src/ffi.rs` | 131 | `"Mozilla/5.0 jellium-desktop/"` — **CEF user agent** | UV/IPC | `"Mozilla/5.0 Astrofin/"` |
| `src/jfn_cef/src/ffi.rs` | 204 | `.unwrap_or_else(\|\| "jellium-desktop".to_string())` (argv[0] fallback) | CODE | `"astrofin"` |
| `src/jfn_cef/src/business_common.rs` | 5 | comment `Jellium Desktop config wiring` | DOC | `Astrofin` |
| `src/mpris/src/sink.rs` | 29 | `BASE_SERVICE_NAME = "org.mpris.MediaPlayer2.JelliumDesktop"` | IPC | `"org.mpris.MediaPlayer2.Astrofin"` |
| `src/mpris/src/sink.rs` | 79 | `ObjectPath::try_from("/net/nullsum/JelliumDesktop/track/1")` | IPC | `"/io/github/thehalfrican/Astrofin/track/1"` |
| `src/mpris/src/sink.rs` | 164 | `"Jellium Desktop"` (MPRIS `Identity` property) | UV | `"Astrofin"` |
| `src/mpris/src/sink.rs` | 494 | doc comment `org.mpris.MediaPlayer2.JelliumDesktop<suffix>` | DOC | `…Astrofin<suffix>` |
| `src/mpv/src/boot.rs` | 156 | `set("title", "Jellium Desktop")` | UV | `"Astrofin"` |
| `src/mpv/src/boot.rs` | 157 | `set("wayland-app-id", "net.nullsum.JelliumDesktop")` | APPID/IPC | `"io.github.thehalfrican.Astrofin"` |
| `src/mpv/build.rs` | 6 | comment `/opt/jellium-desktop/libmpv` | DOC | `/opt/astrofin/libmpv` |
| `src/wayland/src/root_window.rs` | 59 | `const APP_ID = "net.nullsum.JelliumDesktop"` | APPID/IPC | `"io.github.thehalfrican.Astrofin"` |
| `src/wayland/src/root_window.rs` | 60 | `const TITLE = "Jellium Desktop"` | UV | `"Astrofin"` |
| `src/wayland/src/kde_palette.rs` | 53 | `dir.push("jellium-desktop")` (`$XDG_RUNTIME_DIR/jellium-desktop`) | FS/IPC | `"astrofin"` |
| `src/wayland/src/kde_palette.rs` | 95 | `format!("JelliumDesktop-{}.colors", hex_str)` | FS | `format!("Astrofin-{}.colors", …)` |
| `src/wayland/src/kde_palette_template.ini` | 145 | `ColorScheme=JelliumDesktop` | UV/IPC | `ColorScheme=Astrofin` |
| `src/wayland/src/kde_palette_template.ini` | 146 | `Name=Jellium Desktop` | UV | `Name=Astrofin` |
| `src/x11/src/lifecycle.rs` | 23 | doc `Must match StartupWMClass in net.nullsum.JelliumDesktop.desktop` | DOC | `io.github.thehalfrican.Astrofin.desktop` |
| `src/x11/src/lifecycle.rs` | 25 | `WM_CLASS_VALUE = b"net.nullsum.JelliumDesktop\0net.nullsum.JelliumDesktop\0"` | IPC | `b"io.github.thehalfrican.Astrofin\0io.github.thehalfrican.Astrofin\0"` |
| `src/x11/src/lifecycle.rs` | 26 | `APP_TITLE = b"Jellium Desktop"` | UV | `b"Astrofin"` |
| `src/x11/src/shm.rs` | 51 | `memfd_create(c"jellium-shm", …)` | IPC (memfd name, cosmetic) | `c"astrofin-shm"` — **done** |
| `src/x11/src/mpv_proxy.rs` | 955 | comment `a private, jellium-owned runtime dir` | DOC | `astrofin-owned` |
| `src/linux_util/src/idle_inhibit.rs` | 59 | `&(what, "Jellium Desktop", "Media playback", "block")` — logind `Inhibit` who | UV | `"Astrofin"` |
| `src/macos/src/lib.rs` | 75 | `CFString::from_str("Jellium Desktop media playback")` — IOPMAssertion name | UV | `"Astrofin media playback"` |
| `src/macos/src/init.rs` | 894 | `"About Jellium Desktop"` (NSMenu item) | UV | `"About Astrofin"` |
| `src/macos/src/init.rs` | 901 | `"Hide Jellium Desktop"` (NSMenu item) | UV | `"Hide Astrofin"` |
| `src/web/native-shim.js` | 407 | `appName: 'Jellium Desktop'` (`NativeShell.AppHost.init`) | UV | `'Astrofin'` |
| `src/web/native-shim.js` | 425 | `appName() { return 'Jellium Desktop'; }` | UV | `'Astrofin'` |

## 1c. Legacy "Jellyfin"-branded identifiers (not `jellium`, but leftover branding)

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `src/windows/src/input.rs` | 4 | comment `registers a JellyfinCefInput window class` | DOC | `AstrofinCefInput` — **done** |
| `src/windows/src/input.rs` | 445 | `const CLASS_NAME: PCWSTR = w!("JellyfinCefInput")` | IPC (window class) | `w!("AstrofinCefInput")` — **done** (see §3 note: per-process, not a real collision, renamed for hygiene) |
| `src/windows/src/input.rs` | 497 | log `CreateWindowExW(JellyfinCefInput) failed` | DOC | `AstrofinCefInput` — **done** |
| `src/macos/src/input.rs` | 1, 275, 285 | `JellyfinInputView` NSView subclass | CODE | `AstrofinInputView` — **done** (ObjC classes are per-process) |
| `src/macos/src/init.rs` | 3, 45, 53, 55, 58, 81, 147, 152, 158, 159, 169–173, 224, 237, 242–252, 269–297, 345–348 | `JellyfinApplication`, `JellyfinAppIvars`, `JellyfinAppMenuTarget`, `JellyfinLifecycleObserver`, `JellyfinDisplayLinkTarget` | CODE | Renamed to `Astrofin*` (plus `JellyfinWakeTarget`) — **done**; purely internal ObjC runtime names |
| `src/web/csd.js` | 13 | `const HOST_TAG = 'jmp-titlebar'` | CODE/DOM | **Keep** — custom-element tag, no branding surface; renaming risks nothing but gains nothing |
| `src/web/csd.js` | 51, 190–198, 213 | `--jmp-csd-height`, `.jmp-csd-inset` | CODE/CSS | **Keep** — internal CSS custom-property/class names |
| `src/web/native-shim.js` | (many) | `jmpInfo`, `window.jmpNative` | CODE | **Keep** — jellyfin-web plugin-API compat surface inherited from jellyfin-media-player; renaming would need matching changes in every injected script |
| `src/web/about.js` | 50 | `logo.alt = 'Jellyfin'` | UV | `'Astrofin'` once artwork is swapped (see §6) |
| `src/jellyfin/**`, `src/mpv/**` refs to "Jellyfin" | — | product references to the *server* | — | **Keep** — these describe Jellyfin the server, not the client |

## 1d. Resources

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `resources/linux/net.nullsum.JelliumDesktop.desktop` | filename | — | APPID/PKG | rename → `io.github.thehalfrican.Astrofin.desktop` |
| ″ | 2 | `Name=Jellium Desktop` | UV | `Name=Astrofin` |
| ″ | 4 | `Exec=jellium-desktop` | FS | `Exec=astrofin` |
| ″ | 5 | `Icon=net.nullsum.JelliumDesktop` | APPID | `Icon=io.github.thehalfrican.Astrofin` |
| ″ | 9 | `StartupWMClass=net.nullsum.JelliumDesktop` | IPC | `StartupWMClass=io.github.thehalfrican.Astrofin` |
| `resources/linux/net.nullsum.JelliumDesktop.metainfo.xml` | filename | — | APPID/PKG | rename → `io.github.thehalfrican.Astrofin.metainfo.xml` |
| ″ | 3 | `<id>net.nullsum.JelliumDesktop</id>` | APPID | `io.github.thehalfrican.Astrofin` |
| ″ | 4 | `<name>Jellium Desktop</name>` | UV | `Astrofin` |
| ″ | 10 | `Jellium Desktop is a desktop client for Jellyfin media server,` | UV | `Astrofin is a desktop client for Jellyfin media server,` (add "a fork of Jellium Desktop" — see §5) |
| ″ | 14 | `<launchable …>net.nullsum.JelliumDesktop.desktop` | APPID | `io.github.thehalfrican.Astrofin.desktop` |
| ″ | 15 | `<icon type="stock">net.nullsum.JelliumDesktop` | APPID | `io.github.thehalfrican.Astrofin` |
| ″ | 17 | `<url type="bugtracker">…andrewrabert/jellium-desktop/issues` | PKG | `https://github.com/TheHalfrican/Astrofin/issues` |
| ″ | 19 | `<developer id="net.nullsum">` | APPID | `<developer id="io.github.thehalfrican">` |
| ″ | 20 | `<name>Jellium Contributors</name>` | UV | `Astrofin Contributors` (keep Jellium credit in `<description>`) |
| `resources/linux/net.nullsum.JelliumDesktop.svg` | filename | — | PKG | rename → `io.github.thehalfrican.Astrofin.svg` (artwork content later) |
| `resources/macos/Info.plist.in` | 8 | `<string>jellium-desktop</string>` (CFBundleExecutable) | FS | `astrofin` |
| ″ | 12 | `<string>net.nullsum.JelliumDesktop</string>` (CFBundleIdentifier) | APPID | `io.github.thehalfrican.Astrofin` |
| ″ | 16 | `<string>Jellium Desktop</string>` (CFBundleName) | UV | `Astrofin` |
| `resources/win/jellium-desktop.exe.manifest` | filename | — | PKG | rename → `astrofin.exe.manifest` |
| ″ | 5 | `name="net.nullsum.JelliumDesktop"` (assemblyIdentity) | APPID | `io.github.thehalfrican.Astrofin` |
| `resources/win/iconres.rc.in` | 3 | `…/resources/win/jellyfin.ico` | PKG | `…/resources/win/astrofin.ico` (rename asset; artwork later) |
| ″ | 5 | `…/resources/win/jellium-desktop.exe.manifest` | PKG | `…/resources/win/astrofin.exe.manifest` |
| ″ | 20 | `VALUE "CompanyName", "Jellium"` | UV | `"TheHalfrican"` |
| ″ | 21 | `VALUE "FileDescription", "Jellium Desktop"` | UV | `"Astrofin"` |
| ″ | 23 | `VALUE "InternalName", "jellium-desktop"` | UV | `"astrofin"` |
| ″ | 24 | `VALUE "OriginalFilename", "jellium-desktop.exe"` | UV | `"astrofin.exe"` |
| ″ | 25 | `VALUE "ProductName", "Jellium Desktop"` | UV | `"Astrofin"` |
| ~~`src/web/logo.png`~~ | — | Jellyfin logo shown in About + overlay | UV/art | **done** — deleted; About and the overlay use `src/web/logo-mark.svg` |
| ~~`resources/macos/AppIcon.icns`~~ | — | app icon | art | **done** — regenerated from `resources/brand/astrofin-icon.svg` |

## 1e. xtask (build/packaging code)

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `src/xtask/src/build.rs` | 29 | `.arg("jellium-desktop")` (`cargo build --bin`) | PKG | `"astrofin"` |
| ″ | 88 | `println!("Building jellium-desktop (Rust binary)...")` | UV (build log) | `astrofin` |
| ″ | 95 | `"jellium-desktop.exe"` | FS | `"astrofin.exe"` |
| ″ | 97 | `"jellium-desktop"` | FS | `"astrofin"` |
| `src/xtask/src/package.rs` | 39 | `format!("JelliumDesktop-{}-{}-{}", ver.full, TARGET.os_slug, arch)` | PKG | `format!("Astrofin-{}-{}-{}", …)` |
| `src/xtask/src/platform_windows.rs` | 34, 35 | `build_dir.join("jellium-desktop.exe")` / `prefix.join(…)` | FS | `astrofin.exe` |
| `src/xtask/src/platform_linux.rs` | 24, 25 | `build_dir.join("jellium-desktop")` / `prefix.join(…)` | FS | `astrofin` |
| `src/xtask/src/platform_macos.rs` | 7 | `const MACOS_APP_NAME = "Jellium Desktop.app"` | PKG/FS | `"Astrofin.app"` |
| ″ | 30 | `out.join("jellium-desktop")` (install_name -change target) | FS | `astrofin` |
| ″ | 78, 79 | `build_dir.join("jellium-desktop")` / `macos_dir.join(…)` | FS | `astrofin` |
| `src/xtask/src/bundle_macos.rs` | 53 | `let bin = macos_dir.join("jellium-desktop")` | FS | `astrofin` |
| ″ | 81 | `queue.push(macos_dir.join("jellium-desktop"))` | FS | `astrofin` |
| ″ | 333 | `sign(&macos_dir.join("jellium-desktop"), …)` | FS | `astrofin` |

## 1f. justfiles + dev scripts

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `justfile` | 3 | `export JELLIUM_DESKTOP_LOG_LEVEL := env_var_or_default("JELLIUM_DESKTOP_LOG_LEVEL", "debug")` | PKG | `ASTROFIN_LOG_LEVEL` (both sides) |
| `justfile` | 4 | `export JELLIUM_DESKTOP_LOG_FILE := env_var_or_default("JELLIUM_DESKTOP_LOG_FILE", "build/run.log")` | PKG | `ASTROFIN_LOG_FILE` (both sides) |
| `dev/linux/linux.just` | 19 | `build/jellium-desktop {{args}}` | FS | `build/astrofin` |
| ″ | 45 | `cage -- build/jellium-desktop {{args}}` | FS | `build/astrofin` |
| `dev/macos/macos.just` | 22 | `"build/output/Jellium Desktop.app/Contents/MacOS/jellium-desktop" {{args}}` | FS | `"build/output/Astrofin.app/Contents/MacOS/astrofin"` |
| `dev/windows/windows.just` | 22 | `& 'build/jellium-desktop.exe' {{args}}` | FS | `& 'build/astrofin.exe'` |
| `dev/windows/setup.ps1` | 1 | comment `Setup Windows development environment for jellium-desktop` | DOC | `astrofin` |
| ″ | 16 | `Write-Host "=== jellium-desktop Windows Setup ==="` | UV | `=== astrofin Windows Setup ===` |
| `dev/windows/build.ps1` | 1 | comment `Build jellium-desktop on Windows` | DOC | `astrofin` |
| ″ | 41 | `Write-Host "Executable: $BuildDir\jellium-desktop.exe"` | UV | `$BuildDir\astrofin.exe` |
| `dev/windows/build_mpv_source.ps1` | 177 | comment `jellium-desktop links libavcodec directly` | DOC | `astrofin` |
| `dev/macos/setup.sh` | 2 | `# Jellium Desktop - macOS dependency installer` | DOC | `# Astrofin - …` |
| `dev/macos/build_dmg.sh` | 2 | `# Jellium Desktop - create distributable DMG…` | DOC | `# Astrofin - …` |
| ″ | 8 | `APP_NAME="Jellium Desktop.app"` | PKG/FS | `APP_NAME="Astrofin.app"` |
| ″ | 21 | `DMG_NAME="JelliumDesktop-${VERSION}-macos-${ARCH}.dmg"` | PKG | `Astrofin-${VERSION}-macos-${ARCH}.dmg` |
| ″ | 25 | `--volname "Jellium Desktop v${VERSION}"` | UV | `"Astrofin v${VERSION}"` |

## 1g. AppImage

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `dev/linux/appimage/Dockerfile` | 1 | comment `…building jellium-desktop AppImages.` | DOC | `astrofin` |
| `dev/linux/appimage/build.sh` | 8 | `IMG="jellium-desktop-appimage:base"` | PKG | `IMG="astrofin-appimage:base"` |
| ″ | 36 | `BUNDLE="dist/JelliumDesktop-${VERSION}-${ARCH}.AppImage"` | PKG | `dist/Astrofin-${VERSION}-${ARCH}.AppImage` |
| ″ | 37 | `LINK="…/build/appimage/JelliumDesktop.AppImage"` | PKG | `…/build/appimage/Astrofin.AppImage` |
| `dev/linux/appimage/run.sh` | 7 | `exec "…/build/appimage/JelliumDesktop.AppImage" "$@"` | PKG | `Astrofin.AppImage` |
| `dev/linux/appimage/AppRun` | 15 | `INTERP_DIR="/tmp/.jf-cef-interp"` | **IPC/FS (side-by-side collision)** | `/tmp/.astrofin-cef-interp` — see §3 |
| ″ | 34 | `exec "${HERE}/usr/bin/jellium-desktop" "$@"` | FS | `astrofin` |
| `dev/linux/appimage/container-build.sh` | 2 | comment `(jellium-desktop-appimage:base)` | DOC | `astrofin-appimage:base` |
| ″ | 23 | `strip /build/*.so /build/jellium-desktop` | FS | `/build/astrofin` |
| ″ | 33 | `cp "$BUILD"/jellium-desktop "$APPDIR/usr/bin/"` | FS | `astrofin` |
| ″ | 61 | comment `next to jellium-desktop` | DOC | `astrofin` |
| ″ | 74 | `cp /src/resources/linux/net.nullsum.JelliumDesktop.desktop \` | PKG | `io.github.thehalfrican.Astrofin.desktop` |
| ″ | 76 | `cp /src/resources/linux/net.nullsum.JelliumDesktop.svg \` | PKG | `io.github.thehalfrican.Astrofin.svg` |
| ″ | 78 | `cp /src/resources/linux/net.nullsum.JelliumDesktop.metainfo.xml \` | PKG | `io.github.thehalfrican.Astrofin.metainfo.xml` |
| ″ | 152–156 | `patchelf --set-interpreter "/tmp/.jf-cef-interp/${LD_SONAME}" "$APPDIR/usr/bin/jellium-desktop"` | IPC/FS | `/tmp/.astrofin-cef-interp/…` + `astrofin` (must match `AppRun:15`) |
| ″ | 159 | `cp "$APPDIR/usr/share/applications/net.nullsum.JelliumDesktop.desktop" "$APPDIR/"` | PKG | `io.github.thehalfrican.Astrofin.desktop` |
| ″ | 160 | `cp …/net.nullsum.JelliumDesktop.svg "$APPDIR/"` | PKG | `io.github.thehalfrican.Astrofin.svg` |
| ″ | 161 | `ln -sf net.nullsum.JelliumDesktop.svg "$APPDIR/.DirIcon"` | PKG | `io.github.thehalfrican.Astrofin.svg` |
| ″ | 168 | `"/host-output/JelliumDesktop-${VERSION}-${ARCH}.AppImage"` | PKG | `Astrofin-${VERSION}-${ARCH}.AppImage` |

## 1h. Flatpak

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `dev/linux/flatpak/net.nullsum.JelliumDesktop.yml` | filename | — | APPID/PKG | rename → `io.github.thehalfrican.Astrofin.yml` |
| ″ | 1 | `app-id: net.nullsum.JelliumDesktop` | APPID | `io.github.thehalfrican.Astrofin` |
| ″ | 10 | `command: jellium-desktop` | FS | `astrofin` |
| ″ | 19 | `- --own-name=org.mpris.MediaPlayer2.JelliumDesktop` | IPC | `org.mpris.MediaPlayer2.Astrofin` (`--own-name=org.mpris.MediaPlayer2.Astrofin.*` may be needed for the `.instance_<uuid>` suffix — verify current behavior) |
| ″ | 160 | `- name: jellium-desktop` (module name) | PKG | `astrofin` |
| ″ | 171 | `CARGO_HOME: /run/build/jellium-desktop/cargo` | FS | `/run/build/astrofin/cargo` (must match module name on 160) |
| ″ | 181 | `mkdir -p /app/bin && ln -sf ../jellium-desktop /app/bin/jellium-desktop` | FS | `../astrofin /app/bin/astrofin` |
| ″ | 185 | `install -Dm644 resources/linux/net.nullsum.JelliumDesktop.desktop /app/share/applications/net.nullsum.JelliumDesktop.desktop` | APPID/PKG | both → `io.github.thehalfrican.Astrofin.desktop` |
| ″ | 186 | same for `.svg` | APPID/PKG | `io.github.thehalfrican.Astrofin.svg` |
| ″ | 187 | `install … /app/share/metainfo/net.nullsum.JelliumDesktop.metainfo.xml` | APPID/PKG | `io.github.thehalfrican.Astrofin.metainfo.xml` |
| `dev/linux/flatpak/build.sh` | 11 | `MANIFEST="${SCRIPT_DIR}/net.nullsum.JelliumDesktop.yml"` | PKG | `io.github.thehalfrican.Astrofin.yml` |
| ″ | 12 | `APP_ID="net.nullsum.JelliumDesktop"` | APPID | `io.github.thehalfrican.Astrofin` |
| ″ | 16 | `BUNDLE_NAME="JelliumDesktop-${VERSION}-linux-${ARCH}.flatpak"` | PKG | `Astrofin-${VERSION}-linux-${ARCH}.flatpak` |
| ″ | 49 | `--template "${REPO_ROOT}/resources/linux/net.nullsum.JelliumDesktop.metainfo.xml"` | PKG | `io.github.thehalfrican.Astrofin.metainfo.xml` |
| `dev/linux/flatpak/LICENSE.txt` | — | GPL-3 text bundled for flatpak | DOC | **Keep** (see §5 — note the GPL-2 vs GPL-3 mismatch with the repo `LICENSE`) |

## 1i. CI / GitHub

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `.github/workflows/build-linux-appimage.yml` | 45 | `path: dist/JelliumDesktop-*.AppImage` | PKG | `dist/Astrofin-*.AppImage` |
| `.github/workflows/build-linux-flatpak.yml` | 50 | `path: dist/JelliumDesktop-*.flatpak` | PKG | `dist/Astrofin-*.flatpak` |
| `.github/workflows/build-macos.yml` | 63 | `APP_NAME="Jellium Desktop"` | PKG | `APP_NAME="Astrofin"` |
| ″ | 66 | `DMG_NAME="JelliumDesktop-${VERSION}-macos-${ARCH}.dmg"` | PKG | `Astrofin-…` |
| ″ | 71 | `--volname "Jellium Desktop v${VERSION}"` | UV | `"Astrofin v${VERSION}"` |
| ″ | 91 | `path: dist/JelliumDesktop-*.dmg` | PKG | `dist/Astrofin-*.dmg` |
| `.github/workflows/build-macos-legacy.yml` | 142 | `APP_NAME="Jellium Desktop"` | PKG | `"Astrofin"` |
| ″ | 144 | `DMG_NAME="JelliumDesktop-${VERSION}-macos-12-x86_64.dmg"` | PKG | `Astrofin-…` |
| ″ | 148 | `--volname "Jellium Desktop v${VERSION}"` | UV | `"Astrofin v${VERSION}"` |
| ″ | 166 | `path: dist/JelliumDesktop-*.dmg` | PKG | `dist/Astrofin-*.dmg` |
| `.github/workflows/build-windows.yml` | 94 | `path: dist/JelliumDesktop-*.zip` | PKG | `dist/Astrofin-*.zip` |
| `.github/workflows/build-macos.yml` | 36 | `run: echo "version=$(dev/tools/version.sh)"` | **BUG (pre-existing)** | `dev/tools/version.sh` **does not exist in this repo** — replace with `cargo run --quiet --manifest-path src/xtask/Cargo.toml -- version` |
| `.github/workflows/build-macos-legacy.yml` | 37 | same | **BUG (pre-existing)** | same fix |
| `.github/workflows/build-macos-legacy.yml` | 59, 68, 96, 134, 136 | `~/jellyfin-deps` / `$HOME/jellyfin-deps` | FS (CI-local) | optional → `~/astrofin-deps`; harmless to keep |
| `.github/ISSUE_TEMPLATE/*.yml` | — | no branding strings | — | **Keep** |
| `.github/actions/setup-cef/action.yml` | — | no branding strings | — | **Keep** |
| `.gitmodules` | 3 | `url = https://github.com/andrewrabert/mpv` | PKG | **Keep** — the mpv fork is a genuine upstream dependency, not branding |
| `renovate.json`, `.dockerignore`, `.gitignore`, `.cargo/config.toml`, `src/clippy.toml` | — | no branding | — | **Keep** |

## 1j. Docs

| File | Line | Current text | Cat | Replacement |
| --- | --- | --- | --- | --- |
| `README.md` | 1 | `# Jellium Desktop` | DOC | `# Astrofin` + attribution paragraph (§5) |
| `README.md` | 8 | nightly.link `andrewrabert/jellium-desktop/workflows/build-linux-appimage/main/linux-appimage-x86_64.zip` | DOC | `TheHalfrican/Astrofin/...` |
| `README.md` | 9 | same, `linux-appimage-aarch64.zip` | DOC | `TheHalfrican/Astrofin/...` |
| `README.md` | 10 | AUR `jellium-desktop-git` | DOC | **Remove** — no Astrofin AUR package exists |
| `README.md` | 11 | nightly.link flatpak link | DOC | `TheHalfrican/Astrofin/...` |
| `README.md` | 14 | nightly.link `build-macos/main/macos-arm64.zip` | DOC | `TheHalfrican/Astrofin/...` |
| `README.md` | 15 | nightly.link `build-macos/main/macos-x86_64.zip` | DOC | `TheHalfrican/Astrofin/...` |
| `README.md` | 19 | `sudo xattr -cr /Applications/Jellium\ Desktop.app` | DOC | `/Applications/Astrofin.app` |
| `README.md` | 23 | nightly.link `build-windows/main/windows-x64.zip` | DOC | `TheHalfrican/Astrofin/...` |
| `README.md` | 24 | nightly.link `build-windows/main/windows-arm64.zip` | DOC | `TheHalfrican/Astrofin/...` |
| `CLAUDE.md` | 4 | `produces the jellium-desktop binary` | DOC | `astrofin` |
| `LICENSE` | — | GPL-2.0 full text | DOC | **Keep verbatim** (see §5) |

---

# 2. Config/cache/log path code + first-run migration design

## 2a. Where the names live

`src/paths/src/lib.rs` is the single source of truth:

- `APP_DIR_NAME` (line 17) and `LOG_FILE_NAME` (line 18).
- `config_dir()` (:90) → `imp::config_base().join(APP_DIR_NAME)`, wrapped in `ensure()` (`create_dir_all`).
- `cache_dir()` (:97) → `imp::cache_base().join(APP_DIR_NAME)`, also `ensure()`d.
- `log_dir()` (:104) → `imp::log_dir_path()`, `ensure()`d; `log_path()` (:159) joins `LOG_FILE_NAME`.
- `mpv_home()` (:108) → `config_dir()/mpv`.
- `runtime_dir()` (:112, unix) → `$XDG_RUNTIME_DIR`, else `/tmp/{APP_DIR_NAME}-{uid}` (0700, ownership-checked).
- `instance_listener_path(id)` (:148) → unix `runtime_dir()/{APP_DIR_NAME}-{id}`; windows `\\.\pipe\{APP_DIR_NAME}-{id}`.
- Overrides: `set_config_dir_override` / `set_cache_dir_override` (:34/:38), applied before any getter is used.

Platform bases:

| | config | cache | logs |
| --- | --- | --- | --- |
| Linux (`imp_linux.rs`) | `dirs::config_dir()` → `~/.config/<APP>` | `dirs::cache_dir()` → `~/.cache/<APP>` | `dirs::state_dir()/<APP>` → `~/.local/state/<APP>` |
| macOS (`imp_macos.rs`) | `$XDG_CONFIG_HOME` or `~/.config/<APP>` | `~/Library/Caches/<APP>` | `~/Library/Logs/<APP>` |
| Windows (`imp_windows.rs`) | `dirs::config_dir()` = `%APPDATA%\<APP>` | `dirs::data_local_dir()` = `%LOCALAPPDATA%\<APP>` | `%LOCALAPPDATA%\<APP>\Logs` |

Note Windows: log dir is **inside** the cache dir (`%LOCALAPPDATA%\astrofin\Logs`) — the CEF
`root_cache_path` is `%LOCALAPPDATA%\astrofin` itself. Migrating the cache dir therefore also
carries `Logs/`.

## 2b. Consumers of `jfn_paths`

| Call site | What it uses |
| --- | --- |
| `src/jfn_rust/src/app.rs:85` | `default_log_file()` |
| `src/jfn_rust/src/app.rs:139` | `mpv_home()` → sets `MPV_HOME` |
| `src/jfn_rust/src/app.rs:553/556` | `set_config_dir_override` / `set_cache_dir_override` |
| `src/jfn_rust/src/app.rs:559` | `config_dir().join("settings.json")` → `settings_init` + `settings_load` |
| `src/jfn_rust/src/app.rs:573` | `Instance::for_config_dir(&config_dir())` |
| `src/jfn_cef/src/ffi.rs:120` | `config_dir().join("settings.json")` (inside `jfn_cef_initialize`) |
| `src/jfn_cef/src/ffi.rs:134` | `cache_dir()` → CEF `root_cache_path` (the login session lives here) |
| `src/jfn_cef/src/app.rs:652` | `config_dir().join("settings.json")` — **renderer process** (`ensure_renderer_settings_loaded`) |
| `src/jfn_cef/src/resource.rs:83` | `config_dir()` → shown in the About panel |
| `src/jfn_cef/src/business_web.rs:421` | `mpv_home()` → "open mpv config folder" button |
| `src/instance_ipc/src/lib.rs:25` | `instance_listener_path(instance.id())` |
| `src/x11/src/mpv_proxy.rs:991` | `runtime_dir()` for the xauth temp file |
| `src/platform_abi/src/instance.rs:104` | `write_atomic` (instance.json) |
| `src/config/src/lib.rs:415` | `write_atomic` (settings.json) |

## 2c. Migration design

**Where:** new module `src/paths/src/migrate.rs`, re-exported as `jfn_paths::migrate_legacy()`.
Keeping it in `jfn-paths` lets it use the private `imp::*_base()` functions and the
`APP_DIR_NAME` constant, and it is where a future second rename would also live.

**Prerequisite refactor (non-optional).** `config_dir()` and `cache_dir()` call `ensure()`, which
*creates* the directory. A migration that asks "does the new dir exist?" must therefore run before
any getter, and must not use the getters to answer the question. Add non-creating variants:

```rust
// src/paths/src/lib.rs
const LEGACY_APP_DIR_NAME: &str = "jellium-desktop";

fn config_dir_raw() -> PathBuf { config_override().unwrap_or_else(|| imp::config_base().join(APP_DIR_NAME)) }
fn cache_dir_raw()  -> PathBuf { cache_override().unwrap_or_else(|| imp::cache_base().join(APP_DIR_NAME)) }
```

and have `config_dir()`/`cache_dir()` become `ensure(config_dir_raw())` / `ensure(cache_dir_raw())`.

**When:** in `src/jfn_rust/src/app.rs::jfn_app_main`, immediately after the two
`set_*_dir_override` calls (currently lines 552–557) and **before** line 559
(`jfn_paths::config_dir().join("settings.json")`). That point is:

- after `jfn_cef::ffi::jfn_cef_start()` returned `< 0`, so **only the browser process** reaches it
  (CEF helper subprocesses return early at line 542) — no concurrent migration races;
- before `settings_init` / `settings_load`, so the copied `settings.json` is the one that loads;
- before `Instance::for_config_dir`, so the copied `instance.json` (and therefore the Jellyfin
  device identity and the MPRIS `.instance_<uuid>` suffix) is reused rather than re-minted;
- before `jfn_cef_initialize` builds `root_cache_path` from `cache_dir()`, so the copied CEF
  profile is in place when Chromium opens it.

**Signature and behavior:**

```rust
pub fn migrate_legacy() -> MigrationReport   // never returns Err; logs and degrades
```

```
for (legacy_base, new_dir) in [
    (imp::config_base().join(LEGACY_APP_DIR_NAME), config_dir_raw()),
    (imp::cache_base().join(LEGACY_APP_DIR_NAME),  cache_dir_raw()),
] {
    if new_dir.exists()            { continue; }   // already migrated OR user opted out
    if !legacy_base.is_dir()       { continue; }   // fresh install, nothing to carry
    copy_tree(&legacy_base, &new_dir)?;
}
```

Skip the migration entirely when the corresponding CLI/env override is set — an explicit
`--config-dir` means the user chose the location and copying into it is surprising. (Simplest:
`if config_override().is_some() { skip config leg }`, same for cache.)

Linux logs live in a third root (`~/.local/state/<APP>`); migrating logs is pointless, so **do not
migrate the log dir** — new logs simply start in the new location.

**`copy_tree` rules:**

1. **Copy into a staging dir, then rename.** Write to `<new_dir>.migrating-<pid>` and
   `fs::rename` into place at the end. A crash mid-copy leaves the staging dir, and the next launch
   still sees `new_dir` absent and retries cleanly. Delete a stale `*.migrating-*` on entry.
2. **Never touch the source.** Open-read only. No deletes, no moves, no permission changes on
   `jellium-desktop`.
3. **Symlinks:** use `symlink_metadata`; if an entry is a symlink, recreate the *link* rather than
   following it (avoids duplicating a large media dir someone symlinked in, and avoids cycles). If
   symlink creation fails (Windows without Developer Mode / SeCreateSymbolicLink), fall back to
   copying the target if it is a regular file, else skip that entry and log a warning.
4. **Depth/cycle guard:** cap recursion at, say, 32 levels; refuse to descend into a directory whose
   canonical path is a prefix of `new_dir` (prevents self-copy if someone symlinked the new dir into
   the old one).
5. **Per-entry error tolerance:** a failed file copy logs `warn!` and continues; only a failure to
   create the staging root aborts the leg. Report counts (`copied`, `skipped`, `failed`).
6. **Locked files (Windows):** the CEF profile contains SQLite DBs and `LOCK` files. If the old
   Jellium Desktop is *running*, `Cookies`, `Local State`, `first_party_sets.db` etc. may be
   open. `std::fs::copy` on Windows uses `CopyFileEx`, which succeeds for files opened with
   `FILE_SHARE_READ` (Chromium's usual mode) but can fail on the lock files. Skip-and-warn on
   `PermissionDenied`; the profile is still usable — Chromium recreates lock/journal files. Emit a
   one-line `info!` telling the user to close Jellium Desktop and delete
   `%LOCALAPPDATA%\astrofin` if the session did not carry over.
7. **Do not copy** the CEF `SingletonLock` / `SingletonSocket` / `SingletonCookie` entries (Linux/macOS)
   or `LOCK` — an inherited singleton marker pointing at the other install's PID is exactly the kind
   of cross-talk this rebrand is meant to avoid. Explicit denylist by file name.
8. **Unix permissions:** preserve mode via `fs::set_permissions` on regular files; create directories
   0700 for the cache/config roots (matches the existing `private_dir` posture).
9. **Idempotent + observable:** log at `info!` once per leg with source, destination, and counts.
   Because logging is initialized *after* this point (`init_logging` runs at app.rs:565), either
   (a) move `init_logging` above the migration, or (b) buffer the report and emit it right after
   `init_logging`. **(b) is preferable** — `init_logging` itself calls `default_log_file()` →
   `log_dir()`, which is a different root and unaffected by the migration, so the ordering is free
   either way; buffering keeps the change local.

**Platform differences to honor:**

| | legacy source | new destination |
| --- | --- | --- |
| Windows config | `%APPDATA%\jellium-desktop` | `%APPDATA%\astrofin` |
| Windows cache (CEF profile + Logs) | `%LOCALAPPDATA%\jellium-desktop` | `%LOCALAPPDATA%\astrofin` |
| Linux config | `~/.config/jellium-desktop` | `~/.config/astrofin` |
| Linux cache | `~/.cache/jellium-desktop` | `~/.cache/astrofin` |
| macOS config | `$XDG_CONFIG_HOME`/`~/.config/jellium-desktop` | …`/astrofin` |
| macOS cache | `~/Library/Caches/jellium-desktop` | `~/Library/Caches/astrofin` |

On macOS the config root honors `$XDG_CONFIG_HOME`, so the legacy path must be derived from
`imp::config_base()` — never hardcode `~/.config`.

**Gotcha to flag in the migration's own comment:** the copied `mpv/mpv.conf` may contain
`input-ipc-server=\\.\pipe\jellium-mpv` (it does on this machine). See §3 and §6.

---

# 3. Side-by-side collision inventory

Every identifier that two concurrently running installs would contend for.

| # | Identifier | Where | Current | New | Collision risk if unchanged |
| --- | --- | --- | --- | --- | --- |
| 1 | Single-instance socket / named pipe | `src/paths/src/lib.rs:151,155` via `APP_DIR_NAME` | `/run/user/<uid>/jellium-desktop-<uuid>` · `\\.\pipe\jellium-desktop-<uuid>` | `astrofin-<uuid>` | **Critical.** The migration copies `instance.json`, so both apps would carry the *same* UUID. With the same prefix they'd share one pipe → launching Astrofin would just ping Jellium and exit ("Signaled existing instance, exiting", `app.rs:592`). Renaming `APP_DIR_NAME` fixes it. |
| 2 | CEF `root_cache_path` | `src/jfn_cef/src/ffi.rs:134` via `cache_dir()` | `%LOCALAPPDATA%\jellium-desktop` | `%LOCALAPPDATA%\astrofin` | **Critical.** Chromium takes an exclusive process singleton on the profile dir; two processes on one profile → second one refuses to start or corrupts state. |
| 3 | Config dir (settings.json, instance.json, mpv/) | `config_dir()` | `%APPDATA%\jellium-desktop` | `%APPDATA%\astrofin` | High — shared settings would fight (both write `settings.json` atomically on change). |
| 4 | MPRIS bus name | `src/mpris/src/sink.rs:29` + `lib.rs:13` suffix | `org.mpris.MediaPlayer2.JelliumDesktop.instance_<uuid>` | `org.mpris.MediaPlayer2.Astrofin.instance_<uuid>` | High on Linux — identical UUID after migration means identical bus name; second claim fails. |
| 5 | MPRIS track ObjectPath | `src/mpris/src/sink.rs:79` | `/net/nullsum/JelliumDesktop/track/1` | `/io/github/thehalfrican/Astrofin/track/1` | Cosmetic, but part of the app id family. |
| 6 | Flatpak `--own-name` | `dev/linux/flatpak/*.yml:19` | `org.mpris.MediaPlayer2.JelliumDesktop` | `…Astrofin` | Must track #4 or MPRIS silently fails inside the sandbox. |
| 7 | Wayland app id (toplevel) | `src/wayland/src/root_window.rs:59` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | Not a hard collision, but both windows would group under one taskbar icon / desktop file. |
| 8 | Wayland app id (mpv's own surface) | `src/mpv/src/boot.rs:157` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | Same; must match #7. |
| 9 | X11 `WM_CLASS` | `src/x11/src/lifecycle.rs:25` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | Same grouping problem; must match the `.desktop` `StartupWMClass`. |
| 10 | `.desktop` file name + `StartupWMClass` | `resources/linux/*.desktop` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | Installing both would overwrite one another's desktop entry. |
| 11 | Flatpak app id | `dev/linux/flatpak/*.yml:1`, `build.sh:12` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | Same id = same flatpak install slot; one replaces the other. |
| 12 | KDE palette runtime dir | `src/wayland/src/kde_palette.rs:53` | `$XDG_RUNTIME_DIR/jellium-desktop` | `$XDG_RUNTIME_DIR/astrofin` | Both would write `*.colors` into one 0700 dir and stomp each other's scheme file. |
| 13 | KDE color-scheme name | `kde_palette_template.ini:145–146` + `kde_palette.rs:95` | `JelliumDesktop`, `JelliumDesktop-<hex>.colors` | `Astrofin`, `Astrofin-<hex>.colors` | Cosmetic once #12 is split, but keep consistent. |
| 14 | Unix runtime dir fallback | `src/paths/src/lib.rs:120` via `APP_DIR_NAME` | `/tmp/jellium-desktop-<uid>` | `/tmp/astrofin-<uid>` | Shared 0700 dir; the xauth temp files (`mpv_proxy.rs:991`) and the instance socket both live here. |
| 15 | AppImage ELF-interpreter dir | `dev/linux/appimage/AppRun:15` + `container-build.sh:152` | `/tmp/.jf-cef-interp/<ld.so>` | `/tmp/.astrofin-cef-interp/<ld.so>` | **Real hazard.** Both AppImages `ln -sf` into the same fixed path; whichever launched last repoints the symlink, and the other process's *future* re-execs of `/proc/self/exe` (CEF renderer/GPU spawn) load the wrong glibc. |
| 16 | Windows exe file name | everywhere | `jellium-desktop.exe` | `astrofin.exe` | Not a lock, but the user's `switch-mode.ps1` does `Get-Process 'jellium-desktop'` and would match/kill the wrong app. |
| 17 | Windows Win32 assembly identity | `resources/win/*.exe.manifest:5` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | Cosmetic (SxS identity), but part of the app-id family. |
| 18 | Windows input window class | `src/windows/src/input.rs:445` | `JellyfinCefInput` | `AstrofinCefInput` — **done** | **Not** a cross-process collision — classes registered with an `HINSTANCE` are process-local. Rename for hygiene only. |
| 19 | SMTC identity | `src/windows_sink/src/lib.rs` (`GetForWindow(hwnd)`) | derived from the process/HWND, **no string in the repo** | — | No repo change needed; Windows derives the app name/icon from the exe (and AppUserModelID if set). Because the exe name and VERSIONINFO `ProductName` change, SMTC shows "Astrofin" automatically. **Hardening — done:** `win_early_init` calls `SetCurrentProcessExplicitAppUserModelID("io.github.thehalfrican.Astrofin")` so the taskbar/SMTC never conflates the two installs' shortcuts. |
| 20 | macOS bundle identifier | `resources/macos/Info.plist.in:12` | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` | **Critical on macOS** — LaunchServices keys everything (defaults, TCC grants, Dock slot, "already running") off CFBundleIdentifier. Identical ids = one app shadows the other. |
| 21 | macOS `.app` directory | `src/xtask/src/platform_macos.rs:7` | `Jellium Desktop.app` | `Astrofin.app` | Same `/Applications` slot otherwise. |
| 22 | ObjC runtime class names | `src/macos/src/init.rs`, `input.rs` | `Jellyfin*` | `Astrofin*` — **done** | Per-process; no collision. |
| 23 | logind `Inhibit` who-string | `src/linux_util/src/idle_inhibit.rs:59` | `Jellium Desktop` | `Astrofin` | No collision (fd-based), user-visible in `systemd-inhibit --list`. |
| 24 | IOPMAssertion name | `src/macos/src/lib.rs:75` | `Jellium Desktop media playback` | `Astrofin media playback` | No collision; shows in `pmset -g assertions`. |
| 25 | X11 memfd name | `src/x11/src/shm.rs:51` | `jellium-shm` | `astrofin-shm` — **done** | No collision (anonymous memfd); cosmetic in `/proc/*/fd`. |
| 26 | **mpv JSON IPC pipe** | *user's* `%APPDATA%\jellium-desktop\mpv\mpv.conf` | `\\.\pipe\jellium-mpv` | `\\.\pipe\astrofin-mpv` | **Outside the repo, but a real collision** — the migration copies `mpv/` verbatim, so both installs would try to own the same named pipe. See §6. |
| 27 | Jellyfin device identity | `instance.json` UUID → device id + `device_name` | copied by the migration | — | Not a crash, but the Jellyfin server will see two sessions claiming the same device. **Recommendation:** copy `instance.json` (as decided, to preserve the session) but make the *default device name* distinct — `Astrofin (<hostname>)` — so the server's Devices list distinguishes them. If you'd rather have fully separate device rows, exclude `instance.json` from the migration; that costs a re-login. |

---

# 4. Packaging + CI

## 4a. Artifact names

| Platform | Now | After |
| --- | --- | --- |
| Windows zip (`xtask package`) | `JelliumDesktop-<ver>-windows-<x64\|arm64>.zip` | `Astrofin-<ver>-windows-<arch>.zip` |
| macOS zip (`xtask package`) | `JelliumDesktop-<ver>-macos-<arch>.zip` | `Astrofin-<ver>-macos-<arch>.zip` |
| Linux tarball (`xtask package`) | `JelliumDesktop-<ver>-linux-<arch>.tar.gz` | `Astrofin-<ver>-linux-<arch>.tar.gz` |
| macOS DMG | `JelliumDesktop-<ver>-macos-<arch>.dmg` | `Astrofin-<ver>-macos-<arch>.dmg` |
| AppImage | `JelliumDesktop-<ver>-<arch>.AppImage` | `Astrofin-<ver>-<arch>.AppImage` |
| Flatpak bundle | `JelliumDesktop-<ver>-linux-<arch>.flatpak` | `Astrofin-<ver>-linux-<arch>.flatpak` |
| Flatpak app id | `net.nullsum.JelliumDesktop` | `io.github.thehalfrican.Astrofin` |

All driven from `src/xtask/src/package.rs:39` (`format!("JelliumDesktop-…")`),
`dev/macos/build_dmg.sh:21`, `dev/linux/appimage/build.sh:36`, `dev/linux/flatpak/build.sh:16`,
plus the DMG blocks inlined in the two macOS workflows (which do **not** call `build_dmg.sh`).

## 4b. macOS bundle

- `src/xtask/src/platform_macos.rs:7` — `MACOS_APP_NAME` → `"Astrofin.app"`.
- `resources/macos/Info.plist.in` — `CFBundleExecutable` → `astrofin`, `CFBundleIdentifier` →
  `io.github.thehalfrican.Astrofin`, `CFBundleName` → `Astrofin`. `CFBundleIconFile` stays `AppIcon`
  (only the `.icns` content changes).
- Binary path references: `platform_macos.rs:30,78,79`; `bundle_macos.rs:53,81,333` (the last one is
  the codesign call — if it misses the rename the bundle ships unsigned/mismatched).
- `dev/macos/macos.just:22` — the `just run` path.
- `.github/workflows/build-macos*.yml` — `APP_NAME`, `DMG_NAME`, `--volname`, artifact `path:`.
- **Also fix:** both macOS workflows call `dev/tools/version.sh`, which does not exist in this repo.

## 4c. Linux desktop/AppStream

- Rename all three `resources/linux/net.nullsum.JelliumDesktop.{desktop,metainfo.xml,svg}` files to
  `io.github.thehalfrican.Astrofin.*`. AppStream requires the metainfo filename to equal the
  component `<id>`, and the `<launchable>` to equal the `.desktop` filename — all three must move
  together or `appstreamcli validate` fails.
- Update the three copy sites in `dev/linux/appimage/container-build.sh` (74/76/78, 159/160/161) and
  the three `install -Dm644` lines in the flatpak manifest (185/186/187).

## 4d. Windows

- `resources/win/jellium-desktop.exe.manifest` → `astrofin.exe.manifest` (+ `assemblyIdentity name`).
- `resources/win/iconres.rc.in` lines 3 and 5 point at the icon and manifest by path — both must
  track the file renames. `src/jfn_rust/build.rs` expands this template; no code change needed there
  beyond the doc comments.
- `resources/win/jellyfin.ico` → `resources/win/astrofin.ico` (rename now, redraw later).

## 4e. Workflows / artifacts

Artifact *job* names (`linux-appimage-x86_64`, `macos-arm64`, `windows-x64`, …) are what the
nightly.link README URLs reference. **Keep those names unchanged** — only the `path:` globs change.
That way the README links only need the org/repo swap:

```
https://nightly.link/TheHalfrican/Astrofin/workflows/build-windows/main/windows-x64.zip
https://nightly.link/TheHalfrican/Astrofin/workflows/build-macos/main/macos-arm64.zip
https://nightly.link/TheHalfrican/Astrofin/workflows/build-linux-appimage/main/linux-appimage-x86_64.zip
https://nightly.link/TheHalfrican/Astrofin/workflows/build-linux-flatpak/main/linux-flatpak-x86_64.zip
```

Drop the AUR line (`jellium-desktop-git`) entirely — there is no Astrofin AUR package, and pointing
at the upstream one from an Astrofin README is misleading.

---

# 5. LICENSE findings and required attribution

**What the file is.** `LICENSE` is the full **GNU GPL version 2** text (header line 1:
`The GNU General Public License (GPL-2.0)`). It contains no per-project copyright header — the FSF
preamble and terms only, with the standard "how to apply" appendix at the end. Note the
inconsistency to preserve or resolve deliberately: `resources/linux/…metainfo.xml:7` declares
`<project_license>GPL-2.0-or-later</project_license>`, while `dev/linux/flatpak/LICENSE.txt` is the
**GPL-3** text. Do not "fix" these as part of the rebrand; flag them to the maintainer. (If
upstream is GPL-2-or-later, shipping GPL-3 in the flatpak is permitted; if it is GPL-2-only, that
flatpak file is wrong and predates the fork.)

**What GPL-2 requires of a renamed fork** (§2a and §1):

- §2(a): every modified file must "carry prominent notices stating that you changed the files and
  the date of any change."
- §1/§2: keep the copyright notices intact and distribute the same `LICENSE` text with the work.
- §2(c): if the program reads commands interactively, the interactive notice requirement applies —
  this app is a GUI, so the customary discharge is an About box entry, not a startup banner.
- Renaming is explicitly allowed; GPL-2 has no trademark clause. Nothing forbids calling it
  Astrofin. What is *not* optional is stating that it is a modified version of someone else's work.

**Recommended actions (minimal and sufficient):**

1. **Keep `LICENSE` byte-for-byte.** Do not edit the GPL text.
2. **Add a `NOTICE` file** at the repo root (GPL doesn't require the filename, but it is the tidiest
   home for the §2(a) notice and keeps README prose short):

   ```
   Astrofin
   Copyright (C) 2026 TheHalfrican and Astrofin contributors

   Astrofin is a modified version of Jellium Desktop
   (https://github.com/andrewrabert/jellium-desktop),
   Copyright (C) Andrew Rabert and the Jellium Desktop contributors.

   Changes from upstream include renaming the project, its executable,
   its per-user directories and its application identifiers, and adding a
   first-run migration that imports an existing Jellium Desktop profile.

   This program is free software; you can redistribute it and/or modify it
   under the terms of the GNU General Public License version 2 as published
   by the Free Software Foundation. See LICENSE for the full text.
   This program is distributed WITHOUT ANY WARRANTY.
   ```

3. **README** — put this directly under the `# Astrofin` heading, before Downloads:

   > Astrofin is an unofficial [Jellyfin](https://jellyfin.org) desktop client built on
   > [CEF](https://github.com/chromiumembedded/cef) and [mpv](https://mpv.io/).
   >
   > Astrofin is a fork of [Jellium Desktop](https://github.com/andrewrabert/jellium-desktop) by
   > Andrew Rabert, released — like the original — under the GNU General Public License v2. It is
   > not affiliated with or endorsed by the Jellium Desktop project or the Jellyfin project. See
   > [NOTICE](NOTICE) for the list of changes and [LICENSE](LICENSE) for the license text.

4. **About panel** — `src/jfn_cef/src/resource.rs::AboutData` currently carries only `app`, `cef`,
   `configDir`, `logFile`. Add a `upstream` (or `basedOn`) string field rendered by
   `src/web/about.js` as a row: `Based on   Jellium Desktop (GPL-2.0)`. This is the GUI equivalent
   of the §2(c) notice and the single most visible place the attribution belongs.
5. **AppStream `<description>`** (`resources/linux/…metainfo.xml:10`) — append one sentence:
   "Astrofin is a fork of Jellium Desktop." Flathub reviewers look for exactly this on renamed forks.
6. **Do not** put the Jellium name in the product name, the app id, the binary, or the icon — that
   would be the one thing attribution rules actually discourage (implying endorsement).

---

# 6. Risks and ordering

## 6a. Changes that MUST land in the same commit

1. **`APP_DIR_NAME` + `LOG_FILE_NAME` + the migration function + the `*_raw()` refactor.**
   Renaming the dir without the migration silently orphans `settings.json` (server URL, hwdec,
   window geometry) and the CEF login session. Renaming it *after* shipping a build that already
   created `astrofin/` would make the migration a no-op forever, because the check is
   "new dir absent". This is a one-shot window.
2. **Binary name (`[[bin]] name`) + every path that spells it.** `src/jfn_rust/Cargo.toml:12`,
   `xtask/src/build.rs:29,95,97`, `platform_{windows,linux,macos}.rs`, `bundle_macos.rs` (×3),
   `dev/**/*.just`, `dev/linux/appimage/{AppRun,container-build.sh}`, flatpak manifest 181,
   `dev/windows/build.ps1:41`. Miss one and the build stages/signs/launches a file that isn't there.
3. **`.desktop` + `.metainfo.xml` + `.svg` filenames + the `<id>` / `<launchable>` / `<icon>` values
   + `StartupWMClass` + `WM_CLASS_VALUE` + Wayland `APP_ID` + mpv `wayland-app-id`.** These form
   one consistency web; a partial rename produces a window with no icon and a failed AppStream
   validation.
4. **Flatpak app id + manifest filename + `build.sh` `MANIFEST`/`APP_ID` + `--own-name`.**
5. **AppImage interpreter path in `AppRun:15` and `container-build.sh:152`** — these two must be the
   same string or the AppImage fails to exec at all.
6. **Windows `.exe.manifest` filename + the `iconres.rc.in:5` reference to it.**
7. **MPRIS `BASE_SERVICE_NAME` + the flatpak `--own-name`.**

## 6b. Safe to defer

- ~~Artwork: `src/web/logo.png`, `resources/macos/AppIcon.icns`, `resources/win/*.ico`,
  `resources/linux/*.svg` *content*~~ — **done** on `feat/ui-assets`. `src/web/logo.png` is gone,
  replaced by `src/web/logo-mark.svg`; the icon containers are generated from the 1024 master
  `resources/brand/astrofin-icon.svg` by `dev/tools/brand/build-icons.mjs`. `about.js` now says
  `logo.alt = 'Astrofin'`.
- ~~ObjC class renames (`JellyfinApplication`, `JellyfinInputView`, …) and the Windows
  `JellyfinCefInput` class name~~ — **done** on `chore/hygiene-renames`.
- `jmp-*` CSS/DOM prefixes and `jmpInfo` / `window.jmpNative` — keep indefinitely; they are the
  jellyfin-web plugin-API compat surface inherited from jellyfin-media-player.
- `src/x11/src/shm.rs:51` memfd name — **done** (`c"astrofin-shm"`). Still open:
  `~/jellyfin-deps` CI cache dir, doc comments.
- ~~The `SetCurrentProcessExplicitAppUserModelID` hardening (§3 #19).~~ — **done** in
  `src/windows/src/platform.rs::win_early_init`.
- Fixing the missing `dev/tools/version.sh` in the two macOS workflows (pre-existing breakage,
  independent of the rebrand — but it will block any macOS CI run).

## 6c. What breaks in the user's existing setup at `%APPDATA%\jellium-desktop\mpv`

> **Superseded (feat/video-mode, feat/auto-video-mode).** The out-of-repo mode switcher is gone:
> Auto/Live-Action/Animation/Off is now
> a built-in setting (Settings -> Playback -> Video mode) that switches live through libmpv, the
> shaders ship in `resources/shaders/`, and no `mpv.conf` rewrite or app restart is involved. See
> `docs/video-modes.md`. Two rows of the table below are therefore resolved in-repo rather than by
> handing the user an updated script:
>
> - the copied `mpv.conf`'s absolute `glsl-shaders=` paths are rewritten at import time, and an
>   idempotent startup repair (`jfn_paths::repair_mpv_conf`) fixes profiles migrated before that
>   landed, leaving one `mpv.conf.bak`;
> - `switch-mode.ps1` and the two mode shortcuts no longer need Astrofin equivalents at all.
>
> Still open from this section: the `input-ipc-server` pipe name in the copied conf is left as-is
> (harmless unless both apps run at once), and decision 7.2 below ("do NOT post-process the copied
> mpv/mpv.conf") is overridden by the rewrite above.

Confirmed on this machine:

- `%APPDATA%\jellium-desktop\` — `settings.json` (serverUrl `http://192.168.50.76:8096`,
  `windowMaximized`, `hwdec: auto-copy`), `instance.json`, `mpv\`.
- `%APPDATA%\jellium-desktop\mpv\` — `mpv.conf` (generated), `shaders\`, `switch-mode.ps1`,
  `switch-mode.log`.
- `%LOCALAPPDATA%\jellium-desktop\` — the full CEF profile (`Default\`, `Local State`,
  `first_party_sets.db`, `Logs\`, shader caches). This is where the Jellyfin login lives.
- `%LOCALAPPDATA%\Programs\Jellium Desktop\jellium-desktop.exe` — the installed build.
- Start-Menu shortcuts: `Jellium Desktop.lnk`, `Jellium Desktop (Anime mode).lnk`,
  `Jellium Desktop (Movie mode).lnk` (plus older `Jellyfin Desktop*.lnk` from the prior app).

**Things outside the repo that will need updating after the rebrand** (none of them are repo edits;
list them for the implementing agent to hand back to the user):

| Item | Current | Needs |
| --- | --- | --- |
| `switch-mode.ps1:10` | `$mpvHome = "$env:APPDATA\jellium-desktop\mpv"` | A copy under `%APPDATA%\astrofin\mpv` pointing at itself. **The migration copies the script too**, so the copy will still hardcode the *old* path and keep editing Jellium's config. |
| `switch-mode.ps1:13` | `$exe = "$env:LOCALAPPDATA\Programs\Jellium Desktop\jellium-desktop.exe"` | Astrofin's install dir + `astrofin.exe`. |
| `switch-mode.ps1:14` | `$sh = $mpvHome/shaders` | follows `$mpvHome` |
| `switch-mode.ps1:36–37` and generated `mpv.conf:5–6` | `input-ipc-server=\\.\pipe\jellium-mpv` | `\\.\pipe\astrofin-mpv` — **otherwise the two installs fight over one named pipe** (§3 #26) |
| `switch-mode.ps1:48,53,54,59,67` | `Get-Process 'jellium-desktop'` | `Get-Process 'astrofin'` — as written, launching an Astrofin mode shortcut would **kill the running Jellium Desktop** |
| `switch-mode.ps1:71` | message box text "Jellium mode switch failed" | Astrofin wording |
| `mpv.conf:1` | header comment "Jellium Desktop mpv config" | cosmetic |
| Start-Menu `.lnk` files | 3 shortcuts targeting `jellium-desktop.exe` / `switch-mode.ps1` | 3 new Astrofin shortcuts; keep the Jellium ones so both remain launchable |
| Shader files | `%APPDATA%\jellium-desktop\mpv\shaders\*.glsl` | copied by the migration; `glsl-shaders=` in the generated conf is an absolute path — **the copied `mpv.conf` will still point at the Jellium shaders folder** until `switch-mode.ps1` is re-run from the new location |

**Concrete risk:** if the migration copies `mpv/` verbatim and the user then launches Astrofin, mpv
loads a `mpv.conf` that (a) claims `\\.\pipe\jellium-mpv` and (b) references
`C:/Users/NoahM/AppData/Roaming/jellium-desktop/mpv/shaders/...`. It will *work* (the shader paths
resolve, the old dir still exists), but the pipe will collide the moment both apps run. Two options
for the implementing agent:

- **Preferred:** after copying, post-process the copied `mpv/mpv.conf` — rewrite
  `\\.\pipe\jellium-mpv` → `\\.\pipe\astrofin-mpv` and any `jellium-desktop\mpv` path segment →
  `astrofin\mpv`. Keep it to a narrow, documented substitution and log what changed.
- **Alternative:** copy `mpv/` unmodified and print a one-time `warn!` naming the two lines that
  need hand-editing. Lower magic, more user friction.

Either way, hand the user an updated `switch-mode.ps1` for the new location (`$mpvHome`, `$exe`,
`Get-Process` name, pipe name, dialog text) and three new Start-Menu shortcuts. Never delete the
Jellium originals — running side by side is the whole point.

## 6d. Suggested commit order

1. `src/paths` rename + `*_raw()` refactor + `migrate_legacy()` + wire into `jfn_app_main`. Test on
   this machine with the real `%APPDATA%\jellium-desktop` before anything else lands.
2. Binary name + every build/stage/sign path (xtask, justfiles, dev scripts).
3. Runtime identity: env vars, user agents, device-profile name, window titles, MPRIS, Wayland/X11
   ids, KDE palette, idle-inhibit, IOPM assertion, NativeShell `appName`.
4. Resources + packaging: file renames, plist, rc, manifest, desktop/metainfo/svg, flatpak,
   appimage.
5. CI workflows + README links + NOTICE + About-panel attribution row.
6. Artwork and the optional ObjC/window-class hygiene renames.

---

# 7. Orchestrator decisions for the implementation agent (2026-09-07, Fable)

1. Do commits 1-5 of §6d in order, one commit each, on branch `feat/rebrand-astrofin`. Skip §6b hygiene renames (ObjC classes, JellyfinCefInput window class, memfd name, AUMID hardening) — keep the diff small for upstream-merge friction. Artwork: rename FILES only; pixels stay.
2. Migration (§2c): implement exactly as designed (staging dir + rename, symlink handling, lock/singleton denylist, per-entry tolerance, buffered report logged right after init_logging, skipped when a --config-dir/--cache-dir override is set). KEEP copying instance.json (session continuity wins). Do NOT post-process the copied mpv/mpv.conf — copy verbatim; log one info line noting that absolute paths inside mpv.conf may still reference the legacy folder. The user's mode switcher will be regenerated outside the repo.
3. Default device name: leave as-is.
4. Attribution (§5): NOTICE file, README paragraph, About-panel `basedOn` row, metainfo description sentence. LICENSE untouched. Mention the flatpak GPL-3 text mismatch in the final report only; do not change it.
5. Fix the pre-existing missing `dev/tools/version.sh` in both macOS workflows as designed (xtask version). Keep artifact job names unchanged; update README nightly.link URLs to TheHalfrican/Astrofin and drop the AUR line.
6. Windows verification is mandatory (build, run, migration against the real %APPDATA%\jellium-desktop / %LOCALAPPDATA%\jellium-desktop → %APPDATA%\astrofin / %LOCALAPPDATA%\astrofin, login session carried over, both apps launchable side by side with distinct pipes). Linux/macOS changes are text-only; make them consistent but they cannot be built here.
