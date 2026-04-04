use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct TrayManager {
    tray_icon: TrayIcon,
    icon_idle: Icon,
    icon_recording: Icon,
    exit_item_id: tray_icon::menu::MenuId,
}

impl TrayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let icon_idle = create_icon(128, 128, 128, 255); // 灰色 — 空闲
        let icon_recording = create_icon(220, 50, 50, 255); // 红色 — 录音中

        let menu = Menu::new();
        let title_item = MenuItem::new("ViberWhisper", false, None);
        let status_item = MenuItem::new("状态：空闲", false, None);
        let separator = PredefinedMenuItem::separator();
        let exit_item = MenuItem::new("退出", true, None);
        let exit_id = exit_item.id().clone();

        menu.append(&title_item)?;
        menu.append(&status_item)?;
        menu.append(&separator)?;
        menu.append(&exit_item)?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ViberWhisper - 空闲")
            .with_icon(icon_idle.clone())
            .build()?;

        Ok(TrayManager {
            tray_icon,
            icon_idle,
            icon_recording,
            exit_item_id: exit_id,
        })
    }

    /// 切换托盘图标状态
    pub fn set_recording(&mut self, recording: bool) {
        let icon = if recording {
            &self.icon_recording
        } else {
            &self.icon_idle
        };
        let tooltip = if recording {
            "ViberWhisper - 录音中"
        } else {
            "ViberWhisper - 空闲"
        };

        let _ = self.tray_icon.set_icon(Some(icon.clone()));
        let _ = self.tray_icon.set_tooltip(Some(tooltip));
    }

    /// 检查用户是否点击了"退出"菜单
    pub fn check_exit(&self) -> bool {
        if let Ok(event) = MenuEvent::receiver().try_recv()
            && event.id == self.exit_item_id {
                return true;
            }
        false
    }
}

/// 生成一个简单的圆形图标（RGBA）
fn create_icon(r: u8, g: u8, b: u8, a: u8) -> Icon {
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    let radius = center - 2.0;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            let idx = ((y * size + x) * 4) as usize;
            if dist <= radius {
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = a;
            }
        }
    }

    Icon::from_rgba(rgba, size, size).expect("Failed to create tray icon")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_icon() {
        let icon = create_icon(128, 128, 128, 255);
        // Icon created successfully — no panic
        drop(icon);
    }
}
