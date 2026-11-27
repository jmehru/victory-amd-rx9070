//! Borderless fullscreen for the window the picture is shown in
//! (`VICTORY_FULLSCREEN=1`, or `fullscreen = true` under `[window]` in
//! `victory.toml`; F11 and Alt+Enter switch while the title runs).
//!
//! The window loses its frame and takes the monitor it is on; back, it
//! takes its frame and place again. The swap chain keeps the picture's size
//! and the system scales it to the window, so nothing of the present path
//! changes. On a monitor of another shape than the picture the window is
//! the largest rectangle of the picture's shape, centred, over a black
//! window that covers the rest of the monitor.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

/// Whether the window starts fullscreen ([`set`]).
static WANTED: AtomicBool = AtomicBool::new(false);

/// The picture's size, for its shape (the swap chain's; 16:9 before one is
/// made).
static SHAPE: [AtomicU32; 2] = [AtomicU32::new(16), AtomicU32::new(9)];

/// Says whether the window starts fullscreen: from the environment or
/// `victory.toml`, before the title runs.
pub fn set(on: bool) {
    WANTED.store(on, Relaxed);
}

/// The largest rectangle of the shape `picture` centred in `monitor`
/// (left, top, right, bottom): left, top, width, height.
fn fitted(monitor: (i32, i32, i32, i32), picture: (u32, u32)) -> (i32, i32, i32, i32) {
    let (w, h) = (i64::from(monitor.2 - monitor.0), i64::from(monitor.3 - monitor.1));
    let (pw, ph) = (i64::from(picture.0.max(1)), i64::from(picture.1.max(1)));
    let (fw, fh) = if w * ph <= h * pw { (w, w * ph / pw) } else { (h * pw / ph, h) };
    (monitor.0 + ((w - fw) / 2) as i32, monitor.1 + ((h - fh) / 2) as i32, fw as i32, fh as i32)
}

#[cfg(windows)]
mod imp {
    use super::{Mutex, Relaxed, SHAPE, WANTED, fitted};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetMonitorInfoW, GetStockObject, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F11, VK_MENU, VK_RETURN};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    /// While fullscreen: the window's style and place from before, and the
    /// black window behind it if the monitor has another shape.
    struct Before {
        style: isize,
        rect: RECT,
        behind: Option<isize>,
    }

    static BEFORE: Mutex<Option<Before>> = Mutex::new(None);

    extern "system" fn behind_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: the default handling for our window.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// A black window without a frame over `monitor`, made and kept by a
    /// thread of its own (a window's messages come to the thread that made
    /// it).
    fn behind(monitor: RECT) -> Option<isize> {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("victory-behind".into())
            .spawn(move || {
                // SAFETY: registers our class and creates a window of it,
                // then the standard message loop of this thread's window.
                unsafe {
                    let made = (|| -> windows::core::Result<HWND> {
                        let instance = GetModuleHandleW(None)?;
                        let class = WNDCLASSEXW {
                            cbSize: size_of::<WNDCLASSEXW>() as u32,
                            lpfnWndProc: Some(behind_proc),
                            hInstance: instance.into(),
                            lpszClassName: w!("VIctoryBehind"),
                            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                            ..Default::default()
                        };
                        RegisterClassExW(&class);
                        CreateWindowExW(
                            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                            w!("VIctoryBehind"),
                            w!(""),
                            WS_POPUP | WS_VISIBLE,
                            monitor.left,
                            monitor.top,
                            monitor.right - monitor.left,
                            monitor.bottom - monitor.top,
                            None,
                            None,
                            Some(instance.into()),
                            None,
                        )
                    })();
                    let ok = made.is_ok();
                    let _ = sender.send(made.ok().map(|h| h.0 as isize));
                    if ok {
                        let mut message = MSG::default();
                        while GetMessageW(&mut message, None, 0, 0).as_bool() {
                            DispatchMessageW(&message);
                        }
                    }
                }
            })
            .ok()?;
        receiver.recv().ok().flatten()
    }

    /// Fullscreen if windowed, windowed if fullscreen.
    fn switch(hwnd: HWND) {
        let mut before = BEFORE.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: plain calls on a live window of this process.
        unsafe {
            match before.take() {
                Some(was) => {
                    SetWindowLongPtrW(hwnd, GWL_STYLE, was.style);
                    let r = was.rect;
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_NOTOPMOST),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
                    );
                    if let Some(behind) = was.behind {
                        let _ = PostMessageW(Some(HWND(behind as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    tracing::info!("the window is windowed again (F11 or Alt+Enter: fullscreen)");
                }
                None => {
                    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                    if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
                        return;
                    }
                    let mut rect = RECT::default();
                    if GetWindowRect(hwnd, &mut rect).is_err() {
                        return;
                    }
                    let m = info.rcMonitor;
                    let (x, y, w, h) = fitted((m.left, m.top, m.right, m.bottom), (SHAPE[0].load(Relaxed), SHAPE[1].load(Relaxed)));
                    let whole = (w, h) == (m.right - m.left, m.bottom - m.top);
                    let behind = if whole { None } else { behind(m) };
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
                    let _ = SetWindowPos(hwnd, Some(HWND_TOP), x, y, w, h, SWP_FRAMECHANGED | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS);
                    *before = Some(Before { style, rect, behind });
                    tracing::info!(x, y, width = w, height = h, letterboxed = !whole, "the window is fullscreen without a frame (F11 or Alt+Enter: windowed)");
                }
            }
        }
    }

    /// The window the picture is shown in is there: fullscreen now if asked
    /// for, and from now on F11 and Alt+Enter switch while it is in front.
    pub(crate) fn watch(hwnd: HWND, width: u32, height: u32) {
        SHAPE[0].store(width, Relaxed);
        SHAPE[1].store(height, Relaxed);
        let hwnd = hwnd.0 as isize;
        let _ = std::thread::Builder::new().name("victory-fullscreen".into()).spawn(move || {
            let hwnd = HWND(hwnd as *mut _);
            // Fullscreen at the start: from this thread, once the swap chain
            // is made and the first pictures are shown. Done on the thread
            // that presents (before the swap chain), the switch held it and
            // the title ended itself at once (a write to address 0 of one
            // of its threads, three runs in three, pg-fs1/fs2/x1).
            if WANTED.load(Relaxed) {
                std::thread::sleep(std::time::Duration::from_secs(2));
                switch(hwnd);
            }
            let down = |key: u16| {
                // SAFETY: a plain query of a key's state.
                unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
            };
            let mut held = false;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(30));
                // SAFETY: plain queries.
                unsafe {
                    if !IsWindow(Some(hwnd)).as_bool() {
                        return;
                    }
                    if GetForegroundWindow() != hwnd {
                        held = false;
                        continue;
                    }
                }
                let asked = down(VK_F11.0) || down(VK_MENU.0) && down(VK_RETURN.0);
                if asked && !held {
                    switch(hwnd);
                }
                held = asked;
            }
        });
    }
}

#[cfg(windows)]
pub(crate) use imp::watch;

#[cfg(test)]
mod tests {
    use super::fitted;

    /// On a monitor of the picture's shape the window takes all of it; on a
    /// wider one it is centred with bars left and right, on a taller one
    /// with bars above and below; a second monitor's origin is kept.
    #[test]
    fn the_window_is_the_largest_rectangle_of_the_pictures_shape() {
        assert_eq!(fitted((0, 0, 1920, 1080), (1920, 1080)), (0, 0, 1920, 1080));
        assert_eq!(fitted((0, 0, 2560, 1440), (1920, 1080)), (0, 0, 2560, 1440));
        assert_eq!(fitted((0, 0, 3440, 1440), (1920, 1080)), (440, 0, 2560, 1440));
        assert_eq!(fitted((0, 0, 1920, 1200), (1920, 1080)), (0, 60, 1920, 1080));
        assert_eq!(fitted((1920, 0, 3840, 1200), (1280, 720)), (1920, 60, 1920, 1080));
    }
}
