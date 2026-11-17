//! 自定义系统光标（目前只有旋转光标）。GPUI 没有自定义光标的接口，这里在 Windows 上直接调系统：
//!
//! - 给窗口挂一个消息拦截（subclass）：自定义光标生效时，系统每次问「光标设成什么」
//!   （`WM_SETCURSOR`）都由这里回答，GPUI 就不会把它改回去；
//! - 光标图案由 [`crate::rotate_cursor`] 画，按朝向分 24 档缓存。
//!
//! 别的系统、或者拿不到窗口句柄（无界面测试）时什么都不做，调用方退回 GPUI 自带的光标。

use gpui_kit::Window;

/// 给窗口挂上拦截。拿不到系统窗口句柄（无界面测试、非 Windows）时返回 `false`，之后的调用都是空操作。
pub(crate) fn install(window: &Window) -> bool {
    imp::install(window)
}

/// 显示旋转光标：朝向 `angle_deg`（鼠标相对画面中心的方向），按窗口缩放比例放大。
/// 没装上拦截时返回 `false`（调用方用 GPUI 自带的光标代替）。
pub(crate) fn show_rotate(angle_deg: f32, scale: f32) -> bool {
    imp::show_rotate(angle_deg, scale)
}

/// 不再用自定义光标，交还给 GPUI。
pub(crate) fn clear() {
    imp::clear();
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "直接调 Win32：GPUI 没有自定义光标的接口")]
mod imp {
    use crate::rotate_cursor::{BASE_SIZE, STEP_DEG, bucket, rotate_cursor_image};
    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, HCURSOR, HTCLIENT, ICONINFO, SetCursor, WM_SETCURSOR,
    };

    /// 拦截的编号（同一窗口上区分不同拦截用）。
    const SUBCLASS_ID: usize = 0x7665_6375; // "vecu"

    /// 是否装上了拦截。
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    /// 当前要显示的自定义光标（`HCURSOR` 的数值；0 = 不用，交给 GPUI）。
    static CURRENT: AtomicUsize = AtomicUsize::new(0);
    /// 已经做好的光标：（边长，朝向档位）→ `HCURSOR` 的数值。整个进程用，不释放（最多几十个）。
    static CACHE: Mutex<Option<HashMap<(u32, u32), usize>>> = Mutex::new(None);

    pub(super) fn install(window: &Window) -> bool {
        // GPUI 的 Window 有同名的自有方法（返回 GPUI 自己的句柄），这里要的是系统句柄
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return false;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return false;
        };
        let hwnd = h.hwnd.get() as HWND;
        // SAFETY: hwnd 是 GPUI 刚给出的、本线程创建的有效窗口句柄；subclass_proc 的签名符合
        // SUBCLASSPROC，且只读全局原子变量，生命周期覆盖整个进程。
        let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) } != 0;
        if ok {
            INSTALLED.store(true, Ordering::Relaxed);
        }
        ok
    }

    pub(super) fn show_rotate(angle_deg: f32, scale: f32) -> bool {
        if !INSTALLED.load(Ordering::Relaxed) {
            return false;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "缩放后的光标边长夹在 32–128"
        )]
        let size = ((BASE_SIZE as f32) * scale).round().clamp(32.0, 128.0) as u32;
        let slot = bucket(angle_deg);
        let Some(cursor) = cached(size, slot) else {
            return false;
        };
        CURRENT.store(cursor, Ordering::Relaxed);
        // SAFETY: cursor 是 CreateIconIndirect 创建、从不释放的有效光标句柄；在 UI 线程调用。
        unsafe { SetCursor(cursor as HCURSOR) };
        true
    }

    pub(super) fn clear() {
        CURRENT.store(0, Ordering::Relaxed);
    }

    fn cached(size: u32, slot: u32) -> Option<usize> {
        let hit = CACHE
            .lock()
            .ok()?
            .as_ref()
            .and_then(|m| m.get(&(size, slot)).copied());
        if hit.is_some() {
            return hit;
        }
        #[expect(clippy::cast_precision_loss, reason = "档位很小")]
        let made = create(size, slot as f32 * STEP_DEG)?;
        CACHE
            .lock()
            .ok()?
            .get_or_insert_with(HashMap::new)
            .insert((size, slot), made);
        Some(made)
    }

    /// 用画好的图案做一个带透明度的光标，热点在正中间。
    fn create(size: u32, angle_deg: f32) -> Option<usize> {
        let pixels = rotate_cursor_image(size, angle_deg);
        let dim = i32::try_from(size).ok()?;
        let header = BITMAPINFOHEADER {
            biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).ok()?,
            biWidth: dim,
            // 负数 = 从上到下逐行，和图案的排列一致
            biHeight: -dim,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        // SAFETY: info 是完整初始化的 32 位位图描述；bits 接收系统分配的像素内存。
        let color = unsafe {
            CreateDIBSection(
                core::ptr::null_mut(),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                core::ptr::null_mut(),
                0,
            )
        };
        if color.is_null() || bits.is_null() {
            return None;
        }
        // SAFETY: 系统按 info 分配了 size×size×4 字节的像素内存，pixels 正好这么长；两块内存不重叠。
        unsafe {
            core::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        }
        // 单色掩码全 0：透明度完全由彩色位图的 alpha 决定。每行按 16 位对齐。
        let mask_bits = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
        // SAFETY: mask_bits 的长度满足单色位图每行 16 位对齐的要求。
        let mask = unsafe { CreateBitmap(dim, dim, 1, 1, mask_bits.as_ptr().cast()) };
        let icon = ICONINFO {
            fIcon: 0,
            xHotspot: size / 2,
            yHotspot: size / 2,
            hbmMask: mask,
            hbmColor: color,
        };
        // SAFETY: icon 里的两个位图都是上面刚创建的有效句柄；CreateIconIndirect 会复制它们，
        // 之后即可删掉原位图。
        let cursor = unsafe { CreateIconIndirect(&raw const icon) };
        // SAFETY: 两个位图句柄有效且不再使用。
        unsafe {
            DeleteObject(color);
            DeleteObject(mask);
        }
        (!cursor.is_null()).then_some(cursor as usize)
    }

    /// 窗口消息拦截：自定义光标生效、鼠标在窗口内容区时，由这里设光标并告诉系统「已处理」，
    /// GPUI 就不会把它换回去；其余消息原样交给 GPUI。
    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        // 低 16 位是鼠标所在的区域（窗口内容区 / 边框……）
        let hit_area = u32::try_from(lparam & 0xFFFF).unwrap_or(0);
        let current = CURRENT.load(Ordering::Relaxed);
        if msg == WM_SETCURSOR && hit_area == HTCLIENT && current != 0 {
            // SAFETY: current 是 CreateIconIndirect 创建、从不释放的有效光标句柄。
            unsafe { SetCursor(current as HCURSOR) };
            return 1;
        }
        // SAFETY: 参数原样转交给下一个窗口过程（GPUI 的）。
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}

#[cfg(not(windows))]
mod imp {
    use gpui_kit::Window;

    pub(super) const fn install(_window: &Window) -> bool {
        false
    }

    pub(super) const fn show_rotate(_angle_deg: f32, _scale: f32) -> bool {
        false
    }

    pub(super) const fn clear() {}
}
