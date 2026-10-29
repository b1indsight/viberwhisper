//! Decode only the top-level JSON `text` string before the complete response exists.
//! Final schema validation belongs to DictationResult, never to this provisional view.

use serde::de::IgnoredAny;

#[derive(Default)]
enum Phase {
    #[default]
    Object,
    Key,
    Colon(bool),
    Value(bool),
    Text,
    Separator,
    End,
    Invalid,
}

#[derive(Default)]
pub(super) struct TextStream {
    raw: String,
    cursor: usize,
    text: String,
    phase: Phase,
    found: bool,
}

impl TextStream {
    /// The caller bounds the raw response size. Unsafe/incomplete JSON never leaks
    /// metadata or unfinished escapes; an invalid provisional view simply stops.
    pub fn push(&mut self, delta: &str) -> Option<&str> {
        self.raw.push_str(delta);
        let previous = self.text.len();
        if self.advance().is_err() {
            self.phase = Phase::Invalid;
        }
        if self.text.contains('\0') || self.text.len() > 16_384 {
            self.phase = Phase::Invalid;
        }
        (self.text.len() != previous && !matches!(self.phase, Phase::Invalid))
            .then_some(self.text.as_str())
    }

    fn advance(&mut self) -> Result<(), ()> {
        loop {
            let tail = &self.raw[self.cursor..];
            if tail.is_empty() || matches!(self.phase, Phase::Invalid) {
                return Ok(());
            }
            if !matches!(self.phase, Phase::Text) {
                let trimmed = tail.trim_start_matches([' ', '\n', '\r', '\t']);
                self.cursor += tail.len() - trimmed.len();
                if trimmed.is_empty() {
                    return Ok(());
                }
            }
            let tail = &self.raw[self.cursor..];
            let first = tail.as_bytes()[0];
            let consumed = match self.phase {
                Phase::Object if first == b'{' => {
                    self.phase = Phase::Key;
                    1
                }
                Phase::Key if first == b'}' => {
                    self.phase = Phase::End;
                    1
                }
                Phase::Key => {
                    let mut parsed = serde_json::Deserializer::from_str(tail).into_iter::<String>();
                    match parsed.next().ok_or(())? {
                        Ok(key) => {
                            self.phase = Phase::Colon(key == "text");
                            parsed.byte_offset()
                        }
                        Err(error) if error.is_eof() => return Ok(()),
                        Err(_) => return Err(()),
                    }
                }
                Phase::Colon(text) if first == b':' => {
                    self.phase = Phase::Value(text);
                    1
                }
                Phase::Value(true) if first == b'"' && !self.found => {
                    self.found = true;
                    self.phase = Phase::Text;
                    1
                }
                Phase::Value(false) => {
                    let mut parsed =
                        serde_json::Deserializer::from_str(tail).into_iter::<IgnoredAny>();
                    match parsed.next().ok_or(())? {
                        Ok(_) => {
                            // A number ending at this delta can still grow in the
                            // next one. Wait for its delimiter before skipping it.
                            if parsed.byte_offset() == tail.len() {
                                return Ok(());
                            }
                            self.phase = Phase::Separator;
                            parsed.byte_offset()
                        }
                        Err(error) if error.is_eof() => return Ok(()),
                        Err(_) => return Err(()),
                    }
                }
                Phase::Text => match first {
                    b'"' => {
                        self.phase = Phase::Separator;
                        1
                    }
                    b'\\' => {
                        if tail.len() < 2 {
                            return Ok(());
                        }
                        let count = if tail.as_bytes()[1] == b'u' {
                            if tail.len() < 6 {
                                return Ok(());
                            }
                            let high = u16::from_str_radix(tail.get(2..6).ok_or(())?, 16)
                                .map_err(|_| ())?;
                            if (0xd800..=0xdbff).contains(&high) {
                                12
                            } else {
                                6
                            }
                        } else {
                            2
                        };
                        if tail.len() < count {
                            return Ok(());
                        }
                        let escaped = format!("\"{}\"", tail.get(..count).ok_or(())?);
                        self.text
                            .push_str(&serde_json::from_str::<String>(&escaped).map_err(|_| ())?);
                        count
                    }
                    0..=31 => return Err(()),
                    _ => {
                        let character = tail.chars().next().ok_or(())?;
                        self.text.push(character);
                        character.len_utf8()
                    }
                },
                Phase::Separator if first == b',' => {
                    self.phase = Phase::Key;
                    1
                }
                Phase::Separator if first == b'}' => {
                    self.phase = Phase::End;
                    1
                }
                _ => return Err(()),
            };
            self.cursor += consumed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_delta_boundaries_reveal_only_top_level_decoded_text() {
        // Model metadata can precede the transcript, and escapes/surrogate pairs
        // can span tokens. Candidate names and topics must never become preview content.
        let raw = r#"{"memory_candidates":["secret"],"text":"中文\n\"quote\"\\\ud83c\udf99️","context":"hidden"}"#;
        for width in 1..=raw.chars().count() {
            let mut stream = TextStream::default();
            let characters = raw.chars().collect::<Vec<_>>();
            let mut last = String::new();
            for part in characters.chunks(width) {
                if let Some(text) = stream.push(&part.iter().collect::<String>()) {
                    assert!("中文\n\"quote\"\\🎙️".starts_with(text));
                    last = text.into();
                }
            }
            assert_eq!(last, "中文\n\"quote\"\\🎙️");
        }
    }
}
