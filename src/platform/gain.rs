//! Shared dB editing rules; native controls never quantize an unedited saved gain.

#[cfg(not(test))]
pub(crate) type GainNotify = std::sync::Arc<dyn Fn(f32) + Send + Sync>;

#[cfg(all(not(test), target_os = "macos"))]
pub(crate) use super::macos::gain::NativeGain;
#[cfg(all(not(test), target_os = "windows"))]
pub(crate) use super::windows::gain::NativeGain;

/// Format the saved linear amplitude without pretending that zero or a negative value has dB.
pub(crate) fn gain_label(gain: f32) -> String {
    if gain > 0.0 {
        format!("{:+.1} dB", 20.0 * gain.log10())
    } else if gain == 0.0 {
        "静音（0 倍）".into()
    } else {
        format!("{gain} 倍（反相）")
    }
}

/// A 0–200 native slider represents −10.0–+10.0 dB, but only user input creates a draft.
pub(crate) struct GainDraft {
    original_gain: f32,
    draft: Option<i32>,
    saving: Option<i32>,
    queued: Option<i32>,
    failed: bool,
}

impl GainDraft {
    pub(crate) fn new(gain: f32) -> Self {
        Self {
            original_gain: gain,
            draft: None,
            saving: None,
            queued: None,
            failed: false,
        }
    }

    /// Acknowledge persistence without overwriting a later drag or completed adjustment.
    /// Return the latest completed adjustment if one arrived while the save was pending.
    pub(crate) fn set_gain(&mut self, gain: f32, save_failed: bool) -> Option<f32> {
        if self.draft == self.saving.take() && self.queued.is_none() {
            self.draft = None;
        }
        self.original_gain = gain;
        self.failed = save_failed;
        self.take_queued()
    }

    pub(crate) fn position(&self) -> i32 {
        self.draft.unwrap_or_else(|| {
            if self.original_gain > 0.0 {
                (200.0 * self.original_gain.log10() + 100.0)
                    .round()
                    .clamp(0.0, 200.0) as i32
            } else {
                100
            }
        })
    }

    pub(crate) fn value_label(&self) -> String {
        self.draft.map_or_else(
            || gain_label(self.original_gain),
            |position| format!("{:+.1} dB", (position - 100) as f32 / 10.0),
        )
    }

    pub(crate) fn notice(&self) -> &'static str {
        if self.failed && self.draft.is_none() {
            "保存失败，已恢复原值；请重新调整"
        } else if self.draft.is_none()
            && !(0.316_227_76..=3.162_277_7).contains(&self.original_gain)
        {
            "原值超出范围；调整后自动保存"
        } else {
            "松开自动保存，下次录音生效"
        }
    }

    pub(crate) fn change(&mut self, position: i32) {
        let position = position.clamp(0, 200);
        // Native thumb clicks and ENDTRACK can notify without moving or editing a legacy value.
        if position != self.position() {
            self.draft = Some(position);
            self.failed = false;
        }
    }

    /// Commit a mouse release or keyboard adjustment, coalescing rapid completed edits.
    pub(crate) fn submit(&mut self) -> Option<f32> {
        self.queued = self.draft.filter(|position| Some(*position) != self.saving);
        self.take_queued()
    }

    fn take_queued(&mut self) -> Option<f32> {
        if self.saving.is_some() {
            return None;
        }
        let position = self.queued.take()?;
        self.saving = Some(position);
        Some(10.0f32.powf((position - 100) as f32 / 200.0))
    }
}

#[cfg(not(any(test, target_os = "macos", target_os = "windows")))]
pub(crate) struct NativeGain;

#[cfg(not(any(test, target_os = "macos", target_os = "windows")))]
impl NativeGain {
    pub(crate) fn new(
        _: &tray_icon::menu::Menu,
        _: &tray_icon::menu::MenuItem,
        _: GainNotify,
    ) -> anyhow::Result<Self> {
        Ok(Self)
    }
    pub(crate) fn set_gain(&mut self, _: f32, _: bool) {}
    pub(crate) fn show(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("Microphone gain controls are unsupported on this platform")
    }
    pub(crate) fn is_visible(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_editor_preserves_legacy_values_and_restores_the_saved_value_on_failure() {
        // Opening the menu must not round 3× to the nearest displayed tenth of a dB.
        for gain in [3.0, 20.0, 0.0, -2.0] {
            let mut edit = GainDraft::new(gain);
            assert_eq!(edit.submit(), None);
            edit.change(edit.position());
            assert_eq!(edit.submit(), None);
            assert_eq!(edit.value_label(), gain_label(gain));
        }
        for (position, expected) in [
            (0, 0.316_227_76),
            (100, 1.0),
            (200, 3.162_277_7),
            (73, 0.732_824_5),
        ] {
            let mut edit = GainDraft::new(3.0);
            edit.change(position);
            assert!((edit.submit().unwrap() - expected).abs() < 0.000_001);
            assert_eq!(edit.submit(), None);
            edit.set_gain(3.0, true);
            assert!(edit.notice().contains("保存失败"));
            assert_eq!(edit.value_label(), gain_label(3.0));
            assert_eq!(edit.submit(), None);
            edit.change(position);
            assert!((edit.submit().unwrap() - expected).abs() < 0.000_001);
            edit.set_gain(expected, false);
            assert_eq!(edit.submit(), None);
        }
    }

    #[test]
    fn gain_editor_keeps_the_latest_release_without_saving_an_unfinished_drag() {
        // A slow acknowledgement must not reset newer thumb input or lose the final release.
        let mut edit = GainDraft::new(1.0);
        edit.change(127);
        assert!((edit.submit().unwrap() - 1.364_583_1).abs() < 0.000_001);
        edit.change(73);
        assert_eq!(edit.submit(), None);
        edit.change(158);
        assert_eq!(edit.submit(), None);
        edit.change(142); // This drag has not ended yet.
        let next = edit.set_gain(1.364_583_1, false).unwrap();
        assert!((next - 1.949_844_6).abs() < 0.000_001);
        assert_eq!(edit.position(), 142);
        assert_eq!(edit.set_gain(next, false), None);
        assert_eq!(edit.position(), 142);
        let last = edit.submit().unwrap();
        assert!((last - 1.621_810_1).abs() < 0.000_001);
        assert_eq!(edit.set_gain(last, false), None);
        assert_eq!(edit.submit(), None);
        assert_eq!(edit.position(), 142);
    }
}
