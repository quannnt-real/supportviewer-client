use std::{
    io,
    sync::{Arc, Mutex},
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use nokhwa::{
    pixel_format::RgbAFormat,
    query,
    utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType},
    Camera,
};

use base::message_proto::{DisplayInfo, Resolution};

#[cfg(feature = "vram")]
use crate::AdapterDevice;

use crate::common::{bail, ResultType};
use crate::{Frame, TraitCapturer};
#[cfg(target_os = "macos")]
use crate::PixelBuffer;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::{PixelBuffer, Pixfmt};

#[cfg(target_os = "macos")]
mod macos_ffi {
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int};

    extern "C" {
        pub fn sv_camera_auth_status() -> c_int;
        pub fn sv_camera_count() -> c_int;
        pub fn sv_camera_name(idx: c_int, buf: *mut c_char, len: c_int) -> c_int;
        pub fn sv_camera_resolution(idx: c_int, w: *mut c_int, h: *mut c_int) -> c_int;
        pub fn sv_camera_open(idx: c_int, w: *mut c_int, h: *mut c_int) -> *mut c_void;
        pub fn sv_camera_frame(
            h: *mut c_void,
            dst: *mut u8,
            dst_len: c_int,
            w: *mut c_int,
            h_out: *mut c_int,
            stride: *mut c_int,
        ) -> c_int;
        pub fn sv_camera_close(h: *mut c_void);
    }
}

pub const PRIMARY_CAMERA_IDX: usize = 0;
lazy_static::lazy_static! {
    static ref SYNC_CAMERA_DISPLAYS: Arc<Mutex<Vec<DisplayInfo>>> = Arc::new(Mutex::new(Vec::new()));
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
const CAMERA_NOT_SUPPORTED: &str = "This platform doesn't support camera yet";

pub struct Cameras;

// pre-condition
pub fn primary_camera_exists() -> bool {
    Cameras::exists(PRIMARY_CAMERA_IDX)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl Cameras {
    pub fn all_info() -> ResultType<Vec<DisplayInfo>> {
        match query(ApiBackend::Auto) {
            Ok(cameras) => {
                let mut camera_displays = SYNC_CAMERA_DISPLAYS.lock().unwrap();
                camera_displays.clear();
                // FIXME: nokhwa returns duplicate info for one physical camera on linux for now.
                // issue: https://github.com/l1npengtul/nokhwa/issues/171
                // Use only one camera as a temporary hack.
                cfg_if::cfg_if! {
                    if #[cfg(target_os = "linux")] {
                        let Some(info) = cameras.first() else {
                            bail!("No camera found")
                        };
                        // Use index (0) camera as main camera, fallback to the first camera if index (0) is not available.
                        // But maybe we also need to check index (1) or the lowest index camera.
                        //
                        // https://askubuntu.com/questions/234362/how-to-fix-this-problem-where-sometimes-dev-video0-becomes-automatically-dev
                        // https://github.com/rustdesk/rustdesk/pull/12010#issue-3125329069
                        let mut camera_index = info.index().clone();
                        if !matches!(camera_index, CameraIndex::Index(0)) {
                            if cameras.iter().any(|cam| matches!(cam.index(), CameraIndex::Index(0))) {
                                camera_index = CameraIndex::Index(0);
                            }
                        }
                        let camera = Self::create_camera(&camera_index)?;
                        let resolution = camera.resolution();
                        let (width, height) = (resolution.width() as i32, resolution.height() as i32);
                        camera_displays.push(DisplayInfo {
                            x: 0,
                            y: 0,
                            name: info.human_name().clone(),
                            width,
                            height,
                            online: true,
                            cursor_embedded: false,
                            scale:1.0,
                            original_resolution: Some(Resolution {
                                width,
                                height,
                                ..Default::default()
                            }).into(),
                            ..Default::default()
                        });
                    } else {
                        let mut x = 0;
                        for info in &cameras {
                            let camera = Self::create_camera(info.index())?;
                            let resolution = camera.resolution();
                            let (width, height) = (resolution.width() as i32, resolution.height() as i32);
                            camera_displays.push(DisplayInfo {
                                x,
                                y: 0,
                                name: info.human_name().clone(),
                                width,
                                height,
                                online: true,
                                cursor_embedded: false,
                                scale:1.0,
                                original_resolution: Some(Resolution {
                                    width,
                                    height,
                                    ..Default::default()
                                }).into(),
                                ..Default::default()
                            });
                            x += width;
                        }
                    }
                }
                Ok(camera_displays.clone())
            }
            Err(e) => {
                bail!("Query cameras error: {}", e)
            }
        }
    }

    pub fn exists(index: usize) -> bool {
        match query(ApiBackend::Auto) {
            Ok(cameras) => index < cameras.len(),
            _ => return false,
        }
    }

    fn create_camera(index: &CameraIndex) -> ResultType<Camera> {
        let format_type = if cfg!(target_os = "linux") {
            RequestedFormatType::None
        } else {
            RequestedFormatType::AbsoluteHighestResolution
        };
        let result = Camera::new(
            index.clone(),
            RequestedFormat::new::<RgbAFormat>(format_type),
        );
        match result {
            Ok(camera) => Ok(camera),
            Err(e) => bail!("create camera{} error:  {}", index, e),
        }
    }

    pub fn get_camera_resolution(index: usize) -> ResultType<Resolution> {
        let index = CameraIndex::Index(index as u32);
        let camera = Self::create_camera(&index)?;
        let resolution = camera.resolution();
        Ok(Resolution {
            width: resolution.width() as i32,
            height: resolution.height() as i32,
            ..Default::default()
        })
    }

    pub fn get_sync_cameras() -> Vec<DisplayInfo> {
        SYNC_CAMERA_DISPLAYS.lock().unwrap().clone()
    }

    pub fn get_capturer(current: usize) -> ResultType<Box<dyn TraitCapturer>> {
        Ok(Box::new(CameraCapturer::new(current)?))
    }
}

#[cfg(target_os = "macos")]
impl Cameras {
    pub fn all_info() -> ResultType<Vec<DisplayInfo>> {
        let mut camera_displays = SYNC_CAMERA_DISPLAYS.lock().unwrap();
        camera_displays.clear();
        if unsafe { macos_ffi::sv_camera_auth_status() } != 1 {
            bail!("Camera permission not granted");
        }
        let count = unsafe { macos_ffi::sv_camera_count() };
        if count <= 0 {
            bail!("No camera found");
        }
        let mut x = 0;
        for idx in 0..count {
            let mut w: std::os::raw::c_int = 0;
            let mut h: std::os::raw::c_int = 0;
            if unsafe { macos_ffi::sv_camera_resolution(idx, &mut w, &mut h) } != 0
                || w <= 0
                || h <= 0
            {
                bail!("create camera{} error: failed to query resolution", idx);
            }
            let mut name_buf = [0i8; 256];
            let name_len = unsafe {
                macos_ffi::sv_camera_name(idx, name_buf.as_mut_ptr(), name_buf.len() as _)
            };
            let name = if name_len > 0 {
                unsafe { std::ffi::CStr::from_ptr(name_buf.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            } else {
                format!("Camera {}", idx)
            };
            let (width, height) = (w as i32, h as i32);
            camera_displays.push(DisplayInfo {
                x,
                y: 0,
                name,
                width,
                height,
                online: true,
                cursor_embedded: false,
                scale: 1.0,
                original_resolution: Some(Resolution {
                    width,
                    height,
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            });
            x += width;
        }
        Ok(camera_displays.clone())
    }

    pub fn exists(index: usize) -> bool {
        if unsafe { macos_ffi::sv_camera_auth_status() } != 1 {
            return false;
        }
        let count = unsafe { macos_ffi::sv_camera_count() };
        count > 0 && index < count as usize
    }

    pub fn get_camera_resolution(index: usize) -> ResultType<Resolution> {
        if unsafe { macos_ffi::sv_camera_auth_status() } != 1 {
            bail!("Camera permission not granted");
        }
        let mut w: std::os::raw::c_int = 0;
        let mut h: std::os::raw::c_int = 0;
        if unsafe { macos_ffi::sv_camera_resolution(index as _, &mut w, &mut h) } != 0
            || w <= 0
            || h <= 0
        {
            bail!("create camera{} error: failed to query resolution", index);
        }
        Ok(Resolution {
            width: w as i32,
            height: h as i32,
            ..Default::default()
        })
    }

    pub fn get_sync_cameras() -> Vec<DisplayInfo> {
        SYNC_CAMERA_DISPLAYS.lock().unwrap().clone()
    }

    pub fn get_capturer(current: usize) -> ResultType<Box<dyn TraitCapturer>> {
        Ok(Box::new(CameraCapturer::new(current)?))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
impl Cameras {
    pub fn all_info() -> ResultType<Vec<DisplayInfo>> {
        return Ok(Vec::new());
    }

    pub fn exists(_index: usize) -> bool {
        false
    }

    pub fn get_camera_resolution(_index: usize) -> ResultType<Resolution> {
        bail!(CAMERA_NOT_SUPPORTED);
    }

    pub fn get_sync_cameras() -> Vec<DisplayInfo> {
        vec![]
    }

    pub fn get_capturer(_current: usize) -> ResultType<Box<dyn TraitCapturer>> {
        bail!(CAMERA_NOT_SUPPORTED);
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub struct CameraCapturer {
    camera: Camera,
    data: Vec<u8>,
    last_data: Vec<u8>, // for faster compare and copy
}

#[cfg(target_os = "macos")]
pub struct CameraCapturer {
    handle: *mut std::ffi::c_void,
    width: usize,
    height: usize,
    stride: usize,
    data: Vec<u8>,
    last_data: Vec<u8>,
}

#[cfg(target_os = "macos")]
impl Drop for CameraCapturer {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                macos_ffi::sv_camera_close(self.handle);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub struct CameraCapturer;

impl CameraCapturer {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn new(current: usize) -> ResultType<Self> {
        let index = CameraIndex::Index(current as u32);
        let camera = Cameras::create_camera(&index)?;
        Ok(CameraCapturer {
            camera,
            data: Vec::new(),
            last_data: Vec::new(),
        })
    }

    #[cfg(target_os = "macos")]
    fn new(current: usize) -> ResultType<Self> {
        if unsafe { macos_ffi::sv_camera_auth_status() } != 1 {
            bail!("Camera permission not granted");
        }
        let mut w: std::os::raw::c_int = 0;
        let mut h: std::os::raw::c_int = 0;
        let handle = unsafe { macos_ffi::sv_camera_open(current as _, &mut w, &mut h) };
        if handle.is_null() || w <= 0 || h <= 0 {
            bail!("create camera{} error: failed to open camera", current);
        }
        let width = w as usize;
        let height = h as usize;
        let stride = width * 4;
        Ok(CameraCapturer {
            handle,
            width,
            height,
            stride,
            data: vec![0u8; stride * height],
            last_data: Vec::new(),
        })
    }

    #[allow(dead_code)]
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    fn new(_current: usize) -> ResultType<Self> {
        bail!(CAMERA_NOT_SUPPORTED);
    }
}

impl TraitCapturer for CameraCapturer {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    fn frame<'a>(&'a mut self, _timeout: std::time::Duration) -> std::io::Result<Frame<'a>> {
        // TODO: move this check outside `frame`.
        if !self.camera.is_stream_open() {
            if let Err(e) = self.camera.open_stream() {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("Camera open stream error: {}", e),
                ));
            }
        }
        match self.camera.frame() {
            Ok(buffer) => {
                match buffer.decode_image::<RgbAFormat>() {
                    Ok(decoded) => {
                        self.data = decoded.as_raw().to_vec();
                        crate::would_block_if_equal(&mut self.last_data, &self.data)?;
                        // FIXME: macos's PixelBuffer cannot be directly created from bytes slice.
                        cfg_if::cfg_if! {
                            if #[cfg(any(target_os = "linux", target_os = "windows"))] {
                                Ok(Frame::PixelBuffer(PixelBuffer::new(
                                    &self.data,
                                    Pixfmt::RGBA,
                                    decoded.width() as usize,
                                    decoded.height() as usize,
                                )))
                            } else {
                                Err(io::Error::new(
                                    io::ErrorKind::Other,
                                    format!("Camera is not supported on this platform yet"),
                                ))
                            }
                        }
                    }
                    Err(e) => Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Camera frame decode error: {}", e),
                    )),
                }
            }
            Err(e) => Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Camera frame error: {}", e),
            )),
        }
    }

    #[cfg(target_os = "macos")]
    fn frame<'a>(&'a mut self, _timeout: std::time::Duration) -> std::io::Result<Frame<'a>> {
        let mut w: std::os::raw::c_int = 0;
        let mut h: std::os::raw::c_int = 0;
        let mut stride: std::os::raw::c_int = 0;
        let mut rc = unsafe {
            macos_ffi::sv_camera_frame(
                self.handle,
                self.data.as_mut_ptr(),
                self.data.len() as _,
                &mut w,
                &mut h,
                &mut stride,
            )
        };
        if rc == -3 && w > 0 && h > 0 && stride >= w * 4 {
            let needed = (stride as usize) * (h as usize);
            self.data.resize(needed, 0);
            rc = unsafe {
                macos_ffi::sv_camera_frame(
                    self.handle,
                    self.data.as_mut_ptr(),
                    self.data.len() as _,
                    &mut w,
                    &mut h,
                    &mut stride,
                )
            };
        }
        if rc == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if rc < 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Camera frame error: {}", rc),
            ));
        }
        let width = w as usize;
        let height = h as usize;
        let row_stride = stride as usize;
        let used = row_stride * height;
        if used > self.data.len() || width == 0 || height == 0 || row_stride < width * 4 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "Invalid camera frame dimensions",
            ));
        }
        self.width = width;
        self.height = height;
        self.stride = row_stride;
        crate::would_block_if_equal(&mut self.last_data, &self.data[..used])?;
        Ok(Frame::PixelBuffer(PixelBuffer::from_bytes(
            &self.data[..used],
            self.width,
            self.height,
            self.stride,
        )))
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    fn frame<'a>(&'a mut self, _timeout: std::time::Duration) -> std::io::Result<Frame<'a>> {
        Err(io::Error::new(
            io::ErrorKind::Other,
            CAMERA_NOT_SUPPORTED.to_string(),
        ))
    }

    #[cfg(windows)]
    fn is_gdi(&self) -> bool {
        true
    }

    #[cfg(windows)]
    fn set_gdi(&mut self) -> bool {
        true
    }

    #[cfg(feature = "vram")]
    fn device(&self) -> AdapterDevice {
        AdapterDevice::default()
    }

    #[cfg(feature = "vram")]
    fn set_output_texture(&mut self, _texture: bool) {}
}
