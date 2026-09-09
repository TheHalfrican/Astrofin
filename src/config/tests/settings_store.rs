//! The process-global settings store.
//!
//! `settings_init` binds the store's path once per process and the save
//! worker is a process-wide singleton, so this binary holds exactly one test
//! and walks the lifecycle in order. A second test function would race it,
//! and it would have no way to rebind the path.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::Path;

fn text(path: &Path) -> String {
    fs::read_to_string(path).expect("read settings.json")
}

#[test]
fn the_store_binds_one_path_loads_saves_and_drains_its_worker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("settings.json");
    let decoy = tmp.path().join("decoy.json");
    fs::write(
        &path,
        r#"{"serverUrl":"http://seed","hwdec":"vaapi","deviceName":"  seed  box  ",
            "videoModeLibraries":{"lib":"animation"},"unknownKey":[1,2,3],
            "windowWidth":"not a number"}"#,
    )
    .expect("seed");

    jfn_config::settings_init(&path);
    // Idempotent: the store keeps the first path it was given.
    jfn_config::settings_init(&decoy);

    // 1. Load. Unknown and wrong-typed keys do not fail the file.
    assert!(jfn_config::settings_load());
    assert_eq!(jfn_config::server_url(), "http://seed");
    assert_eq!(jfn_config::hwdec(), "vaapi");
    assert_eq!(jfn_config::device_name(), "  seed  box  ");
    assert_eq!(jfn_config::window_geometry().width, 0);
    assert!(!jfn_config::video_mode_migrated());

    // 2. The accessors round-trip through the store.
    jfn_config::set_server_url("http://changed");
    jfn_config::set_video_mode("animation");
    jfn_config::set_transcode_notice("any");
    jfn_config::set_audio_passthrough("eac3");
    jfn_config::set_audio_channels("stereo");
    jfn_config::set_log_level("debug");
    jfn_config::set_hwdec("no");
    jfn_config::set_audio_exclusive(true);
    jfn_config::set_disable_gpu_compositing(true);
    jfn_config::set_transparent_titlebar(false);
    jfn_config::set_force_transcoding(true);
    jfn_config::set_hide_scrollbar(false);
    jfn_config::set_window_decorations(Some("serverThemed"));
    jfn_config::set_device_name("  My   Box \n", "platform-host");
    jfn_config::set_window_geometry(jfn_config::JfnWindowGeometry {
        x: 10,
        y: 20,
        width: 640,
        height: 480,
        logical_width: 320,
        logical_height: 240,
        scale: 2.0,
        maximized: true,
    });

    assert_eq!(jfn_config::video_mode(), "animation");
    assert_eq!(jfn_config::transcode_notice(), "any");
    assert_eq!(jfn_config::audio_passthrough(), "eac3");
    assert_eq!(jfn_config::audio_channels(), "stereo");
    assert_eq!(jfn_config::log_level(), "debug");
    assert!(jfn_config::audio_exclusive());
    assert!(jfn_config::disable_gpu_compositing());
    assert!(!jfn_config::transparent_titlebar());
    assert!(jfn_config::force_transcoding());
    assert!(!jfn_config::hide_scrollbar());
    assert!(jfn_config::configured_window_decorations().is_some());
    // Whitespace is folded before the name ever reaches an auth header.
    assert_eq!(jfn_config::device_name(), "My Box");
    assert_eq!(jfn_config::window_geometry().width, 640);

    // 3. Synchronous save, to the bound path and nowhere else.
    assert!(jfn_config::settings_save());
    assert!(!decoy.exists(), "settings_init was not idempotent");
    let saved = text(&path);
    assert!(saved.contains("http://changed"), "{saved}");
    assert!(saved.ends_with("}\n"), "{saved:?}");

    // 4. The web UI blob reflects the same state and is valid JSON.
    let cli: serde_json::Value =
        serde_json::from_str(&jfn_config::cli_json(&["no", "auto"])).expect("cli_json is json");
    assert_eq!(cli["videoMode"].as_str(), Some("animation"));
    assert_eq!(cli["deviceName"].as_str(), Some("My Box"));
    assert_eq!(cli["forceTranscoding"].as_bool(), Some(true));

    // 5. Async saves coalesce, and the shutdown drains the newest one.
    jfn_config::set_server_url("http://async-1");
    jfn_config::settings_save_async();
    jfn_config::set_server_url("http://async-2");
    jfn_config::settings_save_async();
    jfn_config::settings_shutdown_save_worker();
    assert!(text(&path).contains("http://async-2"));

    // 6. After the shutdown the async path is a no-op — documented, and the
    //    reason the app calls it last — while the synchronous save still
    //    works. Shutting down twice is safe.
    jfn_config::set_server_url("http://after-shutdown");
    jfn_config::settings_save_async();
    jfn_config::settings_shutdown_save_worker();
    assert!(!text(&path).contains("http://after-shutdown"));
    assert!(jfn_config::settings_save());
    assert!(text(&path).contains("http://after-shutdown"));

    // 7. Reload sees what was written, and every file this build writes
    //    carries the video-mode migration marker.
    assert!(jfn_config::settings_load());
    assert_eq!(jfn_config::server_url(), "http://after-shutdown");
    assert!(jfn_config::video_mode_migrated());

    // 8. A settings.json that cannot be parsed leaves the in-memory state
    //    alone and says so.
    fs::write(&path, b"{ truncated").expect("write");
    assert!(!jfn_config::settings_load());
    assert_eq!(jfn_config::server_url(), "http://after-shutdown");

    fs::remove_file(&path).expect("remove");
    assert!(!jfn_config::settings_load());
    assert_eq!(jfn_config::server_url(), "http://after-shutdown");
}
