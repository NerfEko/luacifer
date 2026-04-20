use std::{
    fs::File,
    io::Write,
    time::{Duration, Instant},
};

pub fn draw_probe_buffer(file: &mut File, width: u32, height: u32) -> std::io::Result<()> {
    let mut writer = std::io::BufWriter::new(file);
    for y in 0..height {
        for x in 0..width {
            let a = 0xFF_u8;
            let r = ((width - x) * 0xFF / width) as u8;
            let g = ((x + y) * 0xA0 / (width + height).max(1)) as u8;
            let b = ((height - y) * 0xFF / height) as u8;
            writer.write_all(&[b, g, r, a])?;
        }
    }
    writer.flush()
}

pub fn deadline_from_hold(hold_ms: u64) -> Option<Instant> {
    if hold_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(hold_ms))
    }
}

pub fn deadline_reached(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}
