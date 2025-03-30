// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// * Redistributions of source code must retain the above copyright notice,
//   this list of conditions and the following disclaimer.
//
// * Redistributions in binary form must reproduce the above copyright notice,
//   this list of conditions and the following disclaimer in the documentation
//   and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
// WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
// (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
// LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
// ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use core::str;
use libc::{ioctl, winsize, STDOUT_FILENO, TIOCGWINSZ};
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::io::{BufWriter, Read, Stdin, Stdout, Write};
use std::os::fd::AsRawFd;
use std::process;
use std::time::{Duration, SystemTime};
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
const NO_FILENAME: &'static str = "[No Name]";
const TAB_STOP: usize = 8;

#[derive(Debug)]
struct EditorConfig {
    cursor_x: usize,      // x coordinate of the cursor in the file
    cursor_y: usize,      // y coordinate of the cursor in the file
    render_x: usize,      // x coordinate of the render
    row_offset: usize,    // keeps track of what row you are on
    column_offset: usize, // keeps track of what column you are on
    screen_rows: usize,   // how many rows the terminal can display
    screen_cols: usize,   // how many columns the terminal can display
    rows: Vec<ERow>,      // collection of rows for the file
    filename: String,
    status_message: String,
    status_message_time: SystemTime,
    orig_termios: Termios,
}

#[derive(Debug)]
struct ERow {
    line: String,
    render: String,
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
        cursor_x: 0usize,
        cursor_y: 0usize,
        render_x: 0usize,
        row_offset: 0usize,
        column_offset: 0usize,
        screen_rows: 0usize,
        screen_cols: 0usize,
        rows: Vec::new(),
        filename: String::new(),
        status_message: String::new(),
        status_message_time: SystemTime::now(),
        orig_termios,
    };

    if num_of_args == 2 {
        config.filename = env::args().last().unwrap();
        if let Err(e) = editor_open(&mut config) {
            die(e)
        };
    }

    if let Err(e) = enable_raw_mode(stdin_fd) {
        die(e)
    };

    if let Err(e) = set_window_size(&mut stdin, &mut stdout, &mut config) {
        die(e)
    };

    editor_set_status_message(&mut config, &["HELP: Ctrl-Q = quit"]);

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

    shutdown();
}

const fn ctrl_key(key: u8) -> u16 {
    // mask to strip away the CTRL key bits
    (key & 0x1f) as u16
}

/////////////////////////////////////// FILE I/O ////////////////////////////////////////

fn editor_open(config: &mut EditorConfig) -> io::Result<()> {
    let file_handle = File::open(&config.filename)?;
    let reader = BufReader::new(file_handle);

    for line in reader.lines() {
        let line = line?;
        let render = render_line(&line);

        let erow = ERow { line, render };
        config.rows.push(erow);
    }

    Ok(())
}

/////////////////////////////////////// ROW OPERATIONS //////////////////////////////////

fn render_line(line: &str) -> String {
    let mut len: usize = 0;
    for c in line.chars() {
        if c == '\t' {
            len += TAB_STOP;
        } else {
            len += 1;
        }
    }
    let mut render = String::with_capacity(len);

    let mut index: usize = 0;
    for c in line.chars() {
        if c == '\t' {
            render.push(' ');
            index += 1;
            while index % TAB_STOP != 0 {
                render.push(' ');
                index += 1;
            }
        } else {
            render.push(c);
        }
    }

    render
}

fn editor_row_cursorx_to_renderx(row: &str, cx: usize) -> usize {
    let mut rx: usize = 0;
    let chars = row.chars().take(cx);

    for c in chars {
        if c == '\t' {
            rx += (TAB_STOP - 1) - (rx % TAB_STOP);
        }
        rx += 1;
    }

    rx
}

/////////////////////////////////////// EDITOR OPERATIONS ////////////////////////////////

fn editor_insert_char(config: &mut EditorConfig, c: u8) {
    if config.cursor_y == config.rows.len() {
        // CONSIDERATION: change into an associated function of ERow
        let erow = ERow {
            line: String::new(),
            render: String::new(),
        };
        config.rows.push(erow);
    }
    //
    // need to update the render string here in order to display it
    // this could be better
    //
    let erow = &mut config.rows[config.cursor_y];
    erow.line.insert(config.cursor_x, c as char);
    erow.render = render_line(&erow.line);
    config.cursor_x += 1;
}

/////////////////////////////////////// TERMINAL ////////////////////////////////////////

fn shutdown() {
    let mut stdout = io::stdout();

    // ansi screen clear code
    stdout.write_all(b"\x1b[2J");
    stdout.flush();
    // ansi cursor home code
    stdout.write_all(b"\x1b[H");
    stdout.flush();

    process::exit(0);
}

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
        config.screen_rows = rows as usize;
        config.screen_cols = cols as usize;
    } else {
        config.screen_rows = ws.ws_row as usize;
        config.screen_cols = ws.ws_col as usize;
    }

    config.screen_rows -= 2;
    Ok(())
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
            if key == PAGE_UP {
                config.cursor_y = config.row_offset;
            } else if key == PAGE_DOWN {
                config.cursor_y = config.row_offset + config.screen_rows - 1;
            }

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
            if config.cursor_y < config.rows.len() {
                config.cursor_x = config.rows[config.cursor_y].line.len();
            }
            Ok(None)
        }
        _ => {
            editor_insert_char(config, key as u8);
            Ok(None)
        }
    }
}

fn editor_move_cursor(key: u16, config: &mut EditorConfig) {
    let row = if config.cursor_y >= config.rows.len() {
        None
    } else {
        Some(&config.rows[config.cursor_y])
    };

    match key {
        ARROW_UP => {
            if config.cursor_y != 0 {
                config.cursor_y -= 1
            }
        }
        ARROW_LEFT => {
            if config.cursor_x != 0 {
                config.cursor_x -= 1
            } else if config.cursor_y > 0 {
                config.cursor_y -= 1;
                config.cursor_x = config.rows[config.cursor_y].line.len();
            }
        }
        ARROW_DOWN => {
            if (config.cursor_y) < config.rows.len() {
                config.cursor_y += 1
            }
        }
        ARROW_RIGHT => {
            if row.is_some() && config.cursor_x < row.unwrap().line.len() {
                config.cursor_x += 1
            } else if row.is_some() && config.cursor_x == row.unwrap().line.len() {
                config.cursor_y += 1;
                config.cursor_x = 0;
            }
        }
        _ => (),
    }

    let row = if config.cursor_y >= config.rows.len() {
        None
    } else {
        Some(&config.rows[config.cursor_y])
    };

    let row_len = if row.is_some() {
        row.unwrap().line.len()
    } else {
        0
    };
    if config.cursor_x > row_len {
        config.cursor_x = row_len
    }
}

//////////////////////////////////////// OUTPUT //////////////////////////////////////////

fn editor_set_status_message(config: &mut EditorConfig, messages: &[&str]) {
    for message in messages {
        let status = format!("{} ", message);
        config.status_message = status;
        config.status_message_time = SystemTime::now();
    }
}

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
    editor_draw_status_bar(buf_writer, config)?;
    editor_draw_message_bar(buf_writer, config)?;

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
        (config.render_x - config.column_offset) + 1
    );
    buf_writer.write(cursor_pos.as_bytes())?;

    // show the cursor
    buf_writer.write(b"\x1b[?25h")?;
    buf_writer.flush()?;

    Ok(())
}

fn editor_scroll(config: &mut EditorConfig) {
    config.render_x = config.cursor_x;
    if config.cursor_y < config.rows.len() {
        config.render_x =
            editor_row_cursorx_to_renderx(&config.rows[config.cursor_y].line, config.cursor_x);
    }

    if config.cursor_y < config.row_offset {
        config.row_offset = config.cursor_y;
    }

    if config.cursor_y >= config.row_offset + config.screen_rows {
        config.row_offset = config.cursor_y - config.screen_rows + 1;
    }

    if config.cursor_x < config.column_offset {
        config.column_offset = config.render_x;
    }

    if config.cursor_x >= config.column_offset + config.screen_cols {
        config.column_offset = config.render_x - config.screen_cols + 1;
    }
}

fn editor_draw_rows(buf_writer: &mut BufWriter<Stdout>, config: &EditorConfig) -> io::Result<()> {
    for y in 0..config.screen_rows {
        let filerow = y + config.row_offset;
        if filerow >= config.rows.len() {
            if config.rows.len() == 0 && y == config.screen_rows / 3 {
                // CONSIDERATION: rewrite without making a heap allocation
                // let mut buf = [0u8, 80];
                // let welcome = write!(buf, "Ronto editor --version {}", RONTO_VERSION);

                let welcome = format!("Ronto editor -- version {RONTO_VERSION}");
                let mut padding = (config.screen_cols - welcome.len()) / 2;
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
            let line = &config.rows[filerow].render;
            // returns 0 if result would be negative
            let line_len = line.len().saturating_sub(config.column_offset);

            if line_len > config.screen_cols {
                let line = &line[..config.screen_cols];
                buf_writer.write(line.as_bytes())?;
            } else {
                let line = if line_len == 0 {
                    ""
                } else {
                    &line[config.column_offset..]
                };
                buf_writer.write(line.as_bytes())?;
            }
        }

        // erases part of the line to the right of the cursor
        buf_writer.write(b"\x1b[K")?;
        buf_writer.write(b"\r\n")?;
    }

    Ok(())
}

fn editor_draw_status_bar(
    buf_writer: &mut BufWriter<Stdout>,
    config: &EditorConfig,
) -> io::Result<()> {
    // invert colors
    buf_writer.write(b"\x1b[7m")?;
    // clear line
    buf_writer.write(b"\x1b[2K")?;

    let filename = if !config.filename.is_empty() {
        if config.filename.len() > 20 {
            &config.filename[0..20]
        } else {
            &config.filename
        }
    } else {
        NO_FILENAME
    };

    let num_of_lines = config.rows.len();
    let status = format!("{filename} - {num_of_lines} lines");
    let line_pos = format!("{}/{}", config.cursor_y + 1, config.rows.len());

    buf_writer.write(status.as_bytes())?;
    let end = config.screen_cols - (status.len() + line_pos.len());
    for _ in 0..end {
        buf_writer.write(b" ")?;
    }
    buf_writer.write(line_pos.as_bytes())?;

    // newline
    buf_writer.write(b"\r\n")?;

    // revert colors
    buf_writer.write(b"\x1b[m")?;

    Ok(())
}

fn editor_draw_message_bar(
    buf_writer: &mut BufWriter<Stdout>,
    config: &EditorConfig,
) -> io::Result<()> {
    // clear line
    buf_writer.write(b"\x1b[2K")?;

    let five_seconds = Duration::from_secs(5);

    if SystemTime::now()
        .duration_since(config.status_message_time)
        .unwrap()
        < five_seconds
    {
        let message = if config.status_message.len() > config.screen_cols {
            &config.status_message[..config.screen_cols]
        } else {
            &config.status_message
        };
        buf_writer.write(message.as_bytes())?;
    }

    Ok(())
}
