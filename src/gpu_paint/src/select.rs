//! Which adapter to open, as rules over adapter *descriptions* rather than
//! over adapters.
//!
//! `context.rs` does the enumeration and the device open; the two questions it
//! asks of each adapter — is this one usable at all, and is it better than the
//! one I have — are here, where they are input -> output.

/// True for an adapter worth opening a device on. A software rasterizer or an
/// adapter wgpu cannot classify presents CEF's overlay at a few frames a
/// second, which is worse than not compositing at all.
pub(crate) fn is_usable(device_type: wgpu::DeviceType) -> bool {
    !matches!(device_type, wgpu::DeviceType::Cpu | wgpu::DeviceType::Other)
}

/// Preference order among usable adapters, highest first. Only consulted when
/// the producer's own adapter is unknown or absent — a matched producer wins
/// over any ranking, because an import from a different GPU fails at bind.
pub(crate) fn rank(device_type: wgpu::DeviceType) -> u8 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 3,
        wgpu::DeviceType::IntegratedGpu => 2,
        wgpu::DeviceType::VirtualGpu => 1,
        _ => 0,
    }
}

/// Whether the adapter that was opened counts as the producer's.
///
/// With no producer to match against, the best adapter is as good as it gets
/// and counts as matched; a producer that was named and then not found does
/// not — importing its buffers would fail.
pub(crate) fn matched_without_producer(producer_named: bool) -> bool {
    !producer_named
}

/// Importing needs both halves: this device's import path must be live, and it
/// must be the same device the producer allocates on.
pub(crate) fn can_import(import_capable: bool, device_matched: bool) -> bool {
    import_capable && device_matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::DeviceType;

    #[test]
    fn real_gpus_are_usable() {
        assert!(is_usable(DeviceType::DiscreteGpu));
        assert!(is_usable(DeviceType::IntegratedGpu));
        assert!(is_usable(DeviceType::VirtualGpu));
    }

    #[test]
    fn software_and_unclassified_adapters_are_not_usable() {
        assert!(!is_usable(DeviceType::Cpu));
        assert!(!is_usable(DeviceType::Other));
    }

    #[test]
    fn a_discrete_gpu_outranks_every_other_kind() {
        assert!(rank(DeviceType::DiscreteGpu) > rank(DeviceType::IntegratedGpu));
        assert!(rank(DeviceType::IntegratedGpu) > rank(DeviceType::VirtualGpu));
        assert!(rank(DeviceType::VirtualGpu) > rank(DeviceType::Cpu));
    }

    #[test]
    fn unusable_adapters_all_rank_last() {
        assert_eq!(rank(DeviceType::Cpu), 0);
        assert_eq!(rank(DeviceType::Other), 0);
    }

    #[test]
    fn an_unnamed_producer_makes_the_chosen_adapter_the_matched_one() {
        assert!(matched_without_producer(false));
    }

    #[test]
    fn a_named_but_unfound_producer_leaves_the_adapter_unmatched() {
        assert!(!matched_without_producer(true));
    }

    #[test]
    fn importing_needs_a_live_path_on_the_producers_own_device() {
        assert!(can_import(true, true));
        assert!(!can_import(true, false));
        assert!(!can_import(false, true));
        assert!(!can_import(false, false));
    }
}
