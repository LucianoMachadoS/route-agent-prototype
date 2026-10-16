//! Decoding output captured from Windows console programs.
//!
//! `schtasks.exe`, `ping.exe` and Windows PowerShell's error stream write
//! in the console's OEM code page (CP850 on pt-BR Windows), not UTF-8.
//! Run through `String::from_utf8_lossy`, every accented letter turns
//! into U+FFFD - which is how "Não é possível" ended up in
//! watch-game.log as "N�o � poss�vel".

/// Returns `bytes` as-is if they're valid UTF-8, otherwise decodes them
/// as CP850. Only the letters Portuguese actually uses are mapped; any
/// other non-ASCII byte becomes '?'.
pub fn decode(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    bytes.iter().map(|&b| cp850(b)).collect()
}

fn cp850(b: u8) -> char {
    match b {
        0x00..=0x7f => b as char,
        0x80 => 'Ç',
        0x81 => 'ü',
        0x82 => 'é',
        0x83 => 'â',
        0x85 => 'à',
        0x87 => 'ç',
        0x88 => 'ê',
        0x8a => 'è',
        0x90 => 'É',
        0x93 => 'ô',
        0xa0 => 'á',
        0xa1 => 'í',
        0xa2 => 'ó',
        0xa3 => 'ú',
        0xb5 => 'Á',
        0xb6 => 'Â',
        0xb7 => 'À',
        0xc6 => 'ã',
        0xc7 => 'Ã',
        0xd2 => 'Ê',
        0xd6 => 'Í',
        0xe0 => 'Ó',
        0xe2 => 'Ô',
        0xe4 => 'õ',
        0xe5 => 'Õ',
        0xe9 => 'Ú',
        _ => '?',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_utf8_through_unchanged() {
        assert_eq!(decode("Em execução".as_bytes()), "Em execução");
    }

    #[test]
    fn decodes_cp850_portuguese() {
        // "Não é possível" / "Em execução" as schtasks/PowerShell emit them.
        let nao = [
            b'N', 0xc6, b'o', b' ', 0x82, b' ', b'p', b'o', b's', b's', 0xa1, b'v', b'e', b'l',
        ];
        assert_eq!(decode(&nao), "Não é possível");
        let exec = b"Em execu\x87\xc6o";
        assert_eq!(decode(exec), "Em execução");
    }
}
