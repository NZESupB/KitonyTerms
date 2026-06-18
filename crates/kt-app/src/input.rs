//! Translate egui keyboard input into terminal (ANSI/VT) byte sequences.
//!
//! Covers the keys an interactive shell needs: printable text, Enter, Backspace,
//! Tab, Escape, arrows, Home/End/PageUp/PageDown, Delete, function keys, and
//! Ctrl-letter control codes. Alt is sent as ESC-prefixed (the common
//! "Alt-as-Meta" convention).

use eframe::egui::{Event, Key, Modifiers};

/// Convert a batch of egui events into bytes to send to the PTY.
///
/// Returns the encoded bytes (possibly empty). `Text` events are emitted as
/// UTF-8; key events are mapped to control sequences.
pub fn events_to_bytes(events: &[Event]) -> Vec<u8> {
    let mut out = Vec::new();
    for ev in events {
        match ev {
            Event::Text(text) => {
                // egui already filters out control chars here; send as-is.
                out.extend_from_slice(text.as_bytes());
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if let Some(bytes) = key_to_bytes(*key, modifiers) {
                    out.extend_from_slice(&bytes);
                }
            }
            Event::Paste(text) => {
                out.extend_from_slice(text.as_bytes());
            }
            _ => {}
        }
    }
    out
}

/// Map a single key + modifiers to a byte sequence.
fn key_to_bytes(key: Key, mods: &Modifiers) -> Option<Vec<u8>> {
    // Ctrl-letter → control code (Ctrl-A = 0x01 … Ctrl-Z = 0x1a).
    if mods.ctrl || mods.command {
        if let Some(b) = ctrl_code(key) {
            return Some(vec![b]);
        }
    }

    let seq: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Backspace => b"\x7f",
        Key::Tab => b"\t",
        Key::Escape => b"\x1b",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::F1 => b"\x1bOP",
        Key::F2 => b"\x1bOQ",
        Key::F3 => b"\x1bOR",
        Key::F4 => b"\x1bOS",
        Key::F5 => b"\x1b[15~",
        Key::F6 => b"\x1b[17~",
        Key::F7 => b"\x1b[18~",
        Key::F8 => b"\x1b[19~",
        Key::F9 => b"\x1b[20~",
        Key::F10 => b"\x1b[21~",
        Key::F11 => b"\x1b[23~",
        Key::F12 => b"\x1b[24~",
        _ => return alt_prefixed(key, mods),
    };

    // Alt prefixes the sequence with ESC.
    if mods.alt {
        let mut v = Vec::with_capacity(seq.len() + 1);
        v.push(0x1b);
        v.extend_from_slice(seq);
        Some(v)
    } else {
        Some(seq.to_vec())
    }
}

/// Ctrl-letter and a few Ctrl-symbol control codes.
fn ctrl_code(key: Key) -> Option<u8> {
    let c = match key {
        Key::A => 0x01,
        Key::B => 0x02,
        Key::C => 0x03,
        Key::D => 0x04,
        Key::E => 0x05,
        Key::F => 0x06,
        Key::G => 0x07,
        Key::H => 0x08,
        Key::I => 0x09,
        Key::J => 0x0a,
        Key::K => 0x0b,
        Key::L => 0x0c,
        Key::M => 0x0d,
        Key::N => 0x0e,
        Key::O => 0x0f,
        Key::P => 0x10,
        Key::Q => 0x11,
        Key::R => 0x12,
        Key::S => 0x13,
        Key::T => 0x14,
        Key::U => 0x15,
        Key::V => 0x16,
        Key::W => 0x17,
        Key::X => 0x18,
        Key::Y => 0x19,
        Key::Z => 0x1a,
        Key::OpenBracket => 0x1b,  // Ctrl-[
        Key::Backslash => 0x1c,    // Ctrl-\
        Key::CloseBracket => 0x1d, // Ctrl-]
        _ => return None,
    };
    Some(c)
}

/// For keys that produce no special sequence but may carry an Alt modifier with
/// a letter (Alt+letter → ESC + letter).
fn alt_prefixed(key: Key, mods: &Modifiers) -> Option<Vec<u8>> {
    if mods.alt {
        if let Some(ch) = key.name().chars().next() {
            if key.name().len() == 1 && ch.is_ascii_alphabetic() {
                return Some(vec![0x1b, ch.to_ascii_lowercase() as u8]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn enter_is_cr() {
        assert_eq!(key_to_bytes(Key::Enter, &mods()).unwrap(), b"\r");
    }

    #[test]
    fn arrows() {
        assert_eq!(key_to_bytes(Key::ArrowUp, &mods()).unwrap(), b"\x1b[A");
        assert_eq!(key_to_bytes(Key::ArrowLeft, &mods()).unwrap(), b"\x1b[D");
    }

    #[test]
    fn ctrl_c_is_etx() {
        let m = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(key_to_bytes(Key::C, &m).unwrap(), vec![0x03]);
    }

    #[test]
    fn alt_arrow_prefixes_escape() {
        let m = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(key_to_bytes(Key::ArrowUp, &m).unwrap(), b"\x1b\x1b[A");
    }

    #[test]
    fn text_event_passthrough() {
        let evs = vec![Event::Text("ls -la".to_string())];
        assert_eq!(events_to_bytes(&evs), b"ls -la");
    }

    #[test]
    fn function_keys() {
        assert_eq!(key_to_bytes(Key::F1, &mods()).unwrap(), b"\x1bOP");
        assert_eq!(key_to_bytes(Key::F5, &mods()).unwrap(), b"\x1b[15~");
    }
}
