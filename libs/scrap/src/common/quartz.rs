use crate::{quartz, Frame, Pixfmt};
use std::sync::{Arc, Mutex, TryLockError};
use std::{io, mem};

pub struct Capturer {
    inner: quartz::Capturer,
    frame: Arc<Mutex<Option<quartz::Frame>>>,
    saved_raw_data: Vec<u8>, // for faster compare and copy
}

impl Capturer {
    pub fn new(display: Display) -> io::Result<Capturer> {
        let frame = Arc::new(Mutex::new(None));

        let f = frame.clone();
        let inner = quartz::Capturer::new(
            display.0,
            display.width(),
            display.height(),
            quartz::PixelFormat::Argb8888,
            Default::default(),
            move |inner| {
                if let Ok(mut f) = f.lock() {
                    *f = Some(inner);
                }
            },
        )
        .map_err(|_| io::Error::from(io::ErrorKind::Other))?;

        Ok(Capturer {
            inner,
            frame,
            saved_raw_data: Vec::new(),
        })
    }

    pub fn width(&self) -> usize {
        self.inner.width()
    }

    pub fn height(&self) -> usize {
        self.inner.height()
    }
}

impl crate::TraitCapturer for Capturer {
    fn frame<'a>(&'a mut self, _timeout_ms: std::time::Duration) -> io::Result<Frame<'a>> {
        match self.frame.try_lock() {
            Ok(mut handle) => {
                let mut frame = None;
                mem::swap(&mut frame, &mut handle);

                match frame {
                    Some(mut frame) => {
                        crate::would_block_if_equal(&mut self.saved_raw_data, frame.inner())?;
                        frame.surface_to_bgra(self.height());
                        Ok(Frame::PixelBuffer(PixelBuffer {
                            frame: Some(frame),
                            data: &[],
                            width: self.width(),
                            height: self.height(),
                            stride: 0,
                        }))
                    }

                    None => Err(io::ErrorKind::WouldBlock.into()),
                }
            }

            Err(TryLockError::WouldBlock) => Err(io::ErrorKind::WouldBlock.into()),

            Err(TryLockError::Poisoned(..)) => Err(io::ErrorKind::Other.into()),
        }
    }
}

pub struct PixelBuffer<'a> {
    frame: Option<quartz::Frame>,
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
}

impl<'a> PixelBuffer<'a> {
    pub fn from_bytes(data: &'a [u8], width: usize, height: usize, stride: usize) -> Self {
        Self {
            frame: None,
            data,
            width,
            height,
            stride,
        }
    }
}

impl<'a> crate::TraitPixelBuffer for PixelBuffer<'a> {
    fn data(&self) -> &[u8] {
        match &self.frame {
            Some(frame) => &*frame,
            None => self.data,
        }
    }

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn stride(&self) -> Vec<usize> {
        let mut v = Vec::new();
        v.push(match &self.frame {
            Some(frame) => frame.stride(),
            None => self.stride,
        });
        v
    }

    fn pixfmt(&self) -> Pixfmt {
        Pixfmt::BGRA
    }
}

pub struct Display(quartz::Display);

impl Display {
    pub fn primary() -> io::Result<Display> {
        Ok(Display(quartz::Display::primary()))
    }

    pub fn all() -> io::Result<Vec<Display>> {
        Ok(quartz::Display::online()
            .map_err(|_| io::Error::from(io::ErrorKind::Other))?
            .into_iter()
            .map(Display)
            .collect())
    }

    pub fn width(&self) -> usize {
        self.0.width()
    }

    pub fn height(&self) -> usize {
        self.0.height()
    }

    pub fn scale(&self) -> f64 {
        self.0.scale()
    }

    pub fn name(&self) -> String {
        self.0.id().to_string()
    }

    pub fn is_online(&self) -> bool {
        self.0.is_online()
    }

    pub fn origin(&self) -> (i32, i32) {
        let o = self.0.bounds().origin;
        (o.x as _, o.y as _)
    }

    pub fn is_primary(&self) -> bool {
        self.0.is_primary()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TraitPixelBuffer;

    #[test]
    fn test_pixel_buffer_from_bytes() {
        let raw = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let pb = PixelBuffer::from_bytes(&raw, 2, 2, 8);
        assert_eq!(pb.data(), &raw[..]);
        assert_eq!(pb.width(), 2);
        assert_eq!(pb.height(), 2);
        assert_eq!(pb.stride(), vec![8]);
        assert_eq!(pb.pixfmt(), Pixfmt::BGRA);
    }

    #[test]
    fn test_pixel_buffer_from_bytes_padded_stride() {
        let raw = vec![0xAAu8; 32];
        let pb = PixelBuffer::from_bytes(&raw, 3, 2, 16);
        assert_eq!(pb.data().len(), 32);
        assert_eq!(pb.width(), 3);
        assert_eq!(pb.height(), 2);
        assert_eq!(pb.stride(), vec![16]);
        assert_eq!(pb.pixfmt(), Pixfmt::BGRA);
    }
}
