use std::{thread, time::Duration};

use x11rb::{
    COPY_DEPTH_FROM_PARENT, connection::Connection as _, protocol::xproto::*,
    rust_connection::RustConnection, wrapper::ConnectionExt as _,
};

use super::wayland_helpers::{deadline_from_hold, deadline_reached};

pub fn run(title: &str, hold_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id()?;
    let aux = CreateWindowAux::new()
        .background_pixel(screen.white_pixel)
        .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        120,
        120,
        640,
        360,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &aux,
    )?;
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )?;
    conn.map_window(win)?;
    conn.flush()?;

    if hold_ms == 0 {
        loop {
            if let Some(event) = conn.poll_for_event()? {
                if let x11rb::protocol::Event::DestroyNotify(_) = event {
                    break;
                }
            } else {
                thread::sleep(Duration::from_millis(100));
            }
        }
    } else {
        let deadline = deadline_from_hold(hold_ms);
        while !deadline_reached(deadline) {
            thread::sleep(Duration::from_millis(50));
        }
    }

    let _ = conn.destroy_window(win);
    let _ = conn.flush();
    Ok(())
}
