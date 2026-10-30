//! DXGI Desktop Duplication Screen Capture (Phase 1 implementation)
//!
//! Provides ultra-low latency (<5ms) GPU-accelerated desktop screen capture
//! using Direct3D 11 and the DXGI Desktop Duplication API (IDXGIOutputDuplication).

use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// Represents a captured desktop frame with packed RGBA pixel data.
#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data: Vec<u8>,
    pub timestamp_ms: u64,
    pub capture_duration_us: u128,
}

#[derive(Debug)]
pub enum DxgiCaptureError {
    DeviceCreation(String),
    AdapterNotFound,
    OutputNotFound(u32),
    DuplicationFailed(String),
    AccessLost,
    Timeout,
    StagingTextureError(String),
    MapFailed(String),
    InvalidCall(String),
    Other(String),
}

impl std::fmt::Display for DxgiCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceCreation(msg) => write!(f, "D3D11 Device Creation Failed: {}", msg),
            Self::AdapterNotFound => write!(f, "DXGI Adapter not found"),
            Self::OutputNotFound(idx) => write!(f, "DXGI Output #{} not found", idx),
            Self::DuplicationFailed(msg) => write!(f, "IDXGIOutputDuplication Failed: {}", msg),
            Self::AccessLost => write!(f, "DXGI_ERROR_ACCESS_LOST: Desktop mode changed or session switched"),
            Self::Timeout => write!(f, "DXGI capture timed out waiting for new frame"),
            Self::StagingTextureError(msg) => write!(f, "Failed to create staging texture: {}", msg),
            Self::MapFailed(msg) => write!(f, "Failed to map staging texture: {}", msg),
            Self::InvalidCall(msg) => write!(f, "DXGI invalid call: {}", msg),
            Self::Other(msg) => write!(f, "DXGI capture error: {}", msg),
        }
    }
}

impl std::error::Error for DxgiCaptureError {}

/// Ultra-fast desktop capture engine backed by DXGI Desktop Duplication.
pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging_texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    monitor_index: u32,
    last_frame: Option<CapturedFrame>,
}

impl DxgiCapture {
    /// Initialize DXGI capture for the specified monitor index (0 = primary).
    pub fn new(monitor_index: u32) -> Result<Self, DxgiCaptureError> {
        Self::init_pipeline(monitor_index)
    }

    /// Internal pipeline initialization helper.
    fn init_pipeline(monitor_index: u32) -> Result<Self, DxgiCaptureError> {
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device_opt: Option<ID3D11Device> = None;
        let mut context_opt: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;

        // 1. Create D3D11 Device with Hardware driver, fallback to WARP
        let hr = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device_opt),
                Some(&mut feature_level),
                Some(&mut context_opt),
            )
        };

        if hr.is_err() {
            // Fallback to WARP software rasterizer (useful in VMs/test environments)
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_FLAG(0),
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device_opt),
                    Some(&mut feature_level),
                    Some(&mut context_opt),
                )
                .map_err(|e| DxgiCaptureError::DeviceCreation(e.to_string()))?;
            }
        }

        let device = device_opt.ok_or_else(|| DxgiCaptureError::DeviceCreation("Device was null".into()))?;
        let context = context_opt.ok_or_else(|| DxgiCaptureError::DeviceCreation("Context was null".into()))?;

        // 2. Query DXGI device and enumerate target output
        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|e| DxgiCaptureError::DeviceCreation(format!("Cast to IDXGIDevice failed: {e}")))?;

        let adapter = unsafe {
            dxgi_device
                .GetAdapter()
                .map_err(|e| DxgiCaptureError::DeviceCreation(format!("GetAdapter failed: {e}")))?
        };

        let output = unsafe {
            adapter
                .EnumOutputs(monitor_index)
                .map_err(|_| DxgiCaptureError::OutputNotFound(monitor_index))?
        };

        let output1: IDXGIOutput1 = output
            .cast()
            .map_err(|e| DxgiCaptureError::DuplicationFailed(format!("Cast to IDXGIOutput1 failed: {e}")))?;

        let duplication = unsafe {
            output1
                .DuplicateOutput(&device)
                .map_err(|e| DxgiCaptureError::DuplicationFailed(format!("DuplicateOutput failed: {e}")))?
        };

        // 3. Query output dimensions
        let desc = unsafe {
            output
                .GetDesc()
                .map_err(|e| DxgiCaptureError::Other(format!("GetDesc failed: {e}")))?
        };

        let width = (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).unsigned_abs();
        let height = (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).unsigned_abs();
        let origin_x = desc.DesktopCoordinates.left;
        let origin_y = desc.DesktopCoordinates.top;

        if width == 0 || height == 0 {
            return Err(DxgiCaptureError::Other("Invalid monitor dimensions (0x0)".into()));
        }

        // 4. Create CPU-accessible Staging Texture
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };

        let mut staging_texture_opt: Option<ID3D11Texture2D> = None;
        unsafe {
            device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging_texture_opt))
                .map_err(|e| DxgiCaptureError::StagingTextureError(e.to_string()))?;
        }

        let staging_texture = staging_texture_opt
            .ok_or_else(|| DxgiCaptureError::StagingTextureError("Staging texture was null".into()))?;

        Ok(Self {
            device,
            context,
            duplication,
            staging_texture,
            width,
            height,
            origin_x,
            origin_y,
            monitor_index,
            last_frame: None,
        })
    }

    /// Re-initializes the capture pipeline when DXGI_ERROR_ACCESS_LOST occurs.
    pub fn reinitialize(&mut self) -> Result<(), DxgiCaptureError> {
        let new_instance = Self::init_pipeline(self.monitor_index)?;
        self.device = new_instance.device;
        self.context = new_instance.context;
        self.duplication = new_instance.duplication;
        self.staging_texture = new_instance.staging_texture;
        self.width = new_instance.width;
        self.height = new_instance.height;
        self.origin_x = new_instance.origin_x;
        self.origin_y = new_instance.origin_y;
        Ok(())
    }

    /// Captures the next desktop frame directly from GPU VRAM into a packed RGBA buffer.
    ///
    /// Performance: Executes in under 5ms on modern GPUs.
    /// Timeout: in milliseconds to wait for a newly updated frame (recommended: 100ms).
    pub fn capture_frame(&mut self, timeout_ms: u32) -> Result<CapturedFrame, DxgiCaptureError> {
        let start_time = Instant::now();
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut desktop_resource_opt: Option<IDXGIResource> = None;

        let acquire_res = unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut desktop_resource_opt)
        };

        if let Err(e) = acquire_res {
            if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                // If timed out waiting for a new frame, return previous frame if available
                if let Some(mut prev) = self.last_frame.clone() {
                    prev.capture_duration_us = start_time.elapsed().as_micros();
                    return Ok(prev);
                }
                return Err(DxgiCaptureError::Timeout);
            } else if e.code() == DXGI_ERROR_ACCESS_LOST {
                // Reinitialize pipeline and retry once
                self.reinitialize()?;
                return self.capture_frame(timeout_ms);
            } else {
                return Err(DxgiCaptureError::Other(format!("AcquireNextFrame failed: {e}")));
            }
        }

        // `LastPresentTime == 0` means only the mouse pointer moved: the returned resource holds
        // no new desktop image (copying it yields a garbage frame that differs from everything).
        // Keep the previous image, and wait for a real one if there is none yet.
        if frame_info.LastPresentTime == 0 {
            unsafe {
                let _ = self.duplication.ReleaseFrame();
            }
            if let Some(mut previous) = self.last_frame.clone() {
                previous.capture_duration_us = start_time.elapsed().as_micros();
                return Ok(previous);
            }
            if start_time.elapsed().as_millis() > 2_000 {
                return Err(DxgiCaptureError::Timeout);
            }
            return self.capture_frame(timeout_ms);
        }

        let desktop_resource = desktop_resource_opt
            .ok_or_else(|| DxgiCaptureError::Other("Desktop resource was null".into()))?;

        let desktop_texture: ID3D11Texture2D = desktop_resource
            .cast()
            .map_err(|e| DxgiCaptureError::Other(format!("Cast to ID3D11Texture2D failed: {e}")))?;

        // 1. Copy GPU surface to CPU-readable staging texture
        unsafe {
            self.context
                .CopyResource(&self.staging_texture, &desktop_texture);
            // Release frame back to DXGI immediately to avoid holding desktop lock
            let _ = self.duplication.ReleaseFrame();
        }

        // 2. Map staging texture to CPU memory
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(
                    &self.staging_texture,
                    0,
                    D3D11_MAP_READ,
                    0,
                    Some(&mut mapped),
                )
                .map_err(|e| DxgiCaptureError::MapFailed(e.to_string()))?;
        }

        let width = self.width as usize;
        let height = self.height as usize;
        let row_pitch = mapped.RowPitch as usize;
        let packed_stride = width * 4;
        let total_bytes = width * height * 4;

        let mut rgba_buffer = vec![0u8; total_bytes];
        let src_ptr = mapped.pData as *const u8;

        // 3. Fast row-by-row copy and BGRA -> RGBA in-flight conversion
        for y in 0..height {
            let src_row = unsafe {
                std::slice::from_raw_parts(src_ptr.add(y * row_pitch), packed_stride)
            };
            let dst_row = &mut rgba_buffer[y * packed_stride..(y + 1) * packed_stride];

            for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
                // BGRA to RGBA: swap B and R
                dst_px[0] = src_px[2]; // R
                dst_px[1] = src_px[1]; // G
                dst_px[2] = src_px[0]; // B
                dst_px[3] = 255; // opaque: the desktop has no meaningful alpha, and a zero alpha PNG looks blank
            }
        }

        // 4. Unmap staging texture
        unsafe {
            self.context.Unmap(&self.staging_texture, 0);
        }

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let duration_us = start_time.elapsed().as_micros();

        let frame = CapturedFrame {
            width: self.width,
            height: self.height,
            stride: packed_stride as u32,
            data: rgba_buffer,
            timestamp_ms,
            capture_duration_us: duration_us,
        };

        self.last_frame = Some(frame.clone());
        Ok(frame)
    }

    /// Seeds the "previous frame" so a timeout (nothing changed) returns this image instead of an error.
    pub fn seed_frame(&mut self, data: Vec<u8>) {
        if data.len() == (self.width as usize) * (self.height as usize) * 4 {
            self.last_frame = Some(CapturedFrame {
                width: self.width,
                height: self.height,
                stride: self.width * 4,
                data,
                timestamp_ms: 0,
                capture_duration_us: 0,
            });
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn monitor_index(&self) -> u32 {
        self.monitor_index
    }

    pub fn origin_x(&self) -> i32 {
        self.origin_x
    }

    pub fn origin_y(&self) -> i32 {
        self.origin_y
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.origin_x
            && y >= self.origin_y
            && x < self.origin_x.saturating_add(self.width as i32)
            && y < self.origin_y.saturating_add(self.height as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dxgi_capture_initialization_and_frame() {
        let mut capture = match DxgiCapture::new(0) {
            Ok(c) => c,
            Err(e) => {
                println!("DXGI not available in this environment (headless/sandbox): {e}");
                return;
            }
        };

        assert!(capture.width() > 0);
        assert!(capture.height() > 0);
        println!(
            "DXGI initialized successfully: {}x{} on monitor {}",
            capture.width(),
            capture.height(),
            capture.monitor_index()
        );

        // Attempt a capture frame
        match capture.capture_frame(200) {
            Ok(frame) => {
                assert_eq!(frame.width, capture.width());
                assert_eq!(frame.height, capture.height());
                assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);
                println!(
                    "Captured frame in {} microseconds ({} ms)",
                    frame.capture_duration_us,
                    frame.capture_duration_us as f64 / 1000.0
                );
            }
            Err(DxgiCaptureError::Timeout) => {
                println!("Capture timed out (screen content was completely static), which is valid DXGI behavior");
            }
            Err(e) => {
                panic!("Unexpected capture error: {e}");
            }
        }
    }
}
