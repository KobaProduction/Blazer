use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunAdapterKind {
    WireGuard,
    OpenVpn,
    WindowsVpn,
    Tunnel,
}

/// Local interface metadata, explicitly accessed for actionable UI errors.
/// Debug deliberately omits machine-specific interface names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TunConflict {
    name: [u8; 64],
    name_len: u8,
    description: [u8; 48],
    description_len: u8,
    kind: TunAdapterKind,
}

impl TunConflict {
    pub fn new(name: &str, description: &str, kind: TunAdapterKind) -> Self {
        let (name, name_len) = bounded_text(name);
        let (description, description_len) = bounded_text(description);
        Self {
            name,
            name_len,
            description,
            description_len,
            kind,
        }
    }

    pub fn name(&self) -> &str {
        std::str::from_utf8(&self.name[..usize::from(self.name_len)]).unwrap_or_default()
    }

    pub fn description(&self) -> &str {
        std::str::from_utf8(&self.description[..usize::from(self.description_len)])
            .unwrap_or_default()
    }

    pub fn kind(&self) -> TunAdapterKind {
        self.kind
    }
}

fn bounded_text<const N: usize>(value: &str) -> ([u8; N], u8) {
    let mut bytes = [0; N];
    let mut used = 0;
    for character in value.chars().filter(|c| {
        !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }) {
        if used + character.len_utf8() > bytes.len() {
            break;
        }
        character.encode_utf8(&mut bytes[used..]);
        used += character.len_utf8();
    }
    (bytes, used as u8)
}

impl fmt::Debug for TunConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TunConflict([local interface])")
    }
}
