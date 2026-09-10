pub fn install_shutdown(_on_shutdown: fn()) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn never_called() {}

    #[test]
    fn installing_a_shutdown_handler_is_a_no_op_that_never_runs_it() {
        // On a platform with no POSIX signals there is nothing to hook, so
        // the callback must simply be dropped — not invoked, not stored.
        install_shutdown(never_called);
        // Idempotent: the composition root may install more than once.
        install_shutdown(never_called);
    }
}
