//! Process-lifetime state for the browser process. The configuration
//! setters (log_severity, platform_switches, etc.) write here between
//! `Start()` and `Initialize()`; the App handlers read here.

use parking_lot::Mutex;

pub struct PendingSwitch {
    pub name: String,
    pub value: Option<String>,
}

impl PendingSwitch {
    pub fn flag(name: &str) -> Self {
        Self {
            name: name.into(),
            value: None,
        }
    }

    pub fn with_value(name: &str, value: &str) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
        }
    }
}

#[derive(Default)]
pub struct Config {
    pub log_severity: i32,
    pub remote_debugging_port: i32,
    pub pending_switches: Vec<PendingSwitch>,
    pub on_context_initialized: Option<extern "C" fn()>,
}

static CONFIG: Mutex<Config> = Mutex::new(Config {
    log_severity: 0,
    remote_debugging_port: 0,
    pending_switches: Vec::new(),
    on_context_initialized: None,
});

pub fn with_config<R>(f: impl FnOnce(&mut Config) -> R) -> R {
    let mut g = CONFIG.lock();
    f(&mut g)
}

pub fn snapshot_switches() -> Vec<PendingSwitch> {
    with_config(|c| {
        c.pending_switches
            .iter()
            .map(|s| PendingSwitch {
                name: s.name.clone(),
                value: s.value.clone(),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// `CONFIG` is process-wide, so every test here serialises on this lock
    /// and restores the config it found.
    static CONFIG_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_config<R>(f: impl FnOnce() -> R) -> R {
        let _g = CONFIG_LOCK.lock();
        let saved = with_config(|c| {
            let taken = Config {
                log_severity: c.log_severity,
                remote_debugging_port: c.remote_debugging_port,
                pending_switches: std::mem::take(&mut c.pending_switches),
                on_context_initialized: c.on_context_initialized,
            };
            c.log_severity = 0;
            c.remote_debugging_port = 0;
            c.on_context_initialized = None;
            taken
        });
        let out = f();
        with_config(|c| {
            c.log_severity = saved.log_severity;
            c.remote_debugging_port = saved.remote_debugging_port;
            c.pending_switches = saved.pending_switches;
            c.on_context_initialized = saved.on_context_initialized;
        });
        out
    }

    #[test]
    fn pending_switch_flag_carries_no_value() {
        let s = PendingSwitch::flag("disable-gpu");
        assert_eq!(s.name, "disable-gpu");
        assert_eq!(s.value, None);
    }

    #[test]
    fn pending_switch_with_value_keeps_both_halves() {
        let s = PendingSwitch::with_value("use-angle", "d3d11");
        assert_eq!(s.name, "use-angle");
        assert_eq!(s.value.as_deref(), Some("d3d11"));
    }

    #[test]
    fn pending_switch_with_value_accepts_an_empty_value() {
        // An empty value is distinct from a bare flag on the CEF command
        // line, so it must survive as `Some("")`.
        let s = PendingSwitch::with_value("lang", "");
        assert_eq!(s.value.as_deref(), Some(""));
    }

    #[test]
    fn with_config_hands_out_the_same_mutable_config_every_time() {
        with_clean_config(|| {
            with_config(|c| {
                c.log_severity = 3;
                c.remote_debugging_port = 9222;
            });
            let (sev, port) = with_config(|c| (c.log_severity, c.remote_debugging_port));
            assert_eq!(sev, 3);
            assert_eq!(port, 9222);
        });
    }

    #[test]
    fn with_config_returns_the_closure_result() {
        with_clean_config(|| {
            assert_eq!(with_config(|_| 42), 42);
        });
    }

    #[test]
    fn snapshot_switches_copies_the_pending_list_without_draining_it() {
        with_clean_config(|| {
            with_config(|c| {
                c.pending_switches.push(PendingSwitch::flag("no-sandbox"));
                c.pending_switches
                    .push(PendingSwitch::with_value("use-angle", "d3d11"));
            });
            let snap = snapshot_switches();
            assert_eq!(snap.len(), 2);
            assert_eq!(snap[0].name, "no-sandbox");
            assert_eq!(snap[0].value, None);
            assert_eq!(snap[1].name, "use-angle");
            assert_eq!(snap[1].value.as_deref(), Some("d3d11"));
            // The switches are applied on every process launch, so the
            // snapshot must not consume them.
            assert_eq!(with_config(|c| c.pending_switches.len()), 2);
        });
    }

    #[test]
    fn snapshot_switches_is_empty_before_anything_is_registered() {
        with_clean_config(|| {
            assert!(snapshot_switches().is_empty());
        });
    }

    #[test]
    fn the_default_config_has_no_severity_port_or_callback() {
        let c = Config::default();
        assert_eq!(c.log_severity, 0);
        assert_eq!(c.remote_debugging_port, 0);
        assert!(c.pending_switches.is_empty());
        assert!(c.on_context_initialized.is_none());
    }
}
