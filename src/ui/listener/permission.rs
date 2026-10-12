/// Limits permission guidance to one visible message during a desktop launch.
#[derive(Default)]
pub(super) struct PermissionGuidance {
    shown: bool,
}

impl PermissionGuidance {
    pub(super) fn request(&mut self, permission_available: bool) -> bool {
        if permission_available || self.shown {
            return false;
        }
        self.shown = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::PermissionGuidance;

    #[test]
    fn startup_and_later_revocation_share_one_prompt_per_launch() {
        // Startup can be allowed while a later finalized transcription is denied.
        for initially_allowed in [true, false] {
            let mut guidance = PermissionGuidance::default();
            assert_eq!(guidance.request(initially_allowed), !initially_allowed);
            assert_eq!(guidance.request(false), initially_allowed);
            assert!(!guidance.request(false));
            assert!(!guidance.request(true));
            assert!(!guidance.request(false));
        }
    }
}
