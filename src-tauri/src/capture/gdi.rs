//! GDI screen grab used only as a fallback when Desktop Duplication has no frame to give
//! (for example a brand-new duplication of a completely static screen). Slower than DXGI, but
//! it always returns the current pixels.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    SRCCOPY,
};

use crate::capture::dxgi::CapturedFrame;

/// Captures the physical-pixel rectangle `(x, y, width, height)` of the virtual desktop.
pub fn capture_rect(x: i32, y: i32, width: u32, height: u32) -> Result<CapturedFrame, String> {
    if width == 0 || height == 0 {
        return Err("empty capture rectangle".to_owned());
    }
    let started = Instant::now();
    let mut data = vec![0u8; width as usize * height as usize * 4];
    let rows = unsafe {
        let screen_dc = GetDC(std::ptr::null_mut());
        if screen_dc.is_null() {
            return Err("GetDC failed".to_owned());
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
        if memory_dc.is_null() || bitmap.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            if !memory_dc.is_null() {
                DeleteDC(memory_dc);
            }
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            return Err("could not allocate GDI capture surfaces".to_owned());
        }
        let previous = SelectObject(memory_dc, bitmap);
        let copied = BitBlt(memory_dc, 0, 0, width as i32, height as i32, screen_dc, x, y, SRCCOPY | CAPTUREBLT);

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let rows = if copied != 0 {
            GetDIBits(memory_dc, bitmap, 0, height, data.as_mut_ptr() as *mut _, &mut info, DIB_RGB_COLORS)
        } else {
            0
        };

        SelectObject(memory_dc, previous);
        DeleteObject(bitmap);
        DeleteDC(memory_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        rows
    };
    if rows == 0 {
        return Err("BitBlt/GetDIBits failed".to_owned());
    }

    // BGRA -> RGBA with opaque alpha.
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    Ok(CapturedFrame {
        width,
        height,
        stride: width * 4,
        data,
        timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
        capture_duration_us: started.elapsed().as_micros(),
    })
}

/// Renders one window's own contents (PrintWindow with PW_RENDERFULLCONTENT) into an RGBA frame,
/// whatever covers it on screen. `width` x `height` is the window's visible rectangle.
pub fn print_window(hwnd: windows_sys::Win32::Foundation::HWND, width: u32, height: u32) -> Result<CapturedFrame, String> {
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    let started = Instant::now();
    let mut data = vec![0u8; width as usize * height as usize * 4];
    let (printed, rows) = unsafe {
        let screen_dc = GetDC(std::ptr::null_mut());
        let memory_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
        if screen_dc.is_null() || memory_dc.is_null() || bitmap.is_null() {
            return Err("could not allocate GDI surfaces for PrintWindow".to_owned());
        }
        let previous = SelectObject(memory_dc, bitmap);
        // 2 = PW_RENDERFULLCONTENT: also gets DirectComposition and XAML surfaces.
        let printed = PrintWindow(hwnd, memory_dc, 2);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let rows = GetDIBits(memory_dc, bitmap, 0, height, data.as_mut_ptr() as *mut _, &mut info, DIB_RGB_COLORS);
        SelectObject(memory_dc, previous);
        DeleteObject(bitmap);
        DeleteDC(memory_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        (printed, rows)
    };
    if printed == 0 || rows == 0 {
        return Err("PrintWindow could not render the window".to_owned());
    }
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    Ok(CapturedFrame {
        width,
        height,
        stride: width * 4,
        data,
        timestamp_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
        capture_duration_us: started.elapsed().as_micros(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grabs_a_non_empty_frame_of_the_requested_size() {
        let frame = match capture_rect(0, 0, 64, 48) {
            Ok(frame) => frame,
            Err(error) => {
                println!("GDI capture unavailable here: {error}");
                return;
            }
        };
        assert_eq!((frame.width, frame.height), (64, 48));
        assert_eq!(frame.data.len(), 64 * 48 * 4);
        assert!(frame.data.chunks_exact(4).all(|p| p[3] == 255));
    }
}
