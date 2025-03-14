use core::str;
use libc::{ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ};
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::io::{BufWriter, Read, Stdin, Stdout, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process;
use termios::*;

const ESC: u8 = b'\x1b';
const KEY_Q: u8 = b'q';
const CTRL_Q: u16 = ctrl_key(KEY_Q);
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
    cursor_x: u16,      // x coordinate of the cursor
    cursor_y: u16,      // y coordinate of the cursor
    row_offset: u16,    // row offset from the top 0
    column_offset: u16, // column offset from the left 0
    screen_rows: u16,   // how many rows the terminal can display
    screen_cols: u16,   // how many columns the terminal can display
    rows: Vec<String>,  // collection of rows for the file
    orig_termios: Termios,
}

fn main() {
    let num_of_args = env::args().len();
    if num_of_args < 1 || num_of_args > 2 {
        println!("Usage: ronto <path/to/file>");
        process::exit(1);
    }

    let mut stdout = io::stdout();
    let mut stdin = io::stdin();
    let mut buf_writer = BufWriter::new(io::stdout());
    let stdin_fd = stdin.as_raw_fd();
    let orig_termios = Termios::from_fd(stdin_fd).unwrap();
    let mut config = EditorConfig {
        cursor_x: 0u16,
        cursor_y: 0u16,
        row_offset: 0u16,
        column_offset: 0u16,
        screen_rows: 0u16,
        screen_cols: 0u16,
        rows: Vec::new(),
        orig_termios,
    };

    if num_of_args == 2 {
        let filename = env::args().last().unwrap();
        if let Err(e) = editor_open(&mut config, &filename) {
            die(e)
        };
    }

    if let Err(e) = enable_raw_mode(stdin_fd) {
        die(e)
    };

    set_window_size(&mut stdin, &mut stdout, &mut config);

    loop {
        if let Err(e) = editor_refresh_screen(&mut buf_writer, &mut config) {
            die(e)
        };
        match editor_process_keypress(&mut stdin, &mut buf_writer, &mut config) {
            Ok(Some(())) => break,
            Err(e) => die(e),
            _ => (),
        };
    }

    if let Err(e) = disable_raw_mode(stdin_fd, &config.orig_termios) {
        die(e)
    }
    process::exit(0);
}

const fn ctrl_key(key: u8) -> u16 {
    // mask to strip away the CTRL key bits
    (key & 0x1f) as u16
}

/////////////////////////////////////// FILE I/O ////////////////////////////////////////

fn editor_open(config: &mut EditorConfig, filename: &str) -> io::Result<()> {
    let file_handle = File::open(filename)?;
    let reader = BufReader::new(file_handle);

    for line in reader.lines() {
        let line = line?;
        config.rows.push(line);
    }

    Ok(())
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
    config: &mut EditorConfig,
) -> io::Result<()> {
    let ws = winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    if unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == -1 } || ws.ws_col == 0 {
        let (rows, cols) = get_window_size_from_cursor(stdin, stdout)?;
        config.screen_rows = rows;
        config.screen_cols = cols;
        Ok(())
    } else {
        config.screen_rows = ws.ws_row;
        config.screen_cols = ws.ws_col;
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

/////////////////////////////////////// INPUT ///////////////////////////////////////////

fn editor_process_keypress(
    stdin: &mut Stdin,
    buf_writer: &mut BufWriter<Stdout>,
    config: &mut EditorConfig,
) -> io::Result<Option<()>> {
    let key: u16 = editor_read_key(stdin)?;
    match key {
        CTRL_Q => {
            editor_refresh_screen(buf_writer, config)?;
            Ok(Some(()))
        }
        ARROW_UP | ARROW_DOWN | ARROW_LEFT | ARROW_RIGHT => {
            editor_move_cursor(key, config);
            Ok(None)
        }
        PAGE_UP | PAGE_DOWN => {
            let mut times = config.screen_rows;
            while times > 0 {
                if key == PAGE_UP {
                    editor_move_cursor(ARROW_UP, config);
                } else {
                    editor_move_cursor(ARROW_DOWN, config);
                }
                times -= 1;
            }
            Ok(None)
        }
        HOME_KEY => {
            config.cursor_x = 0;
            Ok(None)
        }
        END_KEY => {
            config.cursor_x = config.screen_cols - 1;
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn editor_move_cursor(key: u16, config: &mut EditorConfig) {
    match key {
        ARROW_UP => {
            if config.cursor_y != 0 {
                config.cursor_y -= 1
            }
        }
        ARROW_LEFT => {
            if config.cursor_x != 0 {
                config.cursor_x -= 1
            }
        }
        ARROW_DOWN => {
            if (config.cursor_y as usize) < config.rows.len() {
                config.cursor_y += 1
            }
        }
        ARROW_RIGHT => {
            if config.cursor_x != config.screen_cols - 1 {
                config.cursor_x += 1
            }
        }
        _ => (),
    }
}

//////////////////////////////////////// OUTPUT /////////////////////////////////////////

fn editor_refresh_screen(
    buf_writer: &mut BufWriter<Stdout>,
    config: &mut EditorConfig,
) -> io::Result<()> {
    editor_scroll(config);

    // hide the cursor
    buf_writer.write(b"\x1b[?25l")?;
    // ansi cursor home code
    buf_writer.write(b"\x1b[H")?;

    editor_draw_rows(buf_writer, config)?;

    // CONSIDERATION: rewrite without making a heap allocation
    // let mut buf = [0u8, 32];
    // let mut cursor = io::Cursor::new(&mut buf[..]);
    // write!(
    //     &mut cursor,
    //     "\x1b[{};{}H",
    //     16u16,
    //     16u16,
    //     // config.cursor_y,
    //     // config.cursor_x
    // )?;
    // buf_writer.write(cursor.get_ref())?;

    let cursor_pos = format!(
        "\x1b[{};{}H",
        (config.cursor_y - config.row_offset) + 1,
        config.cursor_x + 1
    );
    buf_writer.write(cursor_pos.as_bytes())?;

    // show the cursor
    buf_writer.write(b"\x1b[?25h")?;
    buf_writer.flush()?;

    Ok(())
}

fn editor_scroll(config: &mut EditorConfig) {
    if config.cursor_y < config.row_offset {
        config.row_offset = config.cursor_y;
    }
    if config.cursor_y >= config.row_offset + config.screen_rows {
        config.row_offset = config.cursor_y - config.screen_rows + 1;
    }
}

fn editor_draw_rows(buf_writer: &mut BufWriter<Stdout>, config: &EditorConfig) -> io::Result<()> {
    for y in 0..config.screen_rows {
        let filerow = y + config.row_offset;
        let filerow = filerow as usize;
        if filerow >= config.rows.len() {
            if config.rows.len() == 0 && y == config.screen_rows / 3 {
                // CONSIDERATION: rewrite without making a heap allocation
                // let mut buf = [0u8, 80];
                // let welcome = write!(buf, "Ronto editor --version {}", RONTO_VERSION);

                let welcome = format!("Ronto editor -- version {RONTO_VERSION}");
                let mut padding = (config.screen_cols - welcome.len() as u16) / 2;
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
        } else {
            let line = &config.rows[filerow];
            let mut len = line.len() - config.column_offset as usize;

            if line.len() > config.screen_cols as usize {
                let short_line = &line[..config.screen_cols as usize];
                buf_writer.write(short_line.as_bytes())?;
            } else {
                buf_writer.write(line.as_bytes())?;
            }
        }

        // erases part of the line to the right of the cursor
        buf_writer.write(b"\x1b[K")?;
        if y < config.screen_rows - 1 {
            buf_writer.write(b"\r\n")?;
        }
    }
    Ok(())
}
