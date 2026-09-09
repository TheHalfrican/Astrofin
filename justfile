set dotenv-load := true

export ASTROFIN_LOG_LEVEL := env_var_or_default("ASTROFIN_LOG_LEVEL", "debug")
export ASTROFIN_LOG_FILE := env_var_or_default("ASTROFIN_LOG_FILE", "build/run.log")
export JFN_MPV_INCLUDE_DIR := "third_party/mpv/include"

import 'dev/linux/linux.just'
import 'dev/macos/macos.just'
import 'dev/windows/windows.just'

# List recipes
[private]
list:
    @just --list --unsorted

# List outdated dependencies
[group('maintenance')]
outdated:
    cargo outdated --manifest-path src/Cargo.toml --workspace --root-deps-only

# Remove build artifacts
[group('maintenance')]
[macos]
[linux]
clean:
    rm -rf build dist
    cargo clean --manifest-path src/Cargo.toml

# Remove build artifacts
[group('maintenance')]
[windows]
clean:
    if (Test-Path build) { Remove-Item -Recurse -Force build }
    if (Test-Path dist) { Remove-Item -Recurse -Force dist }
    cargo clean --manifest-path src/Cargo.toml

# Run tests
[group('test')]
[unix]
test: build
    cargo test --manifest-path src/Cargo.toml --workspace

# Run tests (loads MSVC + bindgen libclang env via dev/windows/env.ps1)
[group('test')]
[windows]
test: build
    . 'dev/windows/env.ps1'; cargo test --manifest-path src/Cargo.toml --workspace

# Run the JS unit tests (node only; not part of `just test`, which is cargo)
#
# The glob is quoted so the shell leaves it alone: node >= 21 expands test
# globs itself, and PowerShell does not glob native-command arguments at all,
# so the one form works on every OS.
[group('test')]
test-js:
    node --test "src/web/*.test.js"

# Tests per public function, per crate and in total (docs/test-plan.md §2)
[group('test')]
test-ratio *args:
    cargo xtask test-ratio {{args}}

# Fail when the total ratio drops below dev/test-ratio-floor.txt
[group('test')]
test-ratio-check:
    cargo xtask test-ratio --check

# Line coverage — a diagnostic, never a gate; the ratio above is the figure
# the plan tracks. Needs `cargo install cargo-llvm-cov` (it rebuilds the whole
# workspace with instrumentation, so expect a long first run).
[group('test')]
[unix]
coverage:
    cargo llvm-cov --manifest-path src/Cargo.toml --workspace --html --output-dir build/coverage

# Line coverage (loads MSVC + bindgen libclang env via dev/windows/env.ps1)
[group('test')]
[windows]
coverage:
    . 'dev/windows/env.ps1'; cargo llvm-cov --manifest-path src/Cargo.toml --workspace --html --output-dir build/coverage

# Format workspace
[group('lint')]
fmt:
    cargo fmt --manifest-path src/Cargo.toml --all

# Check formatting
[group('lint')]
fmt-check:
    cargo fmt --manifest-path src/Cargo.toml --all -- --check

# Run clippy
[group('lint')]
[unix]
clippy:
    cargo clippy --manifest-path src/Cargo.toml --workspace --all-targets -- \
        -D warnings \
        -D clippy::unwrap_used \
        -D clippy::expect_used \
        -D clippy::panic

# Run clippy (loads MSVC + bindgen libclang env via dev/windows/env.ps1)
[group('lint')]
[windows]
clippy:
    . 'dev/windows/env.ps1'; \
    cargo clippy --manifest-path src/Cargo.toml --workspace --all-targets -- \
        -D warnings \
        -D clippy::unwrap_used \
        -D clippy::expect_used \
        -D clippy::panic

# Advisory database check (needs `cargo install cargo-audit`)
[group('lint')]
audit:
    cargo audit --file src/Cargo.lock

# Licence / advisory / duplicate-crate policy (needs `cargo install cargo-deny`)
[group('lint')]
deny:
    cargo deny --manifest-path src/Cargo.toml check -W unmaintained

# Lint workspace
[group('lint')]
lint: fmt-check clippy

# Strict lint workspace
[group('lint')]
strict-lint:
    cargo fmt --manifest-path src/Cargo.toml --all -- --check
    cargo clippy --manifest-path src/Cargo.toml --workspace --all-targets -- \
        -D warnings \
        -D clippy::pedantic \
        -D clippy::nursery \
        -D clippy::unwrap_used \
        -D clippy::expect_used \
        -D clippy::panic
