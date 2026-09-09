# Supply-chain checks

Two tools, one config file, three places they run (`docs/test-plan.md` phase 0).

## deny.toml

`deny.toml` sits at the **repository root**, not beside `src/Cargo.toml`:
cargo-deny searches upward from the manifest's directory, so
`cargo deny --manifest-path src/Cargo.toml check` finds it from `src/`. It
enforces:

- **advisories** — RUSTSEC vulnerabilities always fail (cargo-deny has no knob
  for them). `unsound = "all"` fails too. `unmaintained = "all"` is *reported*
  everywhere but demoted to a warning by the `-W unmaintained` flag the CI jobs
  pass, because the only current hit (RUSTSEC-2026-0192, `ttf-parser`, via
  `cosmic-text -> fontdb` in the Linux menu renderer) has no upgrade path.
  `yanked = "warn"`. `ignore` is **empty**.
- **licenses** — an allow list built from the real graph, not a template. The
  binary ships as GPL-2.0 (`LICENSE`), so the list is GPL-compatible permissive
  licences only. The single copyleft entry is a scoped exception: `MPL-2.0` for
  `option-ext` (weak, file-level, and MPL-2.0 §3.3 permits GPL distribution).
  Our own crates have no `license` field: `[licenses.private] ignore = true`.
- **bans** — `multiple-versions = "warn"` (13 duplicates today, all transitive);
  `wildcards = "deny"`, with `allow-wildcard-paths = true` for our own
  `{ path = "../x" }` links.
- **sources** — crates.io only. `Cargo.lock` has no git dependencies; CEF and
  mpv come from `cargo xtask fetch-cef` and the `third_party/mpv` submodule,
  outside cargo. A new git source is therefore a deliberate review point.

### Updating the licence allow list

Run `cargo deny --manifest-path src/Cargo.toml list --layout license`, add the
new identifier to `licenses.allow` with a comment naming the crate that drove
it, and re-run `cargo deny ... check licenses`. Anything copyleft goes in
`licenses.exceptions`, scoped to the one crate with the compatibility argument
beside it — never in the blanket allow list.

### Adding an advisory ignore

Don't, unless the fix is genuinely unavailable. Then add the ID to
`advisories.ignore` with a comment saying why it does not affect Astrofin and
which upstream issue is being waited on; an unjustified ignore is a bug.
`unused-ignored-advisory` warns once the entry is stale, so drop it then.

## Where it runs

| Where | Workflow | Runs |
|---|---|---|
| GitHub (hosted) | `.github/workflows/checks.yml` | `cargo fmt --check`, `cargo deny check -W unmaintained`, `cargo audit`, `node --test src/web/*.test.js` |
| GitHub (hosted) | `.github/workflows/codeql.yml` | CodeQL for `rust` (build-mode `none`) and `javascript-typescript`, push/PR + weekly |
| Gitea (self-hosted) | `.gitea/workflows/build-windows.yml` | the same three checks after the existing lint+test step, then the real build/installers |

CodeQL's Rust database is built with `build-mode: none`, so it needs no libmpv
or CEF; `third_party/`, `build/`, `dist/` and `src/target` are excluded. clippy
and `cargo test` stay off the hosted runners — they link the workspace. The
self-hosted runner uses this PC's toolchain and therefore whatever
`cargo install --locked cargo-deny cargo-audit` last installed into
`%USERPROFILE%\.cargo\bin`; the hosted job installs both per run. Locally:
`just deny` and `just audit` (and `just test-js`).
