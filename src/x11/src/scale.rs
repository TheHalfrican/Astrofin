//! X11 display scale probe: the app's scale authority.
//!
//! The app owns geometry and scale on X11 (mpv is embedded and passive), so
//! this probe defines the logical ↔ physical conversion everywhere: boot
//! restore, persist, CEF device scale, and input mapping. The arithmetic —
//! including the Xft.dpi half-step quantization that matches mpv's historical
//! behavior so saved logical sizes round-trip across the ownership change —
//! lives in [`crate::scale_logic`], which is where the tests pin it. What is
//! left here is the connection: the resource database and the screen geometry.

use x11rb::connection::Connection;
use x11rb::resource_manager::new_from_resource_manager;
use x11rb::rust_connection::RustConnection;

use crate::scale_logic::{quantize_dpi, screen_dpi_scale};

pub(crate) fn query_display_scale() -> Option<f32> {
    // Explicitly target the real server: while the mpv proxy has DISPLAY
    // repointed, env-based connect would route through it.
    let display = crate::mpv_proxy::real_display();
    let (conn, screen_num) = RustConnection::connect(display.as_deref()).ok()?;
    if let Some(scale) = query_xft_dpi_scale(&conn) {
        tracing::debug!(target: "x11::scale", "Using Xft.dpi scale: {scale}");
        return Some(scale);
    }
    if let Some(scale) = query_screen_dpi_scale(&conn, screen_num) {
        tracing::debug!(target: "x11::scale", "Using X11 screen DPI scale: {scale}");
        return Some(scale);
    }
    None
}

fn query_xft_dpi_scale(conn: &impl Connection) -> Option<f32> {
    let db = new_from_resource_manager(conn).ok().flatten()?;
    let value: i64 = db.get_value("Xft.dpi", "").ok().flatten()?;
    quantize_dpi(value as f64)
}

fn query_screen_dpi_scale(conn: &impl Connection, screen_num: usize) -> Option<f32> {
    let screen = conn.setup().roots.get(screen_num)?;
    screen_dpi_scale(
        screen.width_in_pixels,
        screen.height_in_pixels,
        screen.width_in_millimeters,
        screen.height_in_millimeters,
    )
}
