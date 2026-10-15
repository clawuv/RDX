use super::*;
#[derive(Default)]
pub(super) struct Pointer {
    pub position: (i32, i32),
    pub visible: bool,
    pub shape: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    pub bytes: Vec<u8>,
}
impl Pointer {
    pub fn composite(&self, frame: &mut CapturedFrame) -> Result<(), CaptureError> {
        if !self.visible || self.bytes.is_empty() {
            return Ok(());
        }
        let s = &self.shape;
        let mono = s.Type == 1;
        let height = if mono { s.Height / 2 } else { s.Height };
        let minimum = if mono {
            u64::from(s.Width).div_ceil(8)
        } else {
            u64::from(s.Width) * 4
        };
        if !matches!(s.Type, 1 | 2 | 4)
            || s.Width == 0
            || height == 0
            || (mono && !s.Height.is_multiple_of(2))
            || u64::from(s.Pitch) < minimum
            || u64::from(s.Pitch) * u64::from(s.Height) > self.bytes.len() as u64
        {
            return Err(CaptureError::Unavailable("invalid cursor buffer".into()));
        }
        let data = std::mem::take(&mut frame.data);
        let mut pixels = match data.try_into_mut() {
            Ok(data) => data,
            Err(data) => bytes::BytesMut::from(data.as_ref()),
        };
        for y in 0..height {
            let dy = i64::from(self.position.1) + i64::from(y);
            if dy < 0 || dy >= i64::from(frame.height) {
                continue;
            }
            for x in 0..s.Width {
                let dx = i64::from(self.position.0) + i64::from(x);
                if dx < 0 || dx >= i64::from(frame.width) {
                    continue;
                }
                let dest = (dy as usize * frame.width as usize + dx as usize) * 4;
                if mono {
                    let offset = y as usize * s.Pitch as usize + x as usize / 8;
                    let bit = 0x80 >> (x % 8);
                    let and = if self.bytes[offset] & bit != 0 {
                        255
                    } else {
                        0
                    };
                    let xor = if self.bytes[offset + height as usize * s.Pitch as usize] & bit != 0
                    {
                        255
                    } else {
                        0
                    };
                    for value in &mut pixels[dest..dest + 3] {
                        *value = (*value & and) ^ xor;
                    }
                } else {
                    let offset = y as usize * s.Pitch as usize + x as usize * 4;
                    let source = &self.bytes[offset..offset + 4];
                    for channel in 0..3 {
                        let old = pixels[dest + channel];
                        pixels[dest + channel] = if s.Type == 4 {
                            if source[3] == 0 {
                                source[channel]
                            } else {
                                old ^ source[channel]
                            }
                        } else {
                            (u32::from(source[channel])
                                + (u32::from(old) * (255 - u32::from(source[3])) + 127) / 255)
                                .min(255) as u8
                        };
                    }
                }
                pixels[dest + 3] = 255;
            }
        }
        frame.data = pixels.freeze();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> CapturedFrame {
        CapturedFrame::from_bgra_rows(2, 2, 8, &[100; 16], 0).unwrap()
    }
    fn pointer(kind: u32, height: u32, pitch: u32, bytes: Vec<u8>) -> Pointer {
        Pointer {
            visible: true,
            shape: DXGI_OUTDUPL_POINTER_SHAPE_INFO {
                Type: kind,
                Width: 2,
                Height: height,
                Pitch: pitch,
                ..Default::default()
            },
            bytes,
            ..Default::default()
        }
    }
    #[test]
    fn color_alpha_and_clipping() {
        let mut p = pointer(2, 1, 8, vec![0, 0, 0, 0, 0, 0, 128, 128]);
        p.position = (-1, 0);
        let mut f = frame();
        p.composite(&mut f).unwrap();
        assert_eq!(&f.data[..4], &[50, 50, 178, 255]);
        assert_eq!(&f.data[4..8], &[100; 4]);
    }
    #[test]
    fn monochrome_and_xor() {
        let p = pointer(1, 2, 1, vec![0b11000000, 0b01000000]);
        let mut f = frame();
        p.composite(&mut f).unwrap();
        assert_eq!(f.data[0], 100);
        assert_eq!(f.data[4], 155);
    }
    #[test]
    fn masked_replace_and_xor() {
        let p = pointer(4, 1, 8, vec![1, 2, 3, 0, 255, 255, 255, 255]);
        let mut f = frame();
        p.composite(&mut f).unwrap();
        assert_eq!(&f.data[..3], &[1, 2, 3]);
        assert_eq!(&f.data[4..7], &[155; 3]);
    }
    #[test]
    fn invalid_shape_preserves_frame() {
        let p = pointer(1, 3, 1, vec![0; 3]);
        let mut f = frame();
        assert!(p.composite(&mut f).is_err());
        assert_eq!(&f.data[..], &[100; 16]);
    }
    #[test]
    fn hidden_and_offscreen_pointer_preserve_pixels() {
        let mut p = pointer(2, 1, 8, vec![255; 8]);
        let mut f = frame();
        p.visible = false;
        p.composite(&mut f).unwrap();
        assert_eq!(&f.data[..], &[100; 16]);
        p.visible = true;
        p.position = (i32::MAX, i32::MIN);
        p.composite(&mut f).unwrap();
        assert_eq!(&f.data[..], &[100; 16]);
    }
}
