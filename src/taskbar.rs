//! Taskbar unread badge: a small red count over the window's taskbar icon.
//!
//! Windows only (`ITaskbarList3::SetOverlayIcon`). On other platforms every
//! call here does nothing; macOS dock badges / Linux launcher counts may
//! come later.
//!
//! Each window owns a [`TaskbarBadge`] and calls [`TaskbarBadge::update`]
//! every frame with its own account's count. Only a change of the label
//! (1-9, 9+, none) or colour touches Windows: no timer, no polling. The
//! COM object and the badge icons are created lazily on the UI thread the
//! first time a badge is shown and freed by [`shutdown`] on exit. With the
//! setting off nothing is created and no COM call is made (apart from
//! clearing a badge that was showing when it was switched off).

/// Which window the badge goes on.
pub enum Target<'a> {
    /// The window's raw handle (main window, see [`frame_target`]).
    #[cfg_attr(not(windows), allow(dead_code))]
    Hwnd(isize),
    /// An account window (a deferred viewport, which has no `Frame`): found
    /// once by its title among this thread's windows, then remembered.
    Title(&'a str),
    /// No window (e.g. no handle available).
    None,
}

/// One window's badge.
#[derive(Default)]
pub struct TaskbarBadge {
    /// Label and colour as last set on the taskbar (`None` = no badge).
    shown: Option<(u8, [u8; 3])>,
    /// The window, once known (an `HWND` as an integer, so this is `Send`).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: isize,
}

impl TaskbarBadge {
    /// Show `count` (0 = clear). `enabled` is `Config::taskbar_badge`;
    /// `fill` the badge colour (`Palette::danger`). Cheap when nothing
    /// changed: one comparison.
    pub fn update(&mut self, target: Target<'_>, count: u32, enabled: bool, fill: [u8; 3]) {
        let want = crate::badge::label(count)
            .filter(|_| enabled)
            .map(|l| (l, fill));
        if want == self.shown {
            return;
        }
        #[cfg(windows)]
        {
            if self.hwnd == 0 || !win::is_window(self.hwnd) {
                self.hwnd = match target {
                    Target::Hwnd(h) => h,
                    Target::Title(t) => win::find_window(t),
                    Target::None => 0,
                };
                if self.hwnd == 0 {
                    // Window not created yet: try again next frame.
                    return;
                }
                win::claim(self.hwnd);
            }
            if win::set_overlay(self.hwnd, want) {
                self.shown = want;
            }
        }
        #[cfg(not(windows))]
        {
            // No taskbar badge on this platform.
            let _ = target;
            self.shown = want;
        }
    }
}

impl Drop for TaskbarBadge {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.hwnd != 0 {
            win::unclaim(self.hwnd);
        }
    }
}

/// The main window, from eframe's `Frame` (`Target::None` off Windows).
pub fn frame_target(frame: &eframe::Frame) -> Target<'static> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(RawWindowHandle::Win32(h)) = frame.window_handle().map(|h| h.as_raw()) {
            return Target::Hwnd(h.hwnd.get());
        }
    }
    let _ = frame;
    Target::None
}

/// A colour as the badge's `[r, g, b]`.
pub fn rgb(c: eframe::egui::Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

/// Free the badge icons and the COM object (call once on exit).
pub fn shutdown() {
    #[cfg(windows)]
    win::shutdown();
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows_sys::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, EnumThreadWindows, GetSystemMetrics, GetWindowTextW,
        HICON, ICONINFO, IsWindow, SM_CXSMICON,
    };
    use windows_sys::core::{BOOL, GUID, HRESULT};

    const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
    const IID_ITASKBAR_LIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);

    /// `ITaskbarList3`'s vtable up to `SetOverlayIcon` (IUnknown,
    /// ITaskbarList, ITaskbarList2, then ITaskbarList3 in declaration order).
    #[repr(C)]
    struct TaskbarListVtbl {
        _query_interface: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        hr_init: unsafe extern "system" fn(*mut c_void) -> HRESULT,
        // AddTab, DeleteTab, ActivateTab, SetActiveAlt.
        _list: [usize; 4],
        // MarkFullscreenWindow.
        _list2: [usize; 1],
        // SetProgressValue, SetProgressState, RegisterTab, UnregisterTab,
        // SetTabOrder, SetTabActive, ThumbBarAddButtons,
        // ThumbBarUpdateButtons, ThumbBarSetImageList.
        _list3: [usize; 9],
        set_overlay_icon:
            unsafe extern "system" fn(*mut c_void, HWND, HICON, *const u16) -> HRESULT,
    }

    /// Lazily created shell state (UI thread only).
    struct Shell {
        /// `ITaskbarList3*` (null until created).
        list: *mut c_void,
        /// We initialised COM on this thread and must uninitialise it.
        com: bool,
        /// Creating the COM object failed: don't retry every frame.
        failed: bool,
        /// Badge icons by (label, colour).
        icons: Vec<((u8, [u8; 3]), HICON)>,
        /// Windows that have a badge, so an account window's title lookup
        /// never picks another window with the same title.
        claimed: Vec<isize>,
    }

    impl Default for Shell {
        fn default() -> Self {
            Self {
                list: null_mut(),
                com: false,
                failed: false,
                icons: Vec::new(),
                claimed: Vec::new(),
            }
        }
    }

    thread_local! {
        static SHELL: RefCell<Shell> = RefCell::default();
    }

    pub fn is_window(hwnd: isize) -> bool {
        // SAFETY: IsWindow accepts any value.
        unsafe { IsWindow(hwnd as HWND) != 0 }
    }

    pub fn claim(hwnd: isize) {
        SHELL.with_borrow_mut(|s| {
            if !s.claimed.contains(&hwnd) {
                s.claimed.push(hwnd);
            }
        });
    }

    pub fn unclaim(hwnd: isize) {
        // `try_with`: may run while thread locals are being torn down.
        let _ = SHELL.try_with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.claimed.retain(|&h| h != hwnd);
            }
        });
    }

    struct Search {
        title: Vec<u16>,
        claimed: Vec<isize>,
        found: isize,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Search` passed to EnumThreadWindows.
        let search = unsafe { &mut *(lparam as *mut Search) };
        if search.claimed.contains(&(hwnd as isize)) {
            return 1;
        }
        let mut buf = [0u16; 256];
        // SAFETY: `buf` is valid for its length.
        let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if n > 0 && buf[..n as usize] == search.title[..] {
            search.found = hwnd as isize;
            return 0;
        }
        1
    }

    /// This thread's (the UI thread's) top-level window titled `title` that
    /// has no badge owner yet. 0 if none.
    pub fn find_window(title: &str) -> isize {
        let claimed = SHELL.with_borrow(|s| s.claimed.clone());
        let mut search = Search {
            title: title.encode_utf16().collect(),
            claimed,
            found: 0,
        };
        // SAFETY: the callback only uses `search` during this call.
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(enum_proc),
                &mut search as *mut Search as LPARAM,
            );
        }
        search.found
    }

    impl Shell {
        /// The `ITaskbarList3`, created on first use.
        fn list(&mut self) -> Option<*mut c_void> {
            if !self.list.is_null() {
                return Some(self.list);
            }
            if self.failed {
                return None;
            }
            // SAFETY: plain COM calls; the pointer is checked before use.
            unsafe {
                // winit has usually initialised OLE on this thread already
                // (S_FALSE); a different mode means COM is usable anyway.
                let hr = CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED as u32);
                self.com = hr >= 0;
                let mut ptr: *mut c_void = null_mut();
                let hr = CoCreateInstance(
                    &CLSID_TASKBAR_LIST,
                    null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_ITASKBAR_LIST3,
                    &mut ptr,
                );
                if hr < 0 || ptr.is_null() {
                    self.failed = true;
                    return None;
                }
                let vtbl = &**(ptr as *mut *const TaskbarListVtbl);
                if (vtbl.hr_init)(ptr) < 0 {
                    (vtbl.release)(ptr);
                    self.failed = true;
                    return None;
                }
                self.list = ptr;
            }
            Some(self.list)
        }

        fn icon(&mut self, key: (u8, [u8; 3])) -> Option<HICON> {
            if let Some((_, h)) = self.icons.iter().find(|(k, _)| *k == key) {
                return Some(*h);
            }
            let h = make_icon(key.0, key.1)?;
            self.icons.push((key, h));
            Some(h)
        }
    }

    /// Set (or with `None` clear) the overlay. False if it didn't take
    /// (the caller tries again on a later frame).
    pub fn set_overlay(hwnd: isize, want: Option<(u8, [u8; 3])>) -> bool {
        SHELL.with_borrow_mut(|s| {
            if want.is_none() && s.list.is_null() {
                // Never showed a badge: nothing to clear, nothing to create.
                return true;
            }
            let Some(list) = s.list() else {
                // No taskbar list (e.g. Explorer not running): give up quietly.
                return true;
            };
            let (icon, text) = match want {
                Some(key) => {
                    let Some(icon) = s.icon(key) else {
                        return true;
                    };
                    let text = format!("{} unread\0", crate::badge::label_text(key.0));
                    (icon, text.encode_utf16().collect::<Vec<u16>>())
                }
                None => (null_mut(), vec![0u16]),
            };
            // SAFETY: `list` is a live ITaskbarList3; `text` is NUL-terminated.
            let hr = unsafe {
                let vtbl = &**(list as *mut *const TaskbarListVtbl);
                (vtbl.set_overlay_icon)(list, hwnd as HWND, icon, text.as_ptr())
            };
            hr >= 0
        })
    }

    /// Draw the badge and turn it into an icon of the small-icon size.
    fn make_icon(label: u8, fill: [u8; 3]) -> Option<HICON> {
        // SAFETY: GetSystemMetrics has no preconditions.
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as u32;
        let rgba = crate::badge::render(label, size, fill);
        // 32-bit DDB rows are BGRA, top to bottom; the alpha is used as is.
        let bgra: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        // 1-bit mask (ignored with a 32-bit alpha bitmap), rows WORD-aligned.
        let mask = vec![0u8; size.div_ceil(16) as usize * 2 * size as usize];
        // SAFETY: the buffers outlive the calls; the bitmaps are deleted
        // after CreateIconIndirect copied them.
        unsafe {
            let color = CreateBitmap(size as i32, size as i32, 1, 32, bgra.as_ptr().cast());
            let mask = CreateBitmap(size as i32, size as i32, 1, 1, mask.as_ptr().cast());
            let info = ICONINFO {
                fIcon: 1,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask,
                hbmColor: color,
            };
            let icon = if color.is_null() || mask.is_null() {
                null_mut()
            } else {
                CreateIconIndirect(&info)
            };
            if !color.is_null() {
                DeleteObject(color);
            }
            if !mask.is_null() {
                DeleteObject(mask);
            }
            (!icon.is_null()).then_some(icon)
        }
    }

    pub fn shutdown() {
        let _ = SHELL.try_with(|s| {
            let Ok(mut s) = s.try_borrow_mut() else {
                return;
            };
            // SAFETY: icons and the list were created by us on this thread.
            unsafe {
                for (_, h) in s.icons.drain(..) {
                    DestroyIcon(h);
                }
                if !s.list.is_null() {
                    let vtbl = &**(s.list as *mut *const TaskbarListVtbl);
                    (vtbl.release)(s.list);
                    s.list = null_mut();
                }
                if std::mem::take(&mut s.com) {
                    CoUninitialize();
                }
            }
            s.failed = true;
        });
    }
}
