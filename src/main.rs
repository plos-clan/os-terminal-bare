use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd};
use std::sync::{Arc, Mutex};
use std::{env, process};

use anyhow::{Context, Result};
use evdev::{Device, EventSummary};
use keycode::KeyMap;
use nix::errno::Errno;
use nix::libc::{TIOCSWINSZ, ioctl};
use nix::pty::{ForkptyResult, Winsize, forkpty};
use nix::unistd::{execvp, read, write};
use os_terminal::Terminal;
use os_terminal::font::TrueTypeFont;

use crate::backends::Display;

pub mod backends;

const DISPLAY_SIZE: (usize, usize) = (1024, 768);
const VT_GETMODE: i32 = 0x5601;
const VT_SETMODE: i32 = 0x5602;

fn main() -> Result<()> {
    match unsafe { forkpty(None, None) }? {
        ForkptyResult::Child => {
            let shell = env::var("SHELL").unwrap_or("/bin/sh".into());
            let c_shell = CString::new(shell).context("Invalid SHELL path")?;
            execvp::<CString>(&c_shell, &[]).context("Failed to exec shell")?;
            unreachable!();
        }
        ForkptyResult::Parent { child: _, master } => {
            let display = Display::new();
            let mut terminal = Terminal::new(display);

            terminal.set_auto_flush(false);
            terminal.set_scroll_speed(5);
            terminal.set_color_cache_size(4096);

            let master_writer = master.try_clone()?;
            terminal.set_pty_writer(Box::new(move |data| {
                let _ = write(master_writer.as_fd(), data.as_bytes());
            }));

            let font_buffer = include_bytes!("../assets/FiraCodeNotoSans.ttf");
            let font_manager = TrueTypeFont::new(10.0, font_buffer).with_subpixel(true);
            terminal.set_font_manager(Box::new(font_manager));
            terminal.set_history_size(1000);

            let win_size = Winsize {
                ws_row: terminal.rows() as u16,
                ws_col: terminal.columns() as u16,
                ws_xpixel: DISPLAY_SIZE.0 as u16,
                ws_ypixel: DISPLAY_SIZE.1 as u16,
            };
            unsafe { ioctl(master.as_raw_fd(), TIOCSWINSZ, &win_size) };

            let terminal = Arc::new(Mutex::new(terminal));

            let master_reader = master.try_clone()?;
            let term_reader = terminal.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match read(master_reader.as_fd(), &mut buf) {
                        Ok(n) if n > 0 => {
                            let mut term = term_reader.lock().unwrap();
                            term.process(&buf[..n]);
                            term.flush();
                        }
                        Ok(_) => break,
                        Err(Errno::EIO) => process::exit(0),
                        Err(e) => {
                            eprintln!("PTY read error: {:?}", e);
                            process::exit(1);
                        }
                    }
                }
            });

            #[derive(Default, Debug)]
            #[repr(C)]
            struct VtMode {
                mode: u8,
                waitv: u8,
                relsig: u16,
                acqsig: u16,
                frsig: u16,
            }
            let mut vt = VtMode::default();
            unsafe {
                ioctl(1, VT_GETMODE, &mut vt as *mut _);
                vt.mode = 1;
                ioctl(1, VT_SETMODE, &mut vt as *mut _);
            }

            let mut evdev = Device::open("/dev/input/event0")?;

            loop {
                for event in evdev.fetch_events()? {
                    if let EventSummary::Key(_, code, press) = event.destructure() {
                        if let Ok(keymap) =
                            KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.code()))
                        {
                            let mut term = terminal.lock().unwrap();

                            let mut scancode = keymap.win;
                            if press == 0 {
                                scancode += 0x80;
                            }

                            if scancode >= 0xe000 {
                                term.handle_keyboard(0xe0);
                                scancode -= 0xe000;
                            }

                            term.handle_keyboard(scancode as u8);
                            term.flush();
                        }
                    }
                }
            }
        }
    }
}
