//! CLI-driven X11 paint preference. Must be set before `early_init`, since the
//! backing `OnceLock` ignores later writes.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11PaintOverride {
    Dmabuf,
    Gpu,
    Shm,
}

static OVERRIDE: OnceLock<X11PaintOverride> = OnceLock::new();

/// Set the override. No-op if called twice.
pub fn set_paint_override(mode: X11PaintOverride) {
    let _ = OVERRIDE.set(mode);
}

pub fn paint_override() -> Option<X11PaintOverride> {
    OVERRIDE.get().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The override is a process-global `OnceLock`, so only the first write in
    // this binary can ever land. Both tests therefore write `Gpu` first and
    // assert against `Gpu`, which holds in either order and interleaved.
    #[test]
    fn a_set_override_is_what_the_reader_sees() {
        set_paint_override(X11PaintOverride::Gpu);
        assert_eq!(paint_override(), Some(X11PaintOverride::Gpu));
    }

    #[test]
    fn a_second_override_never_replaces_the_first() {
        set_paint_override(X11PaintOverride::Gpu);
        set_paint_override(X11PaintOverride::Shm);
        assert_eq!(paint_override(), Some(X11PaintOverride::Gpu));
    }
}
