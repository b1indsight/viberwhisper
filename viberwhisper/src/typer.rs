pub trait TextTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct MockTyper;

impl TextTyper for MockTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        println!("[Mock Typer] 向当前窗口输入文字: {}", text);
        Ok(())
    }
}

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

    // 与 Windows x64 ABI 完全对齐：
    // wVk(2) + wScan(2) + dwFlags(4) + time(4) + pad(4) + dwExtraInfo(8) = 24 bytes
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

    // 联合体用 32 字节 padding 对齐至 MOUSEINPUT 大小
    #[repr(C)]
    pub union INPUT_UNION {
        pub ki: ManuallyDrop<KEYBDINPUT>,
        pub _padding: [u8; 32],
    }

    // type(4) + pad(4) + union(32) = 40 bytes，与 Windows sizeof(INPUT) 一致
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_typer_succeeds() {
        let typer = MockTyper;
        assert!(typer.type_text("hello world").is_ok());
    }

    #[test]
    fn test_windows_typer_empty_string() {
        let typer = WindowsTyper;
        assert!(typer.type_text("").is_ok());
    }
}
