//! X11 platform subsystem: surface management, input thread, Platform impl.

#![cfg(target_os = "linux")]

pub(crate) mod conn_source;
pub(crate) mod conn_source_logic;
pub mod geometry;
pub(crate) mod geometry_logic;
pub(crate) mod input;
pub(crate) mod input_lifecycle;
pub(crate) mod input_logic;
pub mod lifecycle;
pub(crate) mod lifecycle_logic;
pub mod make_platform;
pub(crate) mod make_platform_logic;
pub(crate) mod menu;
pub(crate) mod mpv_host;
pub(crate) mod mpv_proxy;
pub(crate) mod mpv_proxy_logic;
pub(crate) mod overlay_actor;
pub(crate) mod overlay_actor_logic;
pub mod overlay_fsm;
pub(crate) mod paint;
pub(crate) mod paint_logic;
pub mod paint_override;
pub(crate) mod registry;
pub(crate) mod scale;
pub(crate) mod scale_logic;
pub mod shm;
pub(crate) mod shm_logic;
pub mod surface;
pub(crate) mod window_source;
pub(crate) mod x11_state;

pub use paint_override::{X11PaintOverride, paint_override, set_paint_override};
