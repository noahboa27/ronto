use core::str;
use libc::{ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ};
use std::error::Error;
use std::io::{BufWriter, Read, Stdin, Stdout, Write};
// use std::env;
// use std::path;
use std::process;
// use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use termios::*;

const ESC: u8 = b'\x1b';
const CTRL_Q: u16 = ctrl_key(b'q');
const ARROW_UP: u16 = 1000;
const ARROW_LEFT: u16 = 1001;
const ARROW_DOWN: u16 = 1002;
const ARROW_RIGHT: u16 = 1003;
const PAGE_UP: u16 = 1004;
const PAGE_DOWN: u16 = 1005;
const HOME_KEY: u16 = 1006;
const END_KEY: u16 = 1007;
const DEL_KEY: u16 = 1008;
const RONTO_VERSION: &'static str = "0.0.1";

#[derive(Debug)]
struct EditorConfig {
    cursor_x: u16,
    cursor_y: u16,
    screen_rows: u16,
    screen_cols: u16,
    orig_termios: Termios,
}

#[derive(Debug)]
struct ERow {
    size: u32,
    line: String,
}

fn main() {
    let mut stdout = io::stdout();
    let mut stdin = io::stdin();
    let mut buf_writer = BufWriter::new(io::stdout());
    let stdin_fd = stdin.as_raw_fd();
    let orig_termios = Termios::from_fd(stdin_fd).unwrap();
    let mut editor_config = EditorConfig {
        cursor_x: 0u16,
        cursor_y: 0u16,
        screen_rows: 0u16,
        screen_cols: 0u16,
        orig_termios,
    };

    if let Err(e) = enable_raw_mode(stdin_fd) {
        die(e)
    };

    set_window_size(&mut stdin, &mut stdout, &mut editor_config);

    loop {
        if let Err(e) = editor_refresh_screen(&mut buf_writer, &editor_config) {
            die(e)
        };
        match editor_process_keypress(&mut stdin, &mut buf_writer, &mut editor_config) {
            Ok(Some(())) => break,
            Err(e) => die(e),
            _ => (),
        };
    }

    if let Err(e) = disable_raw_mode(stdin_fd, &editor_config.orig_termios) {
        die(e)
    }
    process::exit(0);
}

const fn ctrl_key(key: u8) -> u16 {
    // mask to strip away the CTRL key bits
    (key & 0x1f) as u16
}

/////////////////////////////////////// TERMINAL ////////////////////////////////////////

fn die<T: Error>(e: T) {
    let mut stdout = io::stdout();

    // ansi screen clear code
    stdout.write_all(b"\x1b[2J");
    stdout.flush();
    // ansi cursor home code
    stdout.write_all(b"\x1b[H");
    stdout.flush();

    eprintln!("{e:?}");
    process::exit(1);
}

fn enable_raw_mode(stdin_fd: i32) -> io::Result<()> {
    let mut termios = Termios::from_fd(stdin_fd).unwrap();

    // specs can be found here
    // https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/termios.h.html
    termios.c_iflag &= !(BRKINT | INPCK | ISTRIP | IXON | ICRNL);
    termios.c_oflag &= !(OPOST);
    termios.c_cflag |= CS8;
    termios.c_lflag &= !(ICANON | ECHO | ISIG | IEXTEN);
    tcsetattr(stdin_fd, TCSAFLUSH, &termios)?;

    Ok(())
}

fn disable_raw_mode(stdin_fd: i32, orig_termios: &Termios) -> io::Result<()> {
    tcsetattr(stdin_fd, TCSAFLUSH, orig_termios)?;
    Ok(())
}

fn editor_read_key(stdin: &mut Stdin) -> io::Result<u16> {
    let mut buf = [b'\0'; 1];
    stdin.read(&mut buf)?;

    if buf[0] == ESC {
        let mut seq = [b' '; 3];
        stdin.read(&mut seq)?;
        let seq = seq.trim_ascii_end();

        if !seq[0].is_ascii() || !seq[1].is_ascii() {
            return Ok(ESC as u16);
        }

        if seq[0] == b'[' {
            if seq[1] >= b'0' && seq[1] <= b'9' {
                if !seq[2].is_ascii() {
                    return Ok(ESC as u16);
                }
                if seq[2] == b'~' {
                    match seq[1] {
                        b'1' => return Ok(HOME_KEY),
                        b'3' => return Ok(DEL_KEY),
                        b'4' => return Ok(END_KEY),
                        b'5' => return Ok(PAGE_UP),
                        b'6' => return Ok(PAGE_DOWN),
                        b'7' => return Ok(HOME_KEY),
                        b'8' => return Ok(END_KEY),
                        _ => return Ok(0u16),
                    }
                }
            } else {
                match seq[1] {
                    b'A' => return Ok(ARROW_UP),
                    b'B' => return Ok(ARROW_DOWN),
                    b'C' => return Ok(ARROW_RIGHT),
                    b'D' => return Ok(ARROW_LEFT),
                    b'H' => return Ok(HOME_KEY),
                    b'F' => return Ok(END_KEY),
                    _ => return Ok(0u16),
                }
            }
        } else if seq[0] == b'O' {
            match seq[1] {
                b'H' => return Ok(HOME_KEY),
                b'F' => return Ok(END_KEY),
                _ => return Ok(0u16),
            }
        }

        Ok(ESC as u16)
    } else {
        Ok(buf[0] as u16)
    }
}

fn set_window_size(
    stdin: &mut Stdin,
    stdout: &mut Stdout,
    editor_config: &mut EditorConfig,
) -> io::Result<()> {
    let ws = winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    if unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == -1 } || ws.ws_col == 0 {
        let (rows, cols) = get_window_size_from_cursor(stdin, stdout)?;
        editor_config.screen_rows = rows;
        editor_config.screen_cols = cols;
        Ok(())
    } else {
        editor_config.screen_rows = ws.ws_row;
        editor_config.screen_cols = ws.ws_col;
        Ok(())
    }
}

fn get_window_size_from_cursor(stdin: &mut Stdin, stdout: &mut Stdout) -> io::Result<(u16, u16)> {
    // send cursor to bottom right
    stdout.write_all(b"\x1b[999C\x1b[999B")?;
    stdout.flush()?;

    let mut buffer = [0u8; 32];

    // request cursor cordinates
    stdout.write_all(b"\x1b[6n")?;
    stdout.flush()?;

    stdin.read(&mut buffer)?;
    let mut iter = buffer[2..].split(|num| !num.is_ascii_digit());
    let row_bytes = iter.next().unwrap();
    let col_bytes = iter.next().unwrap();

    let rows: u16 = unsafe { str::from_utf8_unchecked(row_bytes) }
        .parse()
        .unwrap();
    let cols: u16 = unsafe { str::from_utf8_unchecked(col_bytes) }
        .parse()
        .unwrap();

    Ok((rows, cols))
}

/////////////////////////////////////////////////////////////////////////////////////////

/////////////////////////////////////// INPUT ///////////////////////////////////////////

fn editor_process_keypress(
    stdin: &mut Stdin,
    buf_writer: &mut BufWriter<Stdout>,
    editor_config: &mut EditorConfig,
) -> io::Result<Option<()>> {
    let key: u16 = editor_read_key(stdin)?;
    match key {
        CTRL_Q => {
            editor_refresh_screen(buf_writer, editor_config)?;
            Ok(Some(()))
        }
        ARROW_UP | ARROW_DOWN | ARROW_LEFT | ARROW_RIGHT => {
            editor_move_cursor(key, editor_config);
            Ok(None)
        }
        PAGE_UP | PAGE_DOWN => {
            let mut times = editor_config.screen_rows;
            while times > 0 {
                if key == PAGE_UP {
                    editor_move_cursor(ARROW_UP, editor_config);
                } else {
                    editor_move_cursor(ARROW_DOWN, editor_config);
                }
                times -= 1;
            }
            Ok(None)
        }
        HOME_KEY => {
            editor_config.cursor_x = 0;
            Ok(None)
        }
        END_KEY => {
            editor_config.cursor_x = editor_config.screen_cols - 1;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn editor_move_cursor(key: u16, editor_config: &mut EditorConfig) {
    match key {
        ARROW_UP => {
            if editor_config.cursor_y != 0 {
                editor_config.cursor_y -= 1
            }
        }
        ARROW_LEFT => {
            if editor_config.cursor_x != 0 {
                editor_config.cursor_x -= 1
            }
        }
        ARROW_DOWN => {
            if editor_config.cursor_y != editor_config.screen_rows - 1 {
                editor_config.cursor_y += 1
            }
        }
        ARROW_RIGHT => {
            if editor_config.cursor_x != editor_config.screen_cols - 1 {
                editor_config.cursor_x += 1
            }
        }
        _ => (),
    }
}

//////////////////////////////////////// OUTPUT /////////////////////////////////////////

fn editor_refresh_screen(
    buf_writer: &mut BufWriter<Stdout>,
    editor_config: &EditorConfig,
) -> io::Result<()> {
    // hide the cursor
    buf_writer.write(b"\x1b[?25l")?;
    // ansi cursor home code
    buf_writer.write(b"\x1b[H")?;

    editor_draw_rows(buf_writer, editor_config)?;

    // CONSIDERATION: rewrite without making a heap allocation
    // let mut buf = [0u8, 32];
    // let mut cursor = io::Cursor::new(&mut buf[..]);
    // write!(
    //     &mut cursor,
    //     "\x1b[{};{}H",
    //     16u16,
    //     16u16,
    //     // editor_config.cursor_y,
    //     // editor_config.cursor_x
    // )?;
    // buf_writer.write(cursor.get_ref())?;

    let cursor_pos = format!(
        "\x1b[{};{}H",
        editor_config.cursor_y, editor_config.cursor_x
    );
    buf_writer.write(cursor_pos.as_bytes())?;

    // buf_writer.write(b"\x1b[H")?;
    // show the cursor
    buf_writer.write(b"\x1b[?25h")?;
    buf_writer.flush()?;

    Ok(())
}

fn editor_draw_rows(
    buf_writer: &mut BufWriter<Stdout>,
    editor_config: &EditorConfig,
) -> io::Result<()> {
    for i in 0..editor_config.screen_rows {
        if i == editor_config.screen_rows / 3 {
            // CONSIDERATION: rewrite without making a heap allocation
            // let mut buf = [0u8, 80];
            // let welcome = write!(buf, "Ronto editor --version {}", RONTO_VERSION);

            let welcome = format!("Ronto editor -- version {RONTO_VERSION}");
            let mut padding = (editor_config.screen_cols - welcome.len() as u16) / 2;
            buf_writer.write(b"~")?;
            padding -= 1;
            while padding > 0 {
                buf_writer.write(b" ")?;
                padding -= 1;
            }

            buf_writer.write(welcome.as_bytes())?;
        } else {
            buf_writer.write(b"~")?;
        }

        // erases part of the line to the right of the cursor
        buf_writer.write(b"\x1b[K")?;
        if i < editor_config.screen_rows - 1 {
            buf_writer.write(b"\r\n")?;
        }
    }
    Ok(())
}
