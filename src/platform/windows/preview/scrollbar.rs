//! Decorate the native EDIT scrollbar without owning its input or accessibility model.

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
#[cfg(any(test, feature = "integration"))]
use windows_sys::Win32::Graphics::Gdi::IntersectClipRect;
use windows_sys::Win32::Graphics::Gdi::{RDW_NOCHILDREN, ScreenToClient};
use windows_sys::Win32::UI::Controls::STATE_SYSTEM_INVISIBLE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};

#[cfg(test)]
#[path = "scrollbar_probe.rs"]
mod probe;

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
    origin: (i32, i32),
}

impl Composition {
    fn new(window: HWND) -> Result<Self> {
        // WARP is sufficient for this small surface and does not require a hardware GPU.
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
                origin: (0, 0),
            })
        }
    }

    /// Geometry and colors are the cache key: an unrelated native paint cannot request repair.
    fn update(&mut self, frame: Option<Frame>, origin: POINT) -> Result<bool> {
        if frame == self.frame && (origin.x, origin.y) == self.origin {
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
        self.origin = (origin.x, origin.y);
        Ok(true)
    }

    fn detach(&self) -> Result<()> {
        unsafe {
            self.target.SetRoot(None::<&IDCompositionVisual>)?;
            self.device.Commit()?;
            self.device.WaitForCommitCompletion()?;
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
/// NativePreview drops this guard before destroying its HWNDs and stable boxed state.
pub(super) struct Scrollbar {
    state: *const WindowState,
    composition: RefCell<Option<Composition>>,
    pressed: Cell<bool>,
    #[cfg(test)]
    paints: Cell<u64>,
    #[cfg(test)]
    commits: Cell<u64>,
    failure: RefCell<Option<String>>,
    #[cfg(test)]
    trace: RefCell<Vec<String>>,
}

impl Scrollbar {
    fn state(&self) -> &WindowState {
        // SAFETY: The constructor requires stable state and live HWNDs until this guard is dropped.
        unsafe { &*self.state }
    }

    fn info(&self) -> Option<SCROLLBARINFO> {
        let state = self.state();
        let mut info = SCROLLBARINFO {
            cbSize: size_of::<SCROLLBARINFO>() as u32,
            ..Default::default()
        };
        // A hidden EDIT can retain geometry after its session has ended.
        if state.session_id.get().is_some()
            && !state.style.get().high_contrast
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

    pub(super) fn sync(&self) {
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
            // A nested notification must not panic or interfere with an active graphics update.
            let Ok(mut composition) = self.composition.try_borrow_mut() else {
                return Ok(());
            };
            if let Some(composition) = composition.as_mut()
                && composition.update(frame, origin)?
            {
                #[cfg(test)]
                self.commits.set(self.commits.get() + 1);
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
        }
    }

    fn fail(&self, error: anyhow::Error) {
        // Keep callbacks alive, but use the native scrollbar for the rest of this HWND's life.
        if self.failure.borrow().is_some() {
            return;
        }
        *self.failure.borrow_mut() = Some(format!("{error:#}"));
        tracing::warn!(%error, "Scrollbar composition failed; restoring native scrollbar");
        if let Err(error) = self.detach() {
            tracing::warn!(%error, "Could not detach failed scrollbar composition");
        }
        unsafe {
            RedrawWindow(
                self.state().text.get(),
                null(),
                null_mut(),
                RDW_FRAME | RDW_INVALIDATE | RDW_NOCHILDREN,
            );
        }
    }

    fn detach(&self) -> Result<()> {
        // Release the RefCell borrow before COM teardown can run native callbacks.
        let composition = self.composition.borrow_mut().take();
        if let Some(composition) = composition {
            composition.detach()?;
        }
        Ok(())
    }

    fn check(&self) -> Result<()> {
        if let Some(error) = &*self.failure.borrow() {
            anyhow::bail!("Scrollbar composition failed: {error}");
        }
        Ok(())
    }

    /// Manual WM_PRINT evidence only; this GDI path never paints the actual screen.
    #[cfg(any(test, feature = "integration"))]
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
        #[cfg(test)]
        self.paints.set(self.paints.get() + 1);
        Ok(())
    }

    /// Attach once on the HWND-owning thread; drop before destroying either HWND.
    ///
    /// # Safety
    /// The state must keep its stable address and both HWNDs alive until the guard is dropped.
    /// Only one guard may target the parent, and it must not be dropped from an active callback.
    pub(super) unsafe fn new(state: &WindowState) -> Result<Box<Self>> {
        let scrollbar = Box::new(Self {
            state,
            composition: RefCell::new(Some(Composition::new(state.window.get())?)),
            pressed: Cell::new(false),
            #[cfg(test)]
            paints: Cell::new(0),
            #[cfg(test)]
            commits: Cell::new(0),
            failure: RefCell::new(None),
            #[cfg(test)]
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
        Ok(scrollbar)
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
        if let Err(error) = self.detach() {
            tracing::warn!(%error, "Could not detach scrollbar composition");
        }
        unsafe {
            RedrawWindow(
                state.text.get(),
                null(),
                null_mut(),
                RDW_FRAME | RDW_INVALIDATE | RDW_NOCHILDREN,
            );
        }
        #[cfg(test)]
        for line in self.trace.get_mut() {
            eprintln!("{line}");
        }
    }
}

// SAFETY: NativePreview keeps the boxed guard and state alive, removing subclasses before teardown.
unsafe extern "system" fn observe(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    reference: usize,
) -> LRESULT {
    let scrollbar = unsafe { &*(reference as *const Scrollbar) };
    #[cfg(test)]
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
        scrollbar.sync();
    }
    if editor && message == WM_NCMOUSELEAVE {
        scrollbar.sync();
    }
    #[cfg(any(test, feature = "integration"))]
    let result = if editor
        && message == WM_PRINT
        && lparam & PRF_NONCLIENT as isize != 0
        && scrollbar.failure.borrow().is_none()
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
    #[cfg(not(any(test, feature = "integration")))]
    let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
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
            && (matches!(
                message,
                WM_DPICHANGED
                    | WM_WINDOWPOSCHANGED
                    | WM_PAINT
                    | WM_SETTINGCHANGE
                    | WM_SYSCOLORCHANGE
                    | WM_THEMECHANGED
            ) || (message == WM_COMMAND && lparam as HWND == scrollbar.state().text.get())))
    {
        // Nested WM_VSCROLL/SB_THUMBTRACK is observed while DefSubclassProc is still inside
        // its native modal loop. Never infer a thumb position from the 16-bit message value.
        // Unchanged geometry/colors are a no-op, including unrelated WM_PAINT notifications.
        scrollbar.sync();
    }
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(window, Some(observe), 71) };
    }
    #[cfg(test)]
    scrollbar.record(window, message, wparam, "exit");
    result
}
