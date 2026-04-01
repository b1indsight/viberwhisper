/// No-op overlay for unsupported platforms.
pub struct OverlayManager;

impl OverlayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(OverlayManager)
    }

    pub fn set_recording(&mut self, _recording: bool) {}

    pub fn check_click(&self) -> bool {
        false
    }

    pub fn update(&self) {}
}
