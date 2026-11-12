//! Mean relative luminance (0–1) of a screen rectangle in device px, for the adaptive pill.
//! GDI capture into a small 32-bit DIB; nothing is stored or logged.
//!
//! The capture is a `BitBlt` from the screen DC (desktop composition result, *including* our
//! own topmost windows if they overlap the rectangle — callers probe a strip next to the pill).
//! Large rectangles are sampled on a grid of at most [`MAX_SAMPLES`] pixels.

use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
    SRCCOPY,
};

/// Largest rectangle captured (device px); bigger requests are clamped to their top-left part.
pub const MAX_SIDE: i32 = 1024;
/// Pixels averaged at most.
pub const MAX_SAMPLES: usize = 4096;

/// Mean relative luminance (WCAG, linearised sRGB) of the rectangle, `None` on failure or an
/// empty rectangle.
pub fn luminance(x: i32, y: i32, w: i32, h: i32) -> Option<f32> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let (w, h) = (w.min(MAX_SIDE), h.min(MAX_SIDE));
    let pixels = capture(x, y, w, h)?;
    mean_luminance_bgra(&pixels, w as usize, h as usize)
}

/// WCAG relative luminance of one sRGB colour.
pub fn relative_luminance(r: u8, g: u8, b: u8) -> f32 {
    fn lin(c: u8) -> f32 {
        let c = f32::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Mean luminance of a top-down BGRA buffer, sampling at most [`MAX_SAMPLES`] pixels.
pub(crate) fn mean_luminance_bgra(px: &[u8], w: usize, h: usize) -> Option<f32> {
    if w == 0 || h == 0 || px.len() < w * h * 4 {
        return None;
    }
    let total = w * h;
    let step = ((total as f64 / MAX_SAMPLES as f64).sqrt().ceil() as usize).max(1);
    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let i = (y * w + x) * 4;
            sum += f64::from(relative_luminance(px[i + 2], px[i + 1], px[i]));
            n += 1;
        }
    }
    (n > 0).then(|| (sum / n as f64) as f32)
}

struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: obtained from GetDC(null); released once.
        unsafe { ReleaseDC(std::ptr::null_mut(), self.0) };
    }
}
struct MemDc(HDC);
impl Drop for MemDc {
    fn drop(&mut self) {
        // SAFETY: created by CreateCompatibleDC; deleted once.
        unsafe { DeleteDC(self.0) };
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: created by CreateDIBSection and no longer selected into a DC (see `capture`).
        unsafe { DeleteObject(self.0 as HGDIOBJ) };
    }
}

/// Copy the screen rectangle into a top-down BGRA buffer.
fn capture(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    // SAFETY: GetDC(null) = DC of the whole virtual screen.
    let screen = ScreenDc(unsafe { GetDC(std::ptr::null_mut()) });
    if screen.0.is_null() {
        return None;
    }
    // SAFETY: valid source DC.
    let mem = MemDc(unsafe { CreateCompatibleDC(screen.0) });
    if mem.0.is_null() {
        return None;
    }
    // SAFETY: BITMAPINFO is plain data; zero is valid.
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: valid DC and header; `bits` receives a pointer to w*h*4 bytes owned by the bitmap.
    let dib = Bitmap(unsafe {
        CreateDIBSection(
            mem.0,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    });
    if dib.0.is_null() || bits.is_null() {
        return None;
    }
    // SAFETY: select our bitmap into our DC; the previous object is restored below.
    let old = unsafe { SelectObject(mem.0, dib.0 as HGDIOBJ) };
    // SAFETY: both DCs valid; the destination is exactly w×h.
    let ok = unsafe { BitBlt(mem.0, 0, 0, w, h, screen.0, x, y, SRCCOPY) } != 0;
    let out = if ok {
        // SAFETY: the DIB section holds w*h*4 bytes (32 bpp, rows are DWORD aligned already).
        Some(
            unsafe { std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize) }.to_vec(),
        )
    } else {
        None
    };
    // SAFETY: restore the DC's original bitmap before the DIB is deleted.
    unsafe { SelectObject(mem.0, old) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_formula() {
        assert!(relative_luminance(0, 0, 0).abs() < 1e-6);
        assert!((relative_luminance(255, 255, 255) - 1.0).abs() < 1e-4);
        // #f5f5f7 ≈ 0.914 (the light pill's L in the spec).
        assert!((relative_luminance(0xf5, 0xf5, 0xf7) - 0.914).abs() < 0.005);
        // Green weighs most.
        assert!(relative_luminance(0, 255, 0) > relative_luminance(255, 0, 0));
    }

    #[test]
    fn mean_of_a_buffer() {
        // 2×1: black + white (BGRA) → 0.5.
        let px = [0, 0, 0, 255, 255, 255, 255, 255];
        let m = mean_luminance_bgra(&px, 2, 1).unwrap();
        assert!((m - 0.5).abs() < 1e-4);
        assert_eq!(mean_luminance_bgra(&px, 3, 1), None);
        assert_eq!(mean_luminance_bgra(&[], 0, 0), None);
        // Large uniform buffer is sampled, same mean.
        let big = vec![255u8; 300 * 300 * 4];
        assert!((mean_luminance_bgra(&big, 300, 300).unwrap() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn empty_rect_is_none() {
        assert_eq!(luminance(0, 0, 0, 10), None);
        assert_eq!(luminance(0, 0, 10, -1), None);
    }

    /// One 1×1 capture to check the API path returns; the value is neither stored nor printed.
    #[test]
    fn one_pixel_capture_returns() {
        if let Some(l) = luminance(0, 0, 1, 1) {
            assert!((0.0..=1.0).contains(&l));
        }
    }
}
