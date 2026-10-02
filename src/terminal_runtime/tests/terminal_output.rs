use super::encode_console_utf16;

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn box_drawing_frame_bytes_transcode_to_utf16_instead_of_oem_bytes() {
    // `│` is E2 94 82, the sequence a CP437 console rendered as `Γöé`.
    let mut partial = Vec::new();

    let units = encode_console_utf16(&mut partial, "│ Documents".as_bytes());

    assert_eq!(units, utf16("│ Documents"));
    assert!(partial.is_empty());
}

#[test]
fn sequence_split_across_writes_is_rejoined_not_replaced() {
    let bytes = "─".as_bytes();
    let mut partial = Vec::new();

    let first = encode_console_utf16(&mut partial, &bytes[..2]);
    assert!(first.is_empty(), "incomplete sequence must not emit yet");
    assert_eq!(partial.len(), 2, "tail is held for the next write");

    let second = encode_console_utf16(&mut partial, &bytes[2..]);

    assert_eq!(second, utf16("─"));
    assert!(partial.is_empty());
}

#[test]
fn astral_glyph_encodes_as_a_surrogate_pair() {
    let mut partial = Vec::new();

    let units = encode_console_utf16(&mut partial, "🗑".as_bytes());

    assert_eq!(units.len(), 2, "non-BMP glyph needs a surrogate pair");
    assert_eq!(units, utf16("🗑"));
}

#[test]
fn malformed_byte_becomes_one_replacement_and_keeps_later_text() {
    let mut partial = Vec::new();
    let mut bytes = vec![0xFF];
    bytes.extend_from_slice("ok".as_bytes());

    let units = encode_console_utf16(&mut partial, &bytes);

    assert_eq!(units, utf16("\u{FFFD}ok"));
    assert!(partial.is_empty());
}

#[test]
fn ascii_only_output_is_unchanged() {
    let mut partial = Vec::new();

    let units = encode_console_utf16(&mut partial, b"\x1b[?1000h");

    assert_eq!(units, utf16("\x1b[?1000h"));
}
