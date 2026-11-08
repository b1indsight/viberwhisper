//! Test-only feasibility probe. Never enabled in the shipped preview before native validation.

use super::*;
use windows::Win32::Foundation::{
    HMODULE, HWND as CompositionWindow, POINT as DrawOffset, RECT as CompositionRect,
};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_ROUNDED_RECT, D2D1CreateDevice, ID2D1DeviceContext,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionSurface,
    IDCompositionTarget, IDCompositionVisual, IDCompositionVisual2,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::core::Interface;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    IntersectClipRect, RDW_NOCHILDREN, RDW_UPDATENOW, ScreenToClient,
};
use windows_sys::Win32::UI::Controls::{STATE_SYSTEM_INVISIBLE, WM_MOUSELEAVE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};

/// Native screen slot and slot-local thumb, shared by screen composition and manual printing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    slot: CompositionRect,
    thumb: CompositionRect,
    background: u32,
    color: u32,
}

/// Owns only pixels: no HWND, input routing, accessibility provider or scroll-position model.
struct Composition {
    device: IDCompositionDesktopDevice,
    target: IDCompositionTarget,
    visual: IDCompositionVisual2,
    surface: Option<(i32, i32, IDCompositionSurface)>,
    frame: Option<Frame>,
    detached: Cell<bool>,
}

impl Composition {
    fn new(window: HWND) -> Result<Self> {
        // WARP is sufficient for this small test surface and does not require a hardware GPU.
        // The D2D-backed device makes BeginDraw<ID2D1DeviceContext> a documented drawing path.
        let mut d3d: Option<ID3D11Device> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )?;
            let dxgi: IDXGIDevice = d3d
                .ok_or_else(|| anyhow!("D3D11 returned no drawing device"))?
                .cast()?;
            let d2d = D2D1CreateDevice(&dxgi, None)?;
            let device: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d)?;
            let target = device.CreateTargetForHwnd(CompositionWindow(window as isize), true)?;
            let visual = device.CreateVisual()?;
            target.SetRoot(&visual)?;
            Ok(Self {
                device,
                target,
                visual,
                surface: None,
                frame: None,
                detached: Cell::new(false),
            })
        }
    }

    /// Geometry and colors are the cache key: an unrelated native paint cannot request repair.
    fn update(&mut self, frame: Option<Frame>, origin: POINT) -> Result<bool> {
        if frame == self.frame {
            return Ok(false);
        }
        unsafe {
            if let Some(frame) = frame {
                let width = frame.slot.right - frame.slot.left;
                let height = frame.slot.bottom - frame.slot.top;
                ensure!(width > 0 && height > 0, "Composition slot has no area");
                if self.surface.as_ref().map(|(w, h, _)| (*w, *h)) != Some((width, height)) {
                    self.surface = Some((
                        width,
                        height,
                        self.device.CreateSurface(
                            width as u32,
                            height as u32,
                            DXGI_FORMAT_B8G8R8A8_UNORM,
                            DXGI_ALPHA_MODE_IGNORE,
                        )?,
                    ));
                }
                let surface = &self.surface.as_ref().unwrap().2;
                let mut offset = DrawOffset::default();
                let context: ID2D1DeviceContext = surface.BeginDraw(None, &mut offset)?;
                let drawing = (|| -> Result<()> {
                    // Native geometry is already physical. Only the thumb width was DPI-scaled.
                    context.SetDpi(96.0, 96.0);
                    context.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
                    let clip = D2D_RECT_F {
                        left: offset.x as f32,
                        top: offset.y as f32,
                        right: (offset.x + width) as f32,
                        bottom: (offset.y + height) as f32,
                    };
                    let background =
                        context.CreateSolidColorBrush(&draw_color(frame.background), None)?;
                    let thumb = context.CreateSolidColorBrush(&draw_color(frame.color), None)?;
                    context.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                    context.FillRectangle(&clip, &background);
                    let radius = (frame.thumb.right - frame.thumb.left) as f32 / 2.0;
                    context.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: (offset.x + frame.thumb.left) as f32,
                                top: (offset.y + frame.thumb.top) as f32,
                                right: (offset.x + frame.thumb.right) as f32,
                                bottom: (offset.y + frame.thumb.bottom) as f32,
                            },
                            radiusX: radius,
                            radiusY: radius,
                        },
                        &thumb,
                    );
                    context.PopAxisAlignedClip();
                    Ok(())
                })();
                // EndDraw belongs to the composition surface, not the returned D2D context.
                // Close the update on a brush/drawing failure too, before releasing resources.
                let ended = surface.EndDraw();
                drop(context);
                drawing?;
                ended?;
                self.visual.SetOffsetX2(origin.x as f32)?;
                self.visual.SetOffsetY2(origin.y as f32)?;
                self.visual.SetClip2(&D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: width as f32,
                    bottom: height as f32,
                })?;
                self.visual.SetContent(surface)?;
            } else {
                self.visual.SetContent(None::<&windows::core::IUnknown>)?;
            }
            self.device.Commit()?;
        }
        self.frame = frame;
        Ok(true)
    }

    fn detach(&self) -> Result<()> {
        if !self.detached.get() {
            unsafe {
                self.target.SetRoot(None::<&IDCompositionVisual>)?;
                self.device.Commit()?;
                self.device.WaitForCommitCompletion()?;
            }
            self.detached.set(true);
        }
        Ok(())
    }
}

fn draw_color(color: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (color & 0xff) as f32 / 255.0,
        g: ((color >> 8) & 0xff) as f32 / 255.0,
        b: ((color >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// Composite only the native slot; EDIT input, screen painting and geometry remain untouched.
/// The probe guard must be dropped before its preview; it borrows the stable boxed state by HWND.
struct Scrollbar {
    state: *const WindowState,
    composition: RefCell<Composition>,
    pressed: Cell<bool>,
    hovered: Cell<bool>,
    paints: Cell<u64>,
    commits: Cell<u64>,
    failure: RefCell<Option<String>>,
    trace: RefCell<Vec<String>>,
}

impl Scrollbar {
    fn state(&self) -> &WindowState {
        // SAFETY: `exercise` keeps its preview alive until after this guard is dropped.
        unsafe { &*self.state }
    }

    fn info(&self) -> Option<SCROLLBARINFO> {
        let state = self.state();
        let mut info = SCROLLBARINFO {
            cbSize: size_of::<SCROLLBARINFO>() as u32,
            ..Default::default()
        };
        if !state.style.get().high_contrast
            && unsafe { GetWindowLongPtrW(state.text.get(), GWL_STYLE) } & WS_VSCROLL as isize != 0
            && unsafe { GetScrollBarInfo(state.text.get(), OBJID_VSCROLL, &mut info) } != 0
            && info.rgstate[0] & STATE_SYSTEM_INVISIBLE == 0
        {
            Some(info)
        } else {
            None
        }
    }

    fn frame(&self) -> Result<Option<Frame>> {
        let Some(info) = self.info() else {
            return Ok(None);
        };
        let mut cursor = POINT::default();
        ensure!(
            unsafe { GetCursorPos(&mut cursor) } != 0,
            "Could not read native scrollbar hover"
        );
        let bounds = info.rcScrollBar;
        let width = self
            .state()
            .pixels(6)
            .min(bounds.right - bounds.left - 2)
            .max(1);
        let left = (bounds.right - bounds.left - width) / 2;
        let hover = cursor.x >= bounds.left
            && cursor.x < bounds.right
            && cursor.y >= bounds.top
            && cursor.y < bounds.bottom;
        Ok(Some(Frame {
            slot: CompositionRect {
                left: bounds.left,
                top: bounds.top,
                right: bounds.right,
                bottom: bounds.bottom,
            },
            thumb: CompositionRect {
                left,
                top: info.xyThumbTop,
                right: left + width,
                bottom: info.xyThumbBottom,
            },
            background: self.state().style.get().background,
            color: if self.pressed.get() {
                0x00c7c7c7
            } else if hover {
                0x00b0b0b0
            } else {
                0x00929292
            },
        }))
    }

    fn sync(&self) {
        if self.failure.borrow().is_some() {
            return;
        }
        let result = (|| -> Result<()> {
            let frame = self.frame()?;
            let mut origin = POINT::default();
            if let Some(frame) = frame {
                origin.x = frame.slot.left;
                origin.y = frame.slot.top;
                ensure!(
                    unsafe { ScreenToClient(self.state().window.get(), &mut origin) } != 0,
                    "Could not map native slot to composition target"
                );
            }
            // Query native state before borrowing graphics: queries may re-enter subclasses.
            if self.composition.borrow_mut().update(frame, origin)? {
                self.commits.set(self.commits.get() + 1);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
        }
    }

    fn fail(&self, error: anyhow::Error) {
        eprintln!("scrollbar composition failed: {error:#}");
        self.failure
            .borrow_mut()
            .get_or_insert_with(|| format!("{error:#}"));
    }

    fn check(&self) -> Result<()> {
        if let Some(error) = &*self.failure.borrow() {
            anyhow::bail!("Scrollbar composition failed: {error}");
        }
        Ok(())
    }

    /// Manual WM_PRINT evidence only; this GDI path never paints the actual screen.
    fn print(&self, dc: HDC) -> Result<()> {
        let Some(frame) = self.frame()? else {
            return Ok(());
        };
        let mut window = RECT::default();
        ensure!(
            !dc.is_null() && unsafe { GetWindowRect(self.state().text.get(), &mut window) } != 0,
            "Could not map the scrollbar print context"
        );
        let bounds = RECT {
            left: frame.slot.left - window.left,
            top: frame.slot.top - window.top,
            right: frame.slot.right - window.left,
            bottom: frame.slot.bottom - window.top,
        };
        unsafe {
            let saved = SaveDC(dc);
            ensure!(saved != 0, "Could not save the scrollbar print context");
            if IntersectClipRect(dc, bounds.left, bounds.top, bounds.right, bounds.bottom) == 0 {
                RestoreDC(dc, saved);
                anyhow::bail!("Could not clip the scrollbar print context");
            }
            SetDCBrushColor(dc, frame.background);
            let filled = FillRect(dc, &bounds, GetStockObject(DC_BRUSH));
            SelectObject(dc, GetStockObject(DC_BRUSH));
            SelectObject(dc, GetStockObject(DC_PEN));
            SetDCBrushColor(dc, frame.color);
            SetDCPenColor(dc, frame.color);
            let width = frame.thumb.right - frame.thumb.left;
            let drawn = RoundRect(
                dc,
                bounds.left + frame.thumb.left,
                bounds.top + frame.thumb.top,
                bounds.left + frame.thumb.right,
                bounds.top + frame.thumb.bottom,
                width,
                width,
            );
            let restored = RestoreDC(dc, saved);
            ensure!(
                filled != 0 && drawn != 0 && restored != 0,
                "Could not print the scrollbar"
            );
        }
        self.paints.set(self.paints.get() + 1);
        Ok(())
    }

    fn record(&self, window: HWND, message: u32, wparam: WPARAM, phase: &str) {
        if !matches!(
            message,
            WM_NCPAINT
                | WM_PAINT
                | WM_PRINT
                | WM_COMMAND
                | WM_VSCROLL
                | WM_MOUSEWHEEL
                | WM_NCLBUTTONDOWN
                | WM_NCLBUTTONDBLCLK
                | WM_NCLBUTTONUP
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_NCMOUSEMOVE
                | WM_NCMOUSELEAVE
                | WM_MOUSEMOVE
                | WM_MOUSELEAVE
                | WM_CAPTURECHANGED
                | WM_CANCELMODE
                | WM_TIMER
                | WM_DPICHANGED
                | EM_LINESCROLL
                | EM_SCROLLCARET
        ) {
            return;
        }
        let geometry = self.info().map(|info| {
            (
                info.rcScrollBar.left,
                info.rcScrollBar.top,
                info.rcScrollBar.right,
                info.rcScrollBar.bottom,
                info.xyThumbTop,
                info.xyThumbBottom,
            )
        });
        let capture = unsafe { GetCapture() };
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_millis());
        // Query native state before borrowing: Win32 can synchronously re-enter the subclass.
        self.trace.borrow_mut().push(format!(
            "scrollbar trace: unix-ms={time:?}; {} {phase}; message={message:#06x}; wparam={wparam:#x}; capture={capture:p}; geometry={geometry:?}; pressed={}; prints={}; commits={}",
            if window == self.state().text.get() { "EDIT" } else { "parent" },
            self.pressed.get(),
            self.paints.get(),
            self.commits.get(),
        ));
    }

    fn new(state: &WindowState) -> Result<Box<Self>> {
        let windows = window_handles(state)?;
        let styles = [GWL_STYLE, GWL_EXSTYLE]
            .map(|index| unsafe { GetWindowLongPtrW(state.text.get(), index) });
        let scrollbar = Box::new(Self {
            state,
            composition: RefCell::new(Composition::new(state.window.get())?),
            pressed: Cell::new(false),
            hovered: Cell::new(false),
            paints: Cell::new(0),
            commits: Cell::new(0),
            failure: RefCell::new(None),
            trace: RefCell::new(Vec::new()),
        });
        let reference = (&*scrollbar as *const Self) as usize;
        for control in [state.window.get(), state.text.get()] {
            ensure!(
                unsafe { SetWindowSubclass(control, Some(observe), 71, reference) } != 0,
                "Could not observe native scrollbar repaint"
            );
        }
        scrollbar.sync();
        scrollbar.check()?;
        ensure!(
            window_handles(state)? == windows,
            "Composition created a child or thread-top-level HWND"
        );
        ensure!(
            [GWL_STYLE, GWL_EXSTYLE]
                .map(|index| unsafe { GetWindowLongPtrW(state.text.get(), index) })
                == styles,
            "Composition changed native EDIT styles"
        );
        Ok(scrollbar)
    }

    // Subclasses retain this address until removal; consuming an unboxed Self would move it.
    #[allow(clippy::boxed_local)]
    fn close(self: Box<Self>) -> Result<()> {
        self.check()?;
        for control in [self.state().window.get(), self.state().text.get()] {
            ensure!(
                unsafe { RemoveWindowSubclass(control, Some(observe), 71) } != 0,
                "Could not remove the scrollbar observer"
            );
        }
        self.composition.borrow().detach()
    }
}

impl Drop for Scrollbar {
    fn drop(&mut self) {
        let state = self.state();
        unsafe {
            for control in [state.window.get(), state.text.get()] {
                RemoveWindowSubclass(control, Some(observe), 71);
            }
        }
        if let Err(error) = self.composition.borrow().detach() {
            eprintln!("scrollbar composition cleanup failed: {error:#}");
        }
        unsafe {
            RedrawWindow(
                state.text.get(),
                null(),
                null_mut(),
                RDW_FRAME | RDW_INVALIDATE | RDW_UPDATENOW | RDW_NOCHILDREN,
            );
        }
        for line in self.trace.get_mut() {
            eprintln!("{line}");
        }
    }
}

// SAFETY: The boxed probe outlives these same-thread callbacks and removes both subclasses on drop.
unsafe extern "system" fn observe(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    reference: usize,
) -> LRESULT {
    let scrollbar = unsafe { &*(reference as *const Scrollbar) };
    scrollbar.record(window, message, wparam, "enter");
    let editor = window == scrollbar.state().text.get();
    let tracking = editor
        && matches!(message, WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK)
        && wparam == HTVSCROLL as usize;
    if tracking {
        scrollbar.pressed.set(true);
        scrollbar.sync();
    }
    if editor && message == WM_NCMOUSEMOVE {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE | TME_NONCLIENT,
            hwndTrack: window,
            dwHoverTime: 0,
        };
        unsafe { TrackMouseEvent(&mut tracking) };
        if !scrollbar.hovered.replace(true) {
            scrollbar.sync();
        }
    }
    if editor && message == WM_NCMOUSELEAVE && scrollbar.hovered.replace(false) {
        scrollbar.sync();
    }
    let result = if editor
        && message == WM_PRINT
        && lparam & PRF_NONCLIENT as isize != 0
        && scrollbar.info().is_some()
    {
        // WM_PRINT does not capture DComp. Explicitly print matching geometry into the DIB;
        // only the separate CopyFromScreen driver can validate visible composition.
        let result =
            unsafe { DefSubclassProc(window, message, wparam, lparam & !(PRF_NONCLIENT as isize)) };
        if let Err(error) = scrollbar.print(wparam as HDC) {
            scrollbar.fail(error);
        }
        result
    } else {
        // In particular, native non-client painting and all modal tracking still execute.
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    };
    if tracking || (editor && message == WM_CAPTURECHANGED) {
        scrollbar.pressed.set(false);
        scrollbar.sync();
    }
    if (editor
        && matches!(
            message,
            WM_VSCROLL
                | WM_MOUSEWHEEL
                | EM_LINESCROLL
                | EM_SCROLLCARET
                | WM_PAINT
                | WM_NCPAINT
                | WM_WINDOWPOSCHANGED
                | WM_STYLECHANGED
        ))
        || (!editor
            && (matches!(message, WM_DPICHANGED | WM_WINDOWPOSCHANGED | WM_PAINT)
                || (message == WM_COMMAND && lparam as HWND == scrollbar.state().text.get())))
    {
        // Nested WM_VSCROLL/SB_THUMBTRACK is observed while DefSubclassProc is still inside
        // its native modal loop. Never infer a thumb position from the 16-bit message value.
        // Unchanged geometry/colors are a no-op, including unrelated WM_PAINT notifications.
        scrollbar.sync();
    }
    scrollbar.record(window, message, wparam, "exit");
    result
}

fn window_handles(state: &WindowState) -> Result<[Vec<usize>; 2]> {
    let mut handles = [Vec::new(), Vec::new()];
    // SAFETY: Each enumeration is synchronous and only appends live HWND values to its Vec.
    unsafe {
        EnumChildWindows(
            state.window.get(),
            Some(collect_window),
            (&mut handles[0] as *mut Vec<usize>) as isize,
        );
        let thread = GetWindowThreadProcessId(state.window.get(), null_mut());
        ensure!(
            EnumThreadWindows(
                thread,
                Some(collect_window),
                (&mut handles[1] as *mut Vec<usize>) as isize,
            ) != 0,
            "Could not enumerate native preview thread windows"
        );
    }
    for list in &mut handles {
        list.sort_unstable();
    }
    Ok(handles)
}

unsafe extern "system" fn collect_window(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: Both synchronous enumerations pass a live, uniquely borrowed Vec<usize>.
    unsafe { &mut *(data as *mut Vec<usize>) }.push(window as usize);
    1
}

/// Exercise both the print-only checks and separate real desktop input/UIA gates.
pub(super) fn exercise(
    preview: &mut NativePreview,
    artifacts: Option<&std::path::Path>,
    cleanup_fault: bool,
) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = artifacts.unwrap_or_else(|| temporary.path());
    preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), true, None)?;
    if cleanup_fault {
        return desktop_driver(preview, directory, "input-cleanup");
    }
    let editor = preview.state.text.get();
    let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
    unsafe { SendMessageW(editor, EM_LINESCROLL, 0, -current) };
    let baseline = directory.join("probe-scrollbar-native-baseline.png");
    preview.smoke_capture(&baseline)?;
    let scrollbar = Scrollbar::new(&preview.state)?;
    if preview.state.style.get().high_contrast {
        ensure!(
            scrollbar.info().is_none(),
            "System colors enabled custom scrollbar drawing"
        );
        eprintln!(
            "scrollbar probe skipped: system-color fallback is active; no custom scrollbar validated"
        );
        anyhow::bail!("Desktop scrollbar gates require a non-high-contrast test desktop");
    }
    for first in [0, 7, 25] {
        let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
        unsafe { SendMessageW(editor, EM_LINESCROLL, 0, first - current) };
        ensure!(
            unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) } == first,
            "Probe changed native scroll position"
        );
        let info = scrollbar
            .info()
            .ok_or_else(|| anyhow!("Probe has no native scrollbar"))?;
        let bounds = info.rcScrollBar;
        let capture = directory.join(format!("probe-scrollbar-first-{first}.png"));
        preview.smoke_capture(&capture)?;
        scrollbar.check()?;
        let mut window = RECT::default();
        ensure!(
            unsafe { GetWindowRect(preview.state.window.get(), &mut window) } != 0,
            "Could not measure scrollbar capture"
        );
        let (frame, pixels) = read_capture(&capture)?;
        let pixel = |x: i32, y: i32| {
            let offset = (((y - window.top) * frame.width as i32 + x - window.left) * 4) as usize;
            &pixels[offset..offset + 3]
        };
        let middle = bounds.top + (info.xyThumbTop + info.xyThumbBottom) / 2;
        ensure!(
            pixel(bounds.left + 1, middle) == [0x1e; 3]
                && [[0x92; 3], [0xb0; 3]]
                    .iter()
                    .any(|color| pixel((bounds.left + bounds.right) / 2, middle) == color),
            "WM_PRINT lost the dark rail or the narrow thumb"
        );
        let thumb_width = (bounds.left..bounds.right)
            .filter(|x| pixel(*x, middle) != [0x1e; 3])
            .count() as i32;
        ensure!(
            thumb_width
                == preview
                    .state
                    .pixels(6)
                    .min(bounds.right - bounds.left - 2)
                    .max(1),
            "Scrollbar probe did not retain a six-DIP thumb within the native slot"
        );
        if first == 0 {
            let (original, original_pixels) = read_capture(&baseline)?;
            ensure!(
                original.width == frame.width && original.height == frame.height,
                "Scrollbar probe changed preview size"
            );
            for (index, (before, after)) in original_pixels
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0)
                .enumerate()
            {
                let x = window.left + (index as u32 % frame.width) as i32;
                let y = window.top + (index as u32 / frame.width) as i32;
                if x < bounds.left || x >= bounds.right || y < bounds.top || y >= bounds.bottom {
                    ensure!(
                        before == after,
                        "Scrollbar probe changed pixels outside the native slot"
                    );
                }
            }
        }
        let point = POINT {
            x: bounds.left + 1,
            y: bounds.top + (info.xyThumbTop + info.xyThumbBottom) / 2,
        };
        let packed = (point.x as u32 & 0xffff) | ((point.y as u32 & 0xffff) << 16);
        ensure!(
            unsafe { SendMessageW(editor, WM_NCHITTEST, 0, packed as isize) } == HTVSCROLL as isize,
            "Composition lost native scrollbar hit testing"
        );
        // Unlike sending WM_NCHITTEST, this asks Windows which real HWND is at the slot edge.
        ensure!(
            unsafe { WindowFromPoint(point) } == editor,
            "Native point hit testing did not select EDIT"
        );
    }
    let original = preview.state.style.replace(PreviewStyle::system());
    let paints = scrollbar.paints.get();
    scrollbar.sync();
    scrollbar.check()?;
    let native_fallback = scrollbar.info().is_none()
        && scrollbar.composition.borrow().frame.is_none()
        && scrollbar.paints.get() == paints
        && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize != 0;
    preview.state.style.set(original);
    ensure!(
        native_fallback,
        "System-color fallback lost the native scrollbar"
    );
    preview.update_text("短句", true, None)?;
    scrollbar.sync();
    scrollbar.check()?;
    ensure!(
        scrollbar.info().is_none()
            && scrollbar.composition.borrow().frame.is_none()
            && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize == 0,
        "Short text kept its scrollbar drawing or native slot"
    );
    let windows = window_handles(&preview.state)?;
    scrollbar.close()?;
    ensure!(
        window_handles(&preview.state)? == windows,
        "Removing composition changed native HWNDs"
    );
    for (ready, case) in [(true, "ready"), (false, "readonly")] {
        preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), ready, None)?;
        desktop_driver(preview, directory, &format!("native-{case}"))?;
        let windows = window_handles(&preview.state)?;
        let styles =
            [GWL_STYLE, GWL_EXSTYLE].map(|index| unsafe { GetWindowLongPtrW(editor, index) });
        let scrollbar = Scrollbar::new(&preview.state)?;
        let result = desktop_driver(preview, directory, case);
        let closed = scrollbar.close(); // Flush traces and detach even when the driver failed.
        ensure!(
            window_handles(&preview.state)? == windows
                && [GWL_STYLE, GWL_EXSTYLE]
                    .map(|index| unsafe { GetWindowLongPtrW(editor, index) })
                    == styles,
            "Removing composition changed native HWNDs or EDIT styles"
        );
        result?;
        closed?;
        desktop_driver(preview, directory, &format!("restored-{case}"))?;
    }
    eprintln!(
        "scrollbar probe: print, actual screen, native input/drag and managed UIA gates checked at actual 96 DPI; NOT enabled in production"
    );
    Ok(())
}

fn read_capture(path: &std::path::Path) -> Result<(png::OutputInfo, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::fs::File::open(path)?).read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut pixels)?;
    ensure!(
        frame.color_type == png::ColorType::Rgba && frame.bit_depth == png::BitDepth::Eight,
        "Scrollbar capture is not RGBA8"
    );
    pixels.truncate(frame.buffer_size());
    Ok((frame, pixels))
}

/// The separate client can drive the native modal tracking loop while this thread pumps UIA
/// requests. The non-GUI parent owns the sole deadline, process tree and input cleanup.
fn desktop_driver(preview: &NativePreview, directory: &std::path::Path, case: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let system = std::env::var_os("WINDIR").ok_or_else(|| anyhow!("WINDIR is missing"))?;
    let mut child = Command::new(
        std::path::Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    )
    .args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/platform/windows/preview/scrollbar_probe.ps1"),
    )
    .arg("-Window")
    .arg((preview.state.window.get() as usize).to_string())
    .arg("-Editor")
    .arg((preview.state.text.get() as usize).to_string())
    .arg("-OwnerProcess")
    .arg(std::process::id().to_string())
    .arg("-Directory")
    .arg(directory)
    .arg("-Case")
    .arg(case)
    .arg("-ThumbWidth")
    .arg(preview.state.pixels(6).to_string())
    .creation_flags(0x08000000) // CREATE_NO_WINDOW
    .stdin(Stdio::null())
    // The parent drains these handles even if the GUI cannot return from DispatchMessage.
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            eprintln!(
                "scrollbar client {case} finished in {:?}: {status}",
                started.elapsed()
            );
            ensure!(
                status.success(),
                "Desktop scrollbar driver failed ({case}): {status}"
            );
            return Ok(());
        }
        let mut message = MSG::default();
        // SAFETY: This is the window-owning thread, with no active state/RefCell borrow.
        unsafe {
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}
