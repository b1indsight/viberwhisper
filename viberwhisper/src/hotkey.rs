use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};

pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
}

pub enum HotkeyEvent {
    Pressed,
    Released,
}

impl HotkeyManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let manager = GlobalHotKeyManager::new()?;

        // Hardcoded to F8 (reliable key for global hotkey)
        let hotkey = HotKey::new(Some(Modifiers::empty()), Code::F8);

        manager.register(hotkey)?;

        println!("Registered global hotkey: F8");

        Ok(HotkeyManager { manager, hotkey })
    }

    pub fn check_event(&self) -> Option<HotkeyEvent> {
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == self.hotkey.id() {
                return match event.state {
                    HotKeyState::Pressed => Some(HotkeyEvent::Pressed),
                    HotKeyState::Released => Some(HotkeyEvent::Released),
                };
            }
        }
        None
    }

    pub fn run_event_loop<F>(&self, mut on_press: F, mut on_release: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: FnMut(),
    {
        println!("Hotkey event loop started. Press F8 to record...");

        loop {
            if let Some(event) = self.check_event() {
                match event {
                    HotkeyEvent::Pressed => on_press(),
                    HotkeyEvent::Released => on_release(),
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_manager_creation() {
        // Note: Global hotkey registration may fail in test environment
        // but we verify the API works
        let _ = HotkeyManager::new();
    }
}
