use crate::typer::TextTyper;

pub struct WindowsTyper;

impl TextTyper for WindowsTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        // 给目标窗口一点时间重新获得焦点
        std::thread::sleep(std::time::Duration::from_millis(100));

        let utf16: Vec<u16> = text.encode_utf16().collect();
        if utf16.is_empty() {
            return Ok(());
        }

        let mut inputs: Vec<ffi::INPUT> = Vec::with_capacity(utf16.len() * 2);
        for &code_unit in &utf16 {
            inputs.push(ffi::make_key_input(code_unit, false));
            inputs.push(ffi::make_key_input(code_unit, true));
        }

        let sent = unsafe {
            ffi::SendInput(
                inputs.len() as u32,
                inputs.as_mut_ptr(),
                std::mem::size_of::<ffi::INPUT>() as i32,
            )
        };

        if sent as usize != inputs.len() {
            return Err(format!(
                "SendInput 只发送了 {}/{} 个事件",
                sent,
                inputs.len()
            )
            .into());
        }

        println!("[WindowsTyper] 已输入: {}", text);
        Ok(())
    }
}

mod ffi {
    use std::mem::ManuallyDrop;

    pub const INPUT_KEYBOARD: u32 = 1;
    pub const KEYEVENTF_UNICODE: u32 = 0x0004;
    pub const KEYEVENTF_KEYUP: u32 = 0x0002;

    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(non_snake_case)]
    pub struct KEYBDINPUT {
        pub wVk: u16,
        pub wScan: u16,
        pub dwFlags: u32,
        pub time: u32,
        pub dwExtraInfo: usize,
    }

    #[repr(C)]
    pub union INPUT_UNION {
        pub ki: ManuallyDrop<KEYBDINPUT>,
        pub _padding: [u8; 32],
    }

    #[repr(C)]
    pub struct INPUT {
        pub r#type: u32,
        pub _union: INPUT_UNION,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn SendInput(nInputs: u32, pInputs: *mut INPUT, cbSize: i32) -> u32;
    }

    pub fn make_key_input(scan_code: u16, key_up: bool) -> INPUT {
        let flags = if key_up {
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
        } else {
            KEYEVENTF_UNICODE
        };
        INPUT {
            r#type: INPUT_KEYBOARD,
            _union: INPUT_UNION {
                ki: ManuallyDrop::new(KEYBDINPUT {
                    wVk: 0,
                    wScan: scan_code,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                }),
            },
        }
    }
}
