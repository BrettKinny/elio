use super::super::*;

#[test]
fn audio_preview_defaults_to_tap_with_path_placeholder() {
    let config = Config::default_config();
    assert_eq!(config.audio_preview.command, "tap {path}");
    assert_eq!(config.audio_preview.program_name(), Some("tap"));
    assert_eq!(
        config
            .audio_preview
            .command_for_path(std::path::Path::new("/music/album/song.mp3")),
        "tap /music/album/song.mp3"
    );
}

#[test]
fn audio_preview_command_can_be_overridden() {
    let config = Config::from_str(
        r#"
[audio_preview]
command = "trix {path}"
"#,
    )
    .expect("config should parse");
    assert_eq!(config.audio_preview.command, "trix {path}");
    assert_eq!(config.audio_preview.program_name(), Some("trix"));
}

#[test]
fn audio_preview_without_placeholder_appends_path() {
    let config = Config::from_str(
        r#"
[audio_preview]
command = "mpv"
"#,
    )
    .expect("config should parse");
    assert_eq!(
        config
            .audio_preview
            .command_for_path(std::path::Path::new("/music/song.mp3")),
        "mpv"
    );
}

#[test]
fn audio_preview_empty_command_falls_back_to_default() {
    let config = Config::from_str(
        r#"
[audio_preview]
command = ""
"#,
    )
    .expect("config should parse");
    assert_eq!(config.audio_preview.command, "tap {path}");
}

#[test]
fn audio_preview_unknown_keys_are_ignored() {
    let config = Config::from_str(
        r#"
[audio_preview]
command = "tap {path}"
player = "not-a-real-key"
"#,
    )
    .expect("config should parse");
    assert_eq!(config.audio_preview.command, "tap {path}");
}
