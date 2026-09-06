use std::io::Cursor;
use std::marker::PhantomData;
#[cfg(not(test))]
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
#[cfg(not(test))]
use tracing::warn;
#[cfg(not(test))]
use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::menu::{MenuEvent, MenuId};
#[cfg(not(test))]
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent, TrayIconId};

use crate::history::RECENT_HISTORY_LIMIT;

const MIN_DEBOUNCE_WINDOW: Duration = Duration::from_millis(300);
const HISTORY_LABEL_GRAPHEME_LIMIT: usize = 40;
const STATUS_IDLE_PNG: &[u8] = include_bytes!("../../assets/status-idle.png");
const STATUS_RECORDING_PNG: &[u8] = include_bytes!("../../assets/status-recording.png");

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub enum TrayAction {
    ToggleRecording,
    CopyHistory(String),
    Exit,
}

#[derive(Debug, Clone)]
#[cfg_attr(test, allow(dead_code))]
pub enum TrayEvent {
    Icon(TrayIconEvent),
    Menu(MenuEvent),
}

/// Target policy for native application setup, timing, and icon presentation.
pub(crate) trait TrayPolicy: 'static {
    fn idle_icon_is_template() -> bool;

    #[cfg(not(test))]
    fn prepare_application() -> Result<()>;

    #[cfg(not(test))]
    fn double_click_interval() -> Option<Duration>;

    #[cfg(not(test))]
    fn set_icon(tray_icon: &TrayIcon, icon: Icon, is_template: bool) -> tray_icon::Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
enum ClickPhase {
    LeftUp,
    LeftDouble,
    Other,
}

struct ClickDebounce {
    window: Duration,
    last_accepted: Option<Instant>,
    suppress_next_up_until: Option<Instant>,
}

impl ClickDebounce {
    fn new(window: Duration) -> Self {
        Self {
            window,
            last_accepted: None,
            suppress_next_up_until: None,
        }
    }

    fn accept(&mut self, phase: ClickPhase, now: Instant) -> bool {
        match phase {
            ClickPhase::LeftDouble => {
                self.suppress_next_up_until = Some(now + self.window);
                false
            }
            ClickPhase::LeftUp => {
                if let Some(deadline) = self.suppress_next_up_until.take()
                    && now < deadline
                {
                    return false;
                }
                if let Some(last) = self.last_accepted
                    && now.duration_since(last) < self.window
                {
                    return false;
                }
                self.last_accepted = Some(now);
                true
            }
            ClickPhase::Other => false,
        }
    }
}

fn classify_icon_event(tray_icon_id: &TrayIconId, event: &TrayIconEvent) -> ClickPhase {
    if event.id() != tray_icon_id {
        return ClickPhase::Other;
    }
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => ClickPhase::LeftUp,
        TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => ClickPhase::LeftDouble,
        _ => ClickPhase::Other,
    }
}

fn is_exit_menu_event(exit_item_id: &MenuId, event: &MenuEvent) -> bool {
    event.id == *exit_item_id
}

fn push_recent(entries: &mut Vec<String>, text: String) {
    entries.insert(0, text);
    entries.truncate(RECENT_HISTORY_LIMIT);
}

fn format_history_label(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "（空白）".to_string();
    }

    let mut characters = normalized.chars();
    let mut label: String = characters
        .by_ref()
        .take(HISTORY_LABEL_GRAPHEME_LIMIT)
        .collect();
    if characters.next().is_some() {
        label.push('…');
    }
    label.replace('&', "&&")
}

fn effective_debounce_window(platform_interval: Option<Duration>) -> Duration {
    platform_interval
        .unwrap_or(MIN_DEBOUNCE_WINDOW)
        .max(MIN_DEBOUNCE_WINDOW)
}

#[cfg(not(test))]
pub struct TrayManager<P: TrayPolicy> {
    tray_icon: TrayIcon,
    icon_idle: Icon,
    icon_recording: Icon,
    status_item: MenuItem,
    history_items: Vec<MenuItem>,
    history: Vec<String>,
    exit_item_id: tray_icon::menu::MenuId,
    click_debounce: ClickDebounce,
    policy: PhantomData<P>,
}

// Keep tests away from the process-global native tray backend. App runs and native smoke tests
// exercise that path; unit tests use deterministic policy and event-classification seams.
#[cfg(test)]
pub struct TrayManager<P: TrayPolicy>(PhantomData<P>);

#[cfg(not(test))]
impl<P: TrayPolicy> TrayManager<P> {
    /// Construct the process's single tray manager and install its process-lifetime event handlers.
    ///
    /// `tray-icon` stores handlers in one-shot global cells, so another manager in the same process
    /// cannot replace the callback.
    pub fn new(notify: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Self> {
        P::prepare_application()?;
        let (idle_source, idle_is_template) = status_icon_source::<P>(false);
        let (recording_source, _) = status_icon_source::<P>(true);
        let icon_idle = create_icon(idle_source)?;
        let icon_recording = create_icon(recording_source)?;

        let menu = Menu::new();
        let title_item = MenuItem::new("ViberWhisper", false, None);
        let status_item = MenuItem::new("状态：空闲", false, None);
        let separator = PredefinedMenuItem::separator();
        let history_title_item = MenuItem::new("最近识别", false, None);
        let history_items: Vec<_> = (0..RECENT_HISTORY_LIMIT)
            .map(|index| MenuItem::new(if index == 0 { "暂无识别历史" } else { "" }, false, None))
            .collect();
        let history_separator = PredefinedMenuItem::separator();
        let exit_item = MenuItem::new("退出", true, None);
        let exit_id = exit_item.id().clone();

        menu.append(&title_item)?;
        menu.append(&status_item)?;
        menu.append(&separator)?;
        menu.append(&history_title_item)?;
        for item in &history_items {
            menu.append(item)?;
        }
        menu.append(&history_separator)?;
        menu.append(&exit_item)?;

        install_native_handlers(notify);
        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("ViberWhisper - 空闲")
            .with_icon_as_template(idle_is_template)
            .with_icon(icon_idle.clone())
            .build()?;

        Ok(TrayManager {
            tray_icon,
            icon_idle,
            icon_recording,
            status_item,
            history_items,
            history: Vec::new(),
            exit_item_id: exit_id,
            click_debounce: ClickDebounce::new(effective_debounce_window(
                P::double_click_interval(),
            )),
            policy: PhantomData,
        })
    }

    pub fn set_recording(&mut self, recording: bool) {
        let (icon, is_template) = if recording {
            (&self.icon_recording, status_icon_source::<P>(true).1)
        } else {
            (&self.icon_idle, status_icon_source::<P>(false).1)
        };
        let tooltip = if recording {
            "ViberWhisper - 录音中"
        } else {
            "ViberWhisper - 空闲"
        };

        self.status_item.set_text(if recording {
            "状态：录音中"
        } else {
            "状态：空闲"
        });
        if let Err(err) = P::set_icon(&self.tray_icon, icon.clone(), is_template) {
            warn!(error = ?err, "failed to update tray icon");
        }
        if let Err(err) = self.tray_icon.set_tooltip(Some(tooltip)) {
            warn!(error = ?err, "failed to update tray tooltip");
        }
    }

    pub fn set_history(&mut self, entries: Vec<String>) {
        self.history = entries.into_iter().take(RECENT_HISTORY_LIMIT).collect();
        self.update_history_items();
    }

    pub fn push_history(&mut self, text: String) {
        push_recent(&mut self.history, text);
        self.update_history_items();
    }

    fn update_history_items(&self) {
        for (index, item) in self.history_items.iter().enumerate() {
            if let Some(text) = self.history.get(index) {
                item.set_text(format_history_label(text));
                item.set_enabled(true);
            } else {
                item.set_text(if index == 0 { "暂无识别历史" } else { "" });
                item.set_enabled(false);
            }
        }
    }

    pub fn handle_event(&mut self, event: TrayEvent) -> Option<TrayAction> {
        match event {
            TrayEvent::Menu(event) => {
                if is_exit_menu_event(&self.exit_item_id, &event) {
                    return Some(TrayAction::Exit);
                }
                self.history_items
                    .iter()
                    .position(|item| item.id() == &event.id)
                    .and_then(|index| self.history.get(index))
                    .cloned()
                    .map(TrayAction::CopyHistory)
            }
            TrayEvent::Icon(event) => {
                let phase = classify_icon_event(self.tray_icon.id(), &event);
                self.click_debounce
                    .accept(phase, Instant::now())
                    .then_some(TrayAction::ToggleRecording)
            }
        }
    }
}

#[cfg(not(test))]
fn install_native_handlers(notify: impl Fn(TrayEvent) + Send + Sync + 'static) {
    let notify = Arc::new(notify);
    let icon_notify = Arc::clone(&notify);
    TrayIconEvent::set_event_handler(Some(move |event| {
        icon_notify(TrayEvent::Icon(event));
    }));
    MenuEvent::set_event_handler(Some(move |event| {
        notify(TrayEvent::Menu(event));
    }));
}

#[cfg(test)]
impl<P: TrayPolicy> TrayManager<P> {
    pub fn new(_notify: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Self> {
        Ok(TrayManager(PhantomData))
    }

    pub fn set_recording(&mut self, _recording: bool) {}

    pub fn set_history(&mut self, _entries: Vec<String>) {}

    pub fn push_history(&mut self, _text: String) {}

    pub fn handle_event(&mut self, _event: TrayEvent) -> Option<TrayAction> {
        None
    }
}

fn status_icon_source<P: TrayPolicy>(recording: bool) -> (&'static [u8], bool) {
    if recording {
        (STATUS_RECORDING_PNG, false)
    } else {
        (STATUS_IDLE_PNG, P::idle_icon_is_template())
    }
}

#[cfg(not(test))]
fn create_icon(bytes: &[u8]) -> Result<Icon> {
    let (rgba, width, height) = decode_icon_png(bytes)?;
    Ok(Icon::from_rgba(rgba, width, height)?)
}

fn decode_icon_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info()?;
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgba)?;

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        bail!(
            "status icon must be an 8-bit RGBA PNG, got {:?} {:?}",
            info.bit_depth,
            info.color_type
        );
    }

    rgba.truncate(info.buffer_size());
    Ok((rgba, info.width, info.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tray_icon::dpi::PhysicalPosition;
    use tray_icon::menu::MenuId;
    use tray_icon::{Rect, TrayIconId};

    struct TemplateTrayPolicy;

    impl TrayPolicy for TemplateTrayPolicy {
        fn idle_icon_is_template() -> bool {
            true
        }
    }

    fn left_up(id: &str) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: TrayIconId::new(id),
            position: PhysicalPosition::new(0.0, 0.0),
            rect: Rect::default(),
            button: tray_icon::MouseButton::Left,
            button_state: tray_icon::MouseButtonState::Up,
        }
    }

    #[test]
    fn embedded_status_icons_are_32px_rgba_with_transparency() {
        for bytes in [STATUS_IDLE_PNG, STATUS_RECORDING_PNG] {
            let (rgba, width, height) = decode_icon_png(bytes).unwrap();

            assert_eq!((width, height), (32, 32));
            assert_eq!(rgba.len(), (width * height * 4) as usize);
            assert!(rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 0));
            assert!(rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255));
        }
    }

    #[test]
    fn idle_uses_template_rendering_and_recording_keeps_explicit_color() {
        assert!(status_icon_source::<TemplateTrayPolicy>(false).1);
        assert!(!status_icon_source::<TemplateTrayPolicy>(true).1);
    }

    #[test]
    fn effective_window_uses_platform_interval_with_300ms_floor() {
        assert_eq!(
            effective_debounce_window(Some(Duration::from_millis(200))),
            Duration::from_millis(300)
        );
        assert_eq!(
            effective_debounce_window(Some(Duration::from_millis(500))),
            Duration::from_millis(500)
        );
        assert_eq!(effective_debounce_window(None), Duration::from_millis(300));
    }

    #[test]
    fn debounce_accepts_boundary_and_ignored_click_does_not_extend_window() {
        let base = Instant::now();
        let mut debounce = ClickDebounce::new(Duration::from_millis(300));

        assert!(debounce.accept(ClickPhase::LeftUp, base));
        assert!(!debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(200)));
        assert!(debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(300)));
    }

    #[test]
    fn native_double_click_suppresses_only_the_trailing_left_up() {
        let base = Instant::now();
        let mut debounce = ClickDebounce::new(Duration::from_millis(300));

        assert!(debounce.accept(ClickPhase::LeftUp, base));
        assert!(!debounce.accept(ClickPhase::LeftDouble, base + Duration::from_millis(400)));
        assert!(!debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(401)));
        assert!(debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(402)));
    }

    #[test]
    fn native_events_are_filtered_by_the_owning_tray_and_menu_ids() {
        assert_eq!(
            classify_icon_event(&TrayIconId::new("ours"), &left_up("ours")),
            ClickPhase::LeftUp
        );
        assert_eq!(
            classify_icon_event(&TrayIconId::new("ours"), &left_up("other")),
            ClickPhase::Other
        );

        let exit_id = MenuId::new("exit");
        assert!(is_exit_menu_event(
            &exit_id,
            &MenuEvent {
                id: exit_id.clone()
            }
        ));
        assert!(!is_exit_menu_event(
            &exit_id,
            &MenuEvent {
                id: MenuId::new("status")
            }
        ));
    }

    #[test]
    fn history_label_is_single_line_mnemonic_safe_and_unicode_bounded() {
        assert_eq!(
            format_history_label("one\n  two\t& three"),
            "one two && three"
        );
        assert_eq!(format_history_label(" \n\t "), "（空白）");

        let long_emoji = "😀".repeat(41);
        assert_eq!(
            format_history_label(&long_emoji),
            format!("{}…", "😀".repeat(40))
        );
        assert_eq!(format_history_label(&"中".repeat(40)), "中".repeat(40));
    }

    #[test]
    fn recent_history_is_newest_first_and_bounded_to_five() {
        let mut history = (2..=6)
            .rev()
            .map(|index| format!("entry {index}"))
            .collect();
        push_recent(&mut history, "entry 7".to_string());
        assert_eq!(
            history,
            ["entry 7", "entry 6", "entry 5", "entry 4", "entry 3"]
        );
    }
}
