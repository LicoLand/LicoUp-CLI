//! Platform-neutral ANSI escape-sequence stripping shared by CLI agent lanes.
//!
//! The PTY foundation itself is Unix-only, but agent output parsing runs on
//! every supported platform, so the stripper lives here instead of inside
//! native PTY transport.

/// Incremental ANSI escape-sequence stripper.
///
/// Not a terminal emulator: cursor movement, scrolling and alternate screens
/// are not interpreted. CSI / OSC / DCS / PM / APC / intermediate sequences
/// and single-char escapes are dropped, CR bytes are removed, and everything
/// else passes through. Escape sequences and multibyte UTF-8 characters may
/// span `push` calls; the concatenation of all `push`/`finish` returns is
/// byte-exact for valid UTF-8.
///
/// Every sequence is introduced by `ESC`. The single-byte C1 forms (0x80-0x9F)
/// are deliberately never introducers: this stream is UTF-8, where those bytes
/// are continuation bytes. Treating one as a sequence start both truncates the
/// character it belongs to and swallows every byte up to the next 0x40-0x7E.
/// Common text contains such bytes — `回` is E5 9B 9E, `集` is E9 9B 86, `盖`
/// is E7 9B 96 — so the damage lands in ordinary prose, not just in output that
/// happens to carry escape sequences.
pub struct AnsiStripper {
    state: StripState,
    out: Vec<u8>,
    flushed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StripState {
    Ground,
    Escape,
    Csi,
    Osc,
    Dcs,
    Other,
}

impl AnsiStripper {
    pub fn new() -> Self {
        Self {
            state: StripState::Ground,
            out: Vec::new(),
            flushed: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> String {
        for &byte in bytes {
            self.step(byte);
        }
        let cut = self.utf8_cut();
        let text = String::from_utf8_lossy(&self.out[self.flushed..cut]).into_owned();
        self.flushed = cut;
        text
    }

    pub fn finish(&mut self) -> String {
        self.state = StripState::Ground;
        let text = String::from_utf8_lossy(&self.out[self.flushed..]).into_owned();
        self.flushed = self.out.len();
        text
    }

    fn step(&mut self, byte: u8) {
        match self.state {
            StripState::Ground => match byte {
                0x1B => self.state = StripState::Escape,
                0x0D => {} // drop CR
                _ => self.out.push(byte),
            },
            StripState::Escape => match byte {
                b'[' => self.state = StripState::Csi,
                b']' => self.state = StripState::Osc,
                b'P' | b'^' | b'_' => self.state = StripState::Dcs,
                b'\\' => self.state = StripState::Ground,
                0x20..=0x2F => self.state = StripState::Other,
                // Single-char escapes (7 8 D E M c = >) — dropped.
                _ => self.state = StripState::Ground,
            },
            StripState::Csi => {
                if (0x40..=0x7E).contains(&byte) {
                    self.state = StripState::Ground;
                }
            }
            StripState::Osc => match byte {
                0x07 => self.state = StripState::Ground,
                // ESC \ (ST) closes an OSC.
                0x1B => self.state = StripState::Escape,
                _ => {}
            },
            StripState::Dcs => {
                if byte == 0x1B {
                    self.state = StripState::Escape;
                }
            }
            StripState::Other => {
                if (0x30..=0x7E).contains(&byte) {
                    self.state = StripState::Ground;
                }
            }
        }
    }

    /// Holds back an incomplete trailing UTF-8 sequence so a chunk boundary
    /// never splits a codepoint.
    fn utf8_cut(&self) -> usize {
        let len = self.out.len();
        if len == self.flushed {
            return len;
        }
        let mut end = len;
        while end > self.flushed && (0x80..=0xBF).contains(&self.out[end - 1]) {
            end -= 1;
        }
        if end == self.flushed {
            return len;
        }
        let lead = self.out[end - 1];
        let expected = match lead {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            // Stray or overlong lead byte: emit and let lossy conversion cope.
            _ => return len,
        };
        let trailing = len - end;
        if trailing + 1 >= expected {
            len
        } else {
            end - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AnsiStripper;

    #[test]
    fn strips_csi_osc_and_cr() {
        let mut stripper = AnsiStripper::new();
        let text = stripper.push(b"\x1b[32mhello\x1b[0m\r\n\x1b]0;title\x07world\x1b(B\n");
        assert_eq!(text, "hello\nworld\n");
        assert_eq!(stripper.finish(), "");
    }

    #[test]
    fn handles_sequences_split_across_pushes() {
        let mut stripper = AnsiStripper::new();
        assert_eq!(stripper.push(b"\x1b[3"), "");
        assert_eq!(stripper.push(b"2mred\x1b[0"), "red");
        assert_eq!(stripper.push(b"m\n"), "\n");
        assert_eq!(stripper.finish(), "");
    }

    #[test]
    fn finish_drops_incomplete_escape_tail() {
        let mut stripper = AnsiStripper::new();
        assert_eq!(stripper.push(b"\x1b[31mhi"), "hi");
        assert_eq!(stripper.finish(), "");
    }

    #[test]
    fn preserves_multibyte_utf8_across_pushes() {
        let mut stripper = AnsiStripper::new();
        let bytes = "héllo".as_bytes();
        let first = stripper.push(&bytes[..3]);
        let second = stripper.push(&bytes[3..]);
        assert_eq!(format!("{first}{second}"), "héllo");
        assert!(!first.ends_with('\u{FFFD}'));
    }

    /// A UTF-8 continuation byte is not a control introducer.
    ///
    /// `0x9B` is the second byte of `回` (E5 9B 9E), `集` (E9 9B 86) and `盖`
    /// (E7 9B 96), and the third byte of `；` (EF BC 9B). Reading it as a C1 CSI
    /// truncates that character and then swallows every following byte up to the
    /// next 0x40-0x7E, which would lose ordinary prose.
    #[test]
    fn does_not_treat_utf8_continuation_bytes_as_controls() {
        for text in [
            "活动回显轮询改动进行了只读独立 Review。",
            "Mesh caller 集合改为从 `AdapterRegistry` 派生后？",
            "覆盖用户输入或产生多余读？ | **FAIL**",
            "既有行为回退？",
            "全角分号；连接",
        ] {
            let mut stripper = AnsiStripper::new();
            let bytes = text.as_bytes();
            let mut produced = String::new();
            for chunk in bytes.chunks(3) {
                produced.push_str(&stripper.push(chunk));
            }
            produced.push_str(&stripper.finish());
            assert_eq!(produced, text, "damaged: {produced:?}");
            assert!(!produced.contains('\u{FFFD}'), "damaged: {produced:?}");
        }
    }

    #[test]
    fn still_strips_sequences_containing_continuation_like_bytes() {
        let mut stripper = AnsiStripper::new();
        let text = stripper.push("\x1b[1;32m回显\x1b[0m 完成；\x1b]0;标题\x07读".as_bytes());
        assert_eq!(format!("{text}{}", stripper.finish()), "回显 完成；读");
    }
}
