use linuxfb::Framebuffer;
use memmap::MmapMut;
use os_terminal::{DrawTarget, Rgb};

pub struct Display {
    width: usize,
    height: usize,
    map: MmapMut,
}

impl Display {
    pub fn new() -> Self {
        let fb = Framebuffer::new("/dev/fb0").expect("Failed to open fbdev");
        let (width, height) = fb.get_size();
        let map = fb.map().expect("Failed to map fb");
        Self {
            width: width as usize,
            height: height as usize,
            map,
        }
    }
}

impl DrawTarget for Display {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    #[inline]
    fn draw_pixel(&mut self, x: usize, y: usize, rgb: Rgb) {
        let pixel = (rgb.0 as u32) << 16 | (rgb.1 as u32) << 8 | rgb.2 as u32;
        let buffer = self.map.as_chunks_mut::<4>().0;
        buffer[y * self.width + x].copy_from_slice(&pixel.to_ne_bytes());
    }
}
