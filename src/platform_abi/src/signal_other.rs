#[derive(Default)]
pub struct SignalGuard;

impl SignalGuard {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guard_is_free_to_take_where_there_are_no_signals() {
        // The Unix guard snapshots SIGINT/SIGTERM and restores them on drop;
        // on a platform with no `sigaction` there is nothing to save, so the
        // guard carries no state at all.
        let guard = SignalGuard::new();
        assert_eq!(std::mem::size_of_val(&guard), 0);
    }

    #[test]
    fn taking_more_than_one_guard_is_harmless() {
        // `jfn_cef_initialize` takes one per init; nesting must not conflict.
        let outer = SignalGuard::new();
        let inner = SignalGuard::new();
        assert_eq!(std::mem::size_of_val(&outer), std::mem::size_of_val(&inner));
    }
}
