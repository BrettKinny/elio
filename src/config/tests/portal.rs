use super::super::portal::PortalTerminal;
use super::super::*;

#[test]
fn portal_terminal_accepts_the_canonical_names() {
    let cases = [
        ("kitty", PortalTerminal::Kitty),
        ("ghostty", PortalTerminal::Ghostty),
        ("foot", PortalTerminal::Foot),
        ("wezterm", PortalTerminal::WezTerm),
        ("alacritty", PortalTerminal::Alacritty),
        ("rio", PortalTerminal::Rio),
        ("konsole", PortalTerminal::Konsole),
        ("gnome-terminal", PortalTerminal::Gnome),
        ("xterm", PortalTerminal::Xterm),
    ];

    for (name, terminal) in cases {
        let config = Config::from_str(&format!("[portal]\nterminal = {name:?}"))
            .expect("supported portal terminal should parse");
        assert_eq!(config.portal.terminal, Some(terminal));
    }
}

#[test]
fn portal_terminal_is_optional() {
    assert_eq!(Config::from_str("").unwrap().portal.terminal, None);
    assert_eq!(
        Config::from_str("[portal]\n").unwrap().portal.terminal,
        None
    );
}

#[test]
fn portal_terminal_rejects_unknown_values() {
    let error = match Config::from_str("[portal]\nterminal = \"warp\"") {
        Ok(_) => panic!("unknown portal terminal should be rejected"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("unknown variant `warp`"));
    assert!(error.contains("kitty"));
    assert!(error.contains("gnome-terminal"));
}
