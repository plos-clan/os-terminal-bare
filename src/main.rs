use std::ffi::CString;
use std::os::fd::AsFd;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::{env, process};

use anyhow::Result;
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

fn main() -> Result<()> {
    match unsafe { forkpty(None, None) } {
        Ok(ForkptyResult::Child) => {
            let shell = env::var("SHELL").unwrap_or("/bin/sh".into());
            let _ = execvp::<CString>(&CString::new(shell)?, &[]);
        }
        Ok(ForkptyResult::Parent { child, master }) => {
            let _ = child;

            let (flush_sender, flush_receiver) = channel();
            let (ansi_sender, ansi_receiver) = channel();

            let display = Display::new();

            let mut terminal = Terminal::new(display);
            terminal.set_auto_flush(false);
            terminal.set_scroll_speed(5);
            terminal.set_color_cache_size(4096);
            terminal.set_pty_writer({
                let ansi_sender = ansi_sender.clone();
                Box::new(move |data| ansi_sender.send(data.to_string()).unwrap())
            });

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

            let master_clone = master.try_clone()?;
            let terminal_clone = terminal.clone();
            let flush_sender_clone = flush_sender.clone();
            std::thread::spawn(move || {
                let mut temp = [0u8; 4096];
                loop {
                    match read(master_clone.as_fd(), &mut temp) {
                        Ok(n) if n > 0 => {
                            terminal_clone.lock().unwrap().process(&temp[..n]);
                            flush_sender_clone.send(()).unwrap();
                        }
                        Ok(_) => break,
                        Err(Errno::EIO) => process::exit(0),
                        Err(e) => {
                            eprintln!("Error reading from PTY: {:?}", e);
                            process::exit(1)
                        }
                    }
                }
            });

            #[allow(unused)]
            #[derive(Default)]
            #[repr(C)]
            struct VtMode {
                pub mode: u8,
                pub waitv: u8,
                pub relsig: u16,
                pub acqsig: u16,
                pub frsig: u16,
            }
            let mut vt: VtMode = VtMode::default();
            unsafe {
                ioctl(1, 0x5601, &mut vt as *mut _);
                vt.mode = 1;
                ioctl(1, 0x5602, &mut vt as *mut _);
            }

            std::thread::spawn(move || {
                while let Ok(key) = ansi_receiver.recv() {
                    write(master.as_fd(), key.as_bytes()).unwrap();
                }
            });

            let terminal_clone = terminal.clone();
            std::thread::spawn(move || {
                loop {
                    if let Ok(_) = flush_receiver.recv() {
                        terminal_clone.lock().unwrap().flush();
                    }
                }
            });

            let mut evdev = Device::open("/dev/input/event0").expect("Failed to find keyboard");
            loop {
                for event in evdev.fetch_events().expect("Failed to read events") {
                    match event.destructure() {
                        EventSummary::Key(_event, code, press) => {
                            if let Ok(keymap) =
                                KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(code.code()))
                            {
                                // Windows scancode is 16-bit extended scancode
                                let mut scancode = keymap.win;
                                if press == 0 {
                                    scancode += 0x80;
                                }
                                if scancode >= 0xe000 {
                                    terminal.lock().unwrap().handle_keyboard(0xe0);
                                    scancode -= 0xe000;
                                }
                                terminal.lock().unwrap().handle_keyboard(scancode as u8);
                                flush_sender.send(()).unwrap();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Err(_) => eprintln!("Fork failed"),
    }

    Ok(())
}
