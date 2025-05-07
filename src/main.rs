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
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::io::{BufWriter, Read, Stdout, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::process;
use std::time::{Duration, SystemTime};
use termios::*;

const ESC: u16 = b'\x1b' as u16;
const RETURN: u16 = b'\r' as u16;
const KEY_Q: u8 = b'q';
const KEY_H: u8 = b'h';
const KEY_L: u8 = b'l';
const KEY_S: u8 = b's';
const CTRL_Q: u16 = ctrl_key(KEY_Q);
const CTRL_H: u16 = ctrl_key(KEY_H);
const CTRL_L: u16 = ctrl_key(KEY_L);
const CTRL_S: u16 = ctrl_key(KEY_S);
const BACKSPACE: u16 = 127;
const ARROW_UP: u16 = 1000;
const ARROW_LEFT: u16 = 1001;
const ARROW_DOWN: u16 = 1002;
const ARROW_RIGHT: u16 = 1003;
const PAGE_UP: u16 = 1004;
const PAGE_DOWN: u16 = 1005;
const HOME_KEY: u16 = 1006;
const END_KEY: u16 = 1007;
const DEL_KEY: u16 = 1008;
const RONTO_VERSION: &str = "0.0.1";
const NO_FILENAME: &str = "[No Name]";
const TAB_STOP: usize = 8;
const RONTO_QUIT_TIMES: u8 = 3;

#[derive(Debug)]
struct EditorConfig {
    cursor_x: usize,      // x coordinate of the cursor in the file
    cursor_y: usize,      // y coordinate of the cursor in the file
    render_x: usize,      // x coordinate of the render
    row_offset: usize,    // keeps track of what row you are on
    column_offset: usize, // keeps track of what column you are on
    screen_rows: usize,   // how many rows the terminal can display
    screen_cols: usize,   // how many columns the terminal can display
    rows: Vec<ERow>,      // lines of text in the file
    dirty: bool,          // if the current file has been modified or not
    quit_times: u8,       // how many times you must press ctrl-q without saving first to quit
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

    let stdin_fd = io::stdin().as_raw_fd();
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
        dirty: false,
        quit_times: RONTO_QUIT_TIMES,
        filename: String::new(),
        status_message: String::new(),
        status_message_time: SystemTime::now(),
        orig_termios,
    };

    if num_of_args == 2 {
        config.filename = env::args().last().unwrap();
        if let Err(e) = editor_open(&mut config) {
            shutdown_with_error(&config, e)
        };
    }

    enable_raw_mode(stdin_fd);
    set_window_size(&mut config);

    editor_set_status_message(&mut config, "HELP: Ctrl-S = save | Ctrl-Q = quit");

    // main loop
    loop {
        editor_refresh_screen(&mut config);
        editor_process_keypress(&mut config);
    }
}

const fn ctrl_key(key: u8) -> u16 {
    // mask to strip away the CTRL key bits
    (key & 0x1f) as u16
}

fn is_ctrl(key: &u16) -> bool {
    (0..=31).contains(key) || *key == 127
}

//////////////////// FILE I/O /////////////////////

fn editor_open(config: &mut EditorConfig) -> io::Result<()> {
    let file_handle = File::open(&config.filename)?;
    let reader = BufReader::new(file_handle);

    for line in reader.lines() {
        let line = line?;
        let num_of_rows = config.rows.len();
        editor_insert_row(config, line, num_of_rows);
    }

    Ok(())
}

fn editor_save(config: &mut EditorConfig) {
    if config.filename.is_empty() {
        config.filename = editor_prompt(config, "Save as: {} (ESC to cancel)");
        if config.filename.is_empty() {
            editor_set_status_message(config, "Save aborted");
            return;
        }
    }

    let buf = editor_rows_to_string(config);
    let open_then_save_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&config.filename)
        .and_then(|mut file| file.write_all(buf.as_bytes()));

    if let Err(e) = open_then_save_file {
        editor_set_status_message(config, &format!("Can't save! I/O error: {}", e));
    };

    editor_set_status_message(config, &format!("{} bytes written to disk", buf.len()));
    config.dirty = false;
}

fn editor_rows_to_string(config: &mut EditorConfig) -> String {
    let mut total_len: usize = 0;
    for erow in &config.rows {
        total_len += erow.line.len()
    }
    let mut buf = String::with_capacity(total_len);

    for erow in &config.rows {
        buf.push_str(&erow.line);
        buf.push('\n');
    }

    buf
}

//////////////////// ROW OPERATIONS ////////////////////

fn editor_update_row(erow: &mut ERow) {
    let mut len: usize = 0;
    for c in erow.line.chars() {
        if c == '\t' {
            len += TAB_STOP;
        } else {
            len += 1;
        }
    }
    let mut render = String::with_capacity(len);

    let mut index: usize = 0;
    for c in erow.line.chars() {
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

    erow.render = render;
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

fn editor_insert_row(config: &mut EditorConfig, s: String, at: usize) {
    if at > config.rows.len() {
        return;
    }

    // CONSIDERATION: change into an associated function of ERow
    let mut erow = ERow {
        line: s,
        render: String::new(),
    };
    editor_update_row(&mut erow);
    config.rows.insert(at, erow);
}

fn editor_del_row(config: &mut EditorConfig, at: usize) {
    if at >= config.rows.len() {
        return;
    }
    config.rows.remove(at);
}

fn editor_row_append_string(erow: &mut ERow, string: &str) {
    erow.line.push_str(string);
    editor_update_row(erow);
}

fn editor_row_insert_char(erow: &mut ERow, mut at: usize, c: u8) {
    if at > erow.line.len() {
        at = erow.line.len()
    }
    //
    // need to update the render string here in order to display it.
    // this could be better.
    //
    erow.line.insert(at, c as char);
    editor_update_row(erow);
}

fn editor_row_del_char(erow: &mut ERow, at: usize) {
    if at >= erow.line.len() {
        return;
    }
    erow.line.remove(at);
    editor_update_row(erow);
}

//////////////////// EDITOR OPERATIONS ////////////////////

fn editor_insert_char(config: &mut EditorConfig, c: u8) {
    if config.cursor_y == config.rows.len() {
        editor_insert_row(config, String::new(), 0);
    }

    let erow = &mut config.rows[config.cursor_y];
    editor_row_insert_char(erow, config.cursor_x, c);
    config.cursor_x += 1;
    config.dirty = true;
}

fn editor_del_char(config: &mut EditorConfig) {
    let cx = config.cursor_x;
    let cy = config.cursor_y;

    if cy == config.rows.len() {
        return;
    }

    if cx == 0 && cy == 0 {
        return;
    }

    if cx > 0 {
        let erow = &mut config.rows[cy];
        editor_row_del_char(erow, cx - 1);
        config.cursor_x -= 1;
    } else {
        config.cursor_x = config.rows[cy - 1].line.len();
        // CONSIDERATION: don't clone
        let string = config.rows[cy].line.clone();
        let erow = &mut config.rows[cy - 1];
        editor_row_append_string(erow, string.as_str());
        editor_del_row(config, cy);
        config.cursor_y -= 1;
    }

    config.dirty = true;
}

fn editor_insert_new_line(config: &mut EditorConfig) {
    let (cx, cy) = (config.cursor_x, config.cursor_y);

    if cx == 0 {
        editor_insert_row(config, String::new(), cy);
    } else {
        let string_after_x = config.rows[cy].line.split_off(cx);
        editor_insert_row(config, string_after_x, cy + 1);
        editor_update_row(&mut config.rows[cy]);
    }

    config.cursor_y += 1;
    config.cursor_x = 0;
    config.dirty = true;
}

//////////////////// TERMINAL /////////////////////

#[allow(unused_must_use)]
fn shutdown(config: &EditorConfig) {
    let mut stdout = io::stdout();

    // ansi screen clear code
    stdout.write_all(b"\x1b[2J");
    stdout.flush();
    // ansi cursor home code
    stdout.write_all(b"\x1b[H");
    stdout.flush();

    let stdin_fd = io::stdin().as_raw_fd();
    disable_raw_mode(stdin_fd, &config.orig_termios);

    process::exit(0);
}

#[allow(unused_must_use)]
fn shutdown_with_error<T: Error>(config: &EditorConfig, e: T) {
    let mut stdout = io::stdout();

    // ansi screen clear code
    stdout.write_all(b"\x1b[2J");
    stdout.flush();
    // ansi cursor home code
    stdout.write_all(b"\x1b[H");
    stdout.flush();

    let stdin_fd = io::stdin().as_raw_fd();
    disable_raw_mode(stdin_fd, &config.orig_termios);

    eprintln!("{e:?}");
    process::exit(1);
}

fn enable_raw_mode(stdin_fd: i32) {
    let mut termios = Termios::from_fd(stdin_fd).unwrap();

    // specs can be found here
    // https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/termios.h.html
    termios.c_iflag &= !(BRKINT | INPCK | ISTRIP | IXON | ICRNL);
    termios.c_oflag &= !(OPOST);
    termios.c_cflag |= CS8;
    termios.c_lflag &= !(ICANON | ECHO | ISIG | IEXTEN);
    tcsetattr(stdin_fd, TCSAFLUSH, &termios).unwrap();
}

fn disable_raw_mode(stdin_fd: i32, orig_termios: &Termios) {
    tcsetattr(stdin_fd, TCSAFLUSH, orig_termios).unwrap();
}

fn editor_read_key() -> u16 {
    let mut stdin = io::stdin();
    let mut buf = [b'\0'; 1];
    stdin.read_exact(&mut buf).unwrap();

    if buf[0] == ESC as u8 {
        let mut seq = [b' '; 3];
        let _ = stdin.read(&mut seq).unwrap();
        let seq = seq.trim_ascii_end();

        if !seq[0].is_ascii() || !seq[1].is_ascii() {
            return ESC;
        }

        if seq[0] == b'[' {
            if seq[1] >= b'0' && seq[1] <= b'9' {
                if !seq[2].is_ascii() {
                    return ESC;
                }
                if seq[2] == b'~' {
                    match seq[1] {
                        b'1' => return HOME_KEY,
                        b'3' => return DEL_KEY,
                        b'4' => return END_KEY,
                        b'5' => return PAGE_UP,
                        b'6' => return PAGE_DOWN,
                        b'7' => return HOME_KEY,
                        b'8' => return END_KEY,
                        _ => return 0u16,
                    }
                }
            } else {
                match seq[1] {
                    b'A' => return ARROW_UP,
                    b'B' => return ARROW_DOWN,
                    b'C' => return ARROW_RIGHT,
                    b'D' => return ARROW_LEFT,
                    b'H' => return HOME_KEY,
                    b'F' => return END_KEY,
                    _ => return 0u16,
                }
            }
        } else if seq[0] == b'O' {
            match seq[1] {
                b'H' => return HOME_KEY,
                b'F' => return END_KEY,
                _ => return 0u16,
            }
        }

        ESC
    } else {
        buf[0] as u16
    }
}

fn set_window_size(config: &mut EditorConfig) {
    let ws = winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    if unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == -1 } || ws.ws_col == 0 {
        let (rows, cols) = get_window_size_from_cursor();
        config.screen_rows = rows as usize;
        config.screen_cols = cols as usize;
    } else {
        config.screen_rows = ws.ws_row as usize;
        config.screen_cols = ws.ws_col as usize;
    }

    config.screen_rows -= 2;
}

fn get_window_size_from_cursor() -> (u16, u16) {
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    // send cursor to bottom right
    stdout.write_all(b"\x1b[999C\x1b[999B").unwrap();
    stdout.flush().unwrap();

    let mut buffer = [0u8; 32];

    // request cursor cordinates
    stdout.write_all(b"\x1b[6n").unwrap();
    stdout.flush().unwrap();

    let _ = stdin.read(&mut buffer).unwrap();
    let mut iter = buffer[2..].split(|num| !num.is_ascii_digit());
    let row_bytes = iter.next().unwrap();
    let col_bytes = iter.next().unwrap();

    let rows: u16 = unsafe { str::from_utf8_unchecked(row_bytes) }
        .parse()
        .unwrap();
    let cols: u16 = unsafe { str::from_utf8_unchecked(col_bytes) }
        .parse()
        .unwrap();

    (rows, cols)
}

//////////////////// INPUT /////////////////////

fn editor_prompt(config: &mut EditorConfig, prompt: &str) -> String {
    let mut buf = String::with_capacity(128);

    loop {
        // FIXME: can't pass string args like i want to
        let message = format!(prompt, buf);
        editor_set_status_message(config, &message);
        editor_refresh_screen(config);

        let key = editor_read_key();
        match key {
            DEL_KEY | CTRL_H | BACKSPACE => {
                buf.pop();
            },
            ESC => {
                editor_set_status_message(config, "");
                return String::new();
            },
            RETURN => {
                if !buf.is_empty() {
                    editor_set_status_message(config, "");
                    return buf;
                }
            },
            _ => {
                if !is_ctrl(&key) && key < 128 {
                    buf.push(key as u8 as char);
                }
            }
        }
    }
}

fn editor_process_keypress(config: &mut EditorConfig) {
    let key: u16 = editor_read_key();
    match key {
        RETURN => {
            editor_insert_new_line(config);
        }

        CTRL_Q => {
            if config.dirty && config.quit_times > 0 {
                editor_set_status_message(
                    config,
                    &format!(
                        "WARNING!!! File has unsaved changes. Press Ctrl-Q {} more times to quit.",
                        config.quit_times
                    ),
                );
                config.quit_times -= 1;
                return;
            }
            shutdown(config);
        }

        CTRL_S => {
            editor_save(config);
        }

        HOME_KEY => {
            config.cursor_x = 0;
        }

        END_KEY => {
            if config.cursor_y < config.rows.len() {
                config.cursor_x = config.rows[config.cursor_y].line.len();
            }
        }

        BACKSPACE | CTRL_H | DEL_KEY => {
            if key == DEL_KEY {
                editor_move_cursor(ARROW_RIGHT, config);
            }
            editor_del_char(config);
        }

        ARROW_UP | ARROW_DOWN | ARROW_LEFT | ARROW_RIGHT => {
            editor_move_cursor(key, config);
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
        }

        CTRL_L | ESC => todo!(),

        _ => {
            editor_insert_char(config, key as u8);
        }
    }

    config.quit_times = RONTO_QUIT_TIMES;
}

fn editor_move_cursor(key: u16, config: &mut EditorConfig) {
    let (cx, cy) = (config.cursor_x, config.cursor_y);
    let len = config.rows.len();
    let row = if cy >= len {
        None
    } else {
        Some(&config.rows[cy])
    };

    match key {
        ARROW_UP => {
            if cy != 0 {
                config.cursor_y -= 1
            }
        }
        ARROW_LEFT => {
            if cx != 0 {
                config.cursor_x -= 1
            } else if cy > 0 {
                config.cursor_y -= 1;
                config.cursor_x = config.rows[config.cursor_y].line.len();
            }
        }
        ARROW_DOWN => {
            if (cy) < len {
                config.cursor_y += 1
            }
        }
        ARROW_RIGHT => {
            if row.is_some() && cx < row.unwrap().line.len() {
                config.cursor_x += 1
            } else if row.is_some() && cx == row.unwrap().line.len() {
                config.cursor_y += 1;
                config.cursor_x = 0;
            }
        }
        _ => (),
    }

    let row = if cy >= len {
        None
    } else {
        Some(&config.rows[cy])
    };

    let row_len = match row {
        Some(row) => row.line.len(),
        None => 0
    };
    if cx > row_len {
        config.cursor_x = row_len
    }
}

//////////////////// OUTPUT /////////////////////

fn editor_set_status_message(config: &mut EditorConfig, message: &str) {
    config.status_message = message.to_string();
    config.status_message_time = SystemTime::now();
}

fn editor_refresh_screen(config: &mut EditorConfig) {
    editor_scroll(config);
    let mut buf_writer = BufWriter::new(io::stdout());

    // hide the cursor
    buf_writer.write_all(b"\x1b[?25l").unwrap();
    // ansi cursor home code
    buf_writer.write_all(b"\x1b[H").unwrap();

    editor_draw_rows(&mut buf_writer, config);
    editor_draw_status_bar(&mut buf_writer, config);
    editor_draw_message_bar(&mut buf_writer, config);

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
    buf_writer.write_all(cursor_pos.as_bytes()).unwrap();

    // show the cursor
    buf_writer.write_all(b"\x1b[?25h").unwrap();
    buf_writer.flush().unwrap();
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

fn editor_draw_rows(buf_writer: &mut BufWriter<Stdout>, config: &EditorConfig) {
    for y in 0..config.screen_rows {
        let filerow = y + config.row_offset;
        if filerow >= config.rows.len() {
            if config.rows.is_empty() && y == config.screen_rows / 3 {
                // CONSIDERATION: rewrite without making a heap allocation
                // let mut buf = [0u8, 80];
                // let welcome = write!(buf, "Ronto editor --version {}", RONTO_VERSION);

                let welcome = format!("Ronto editor -- version {RONTO_VERSION}");
                let mut padding = (config.screen_cols - welcome.len()) / 2;
                buf_writer.write_all(b"~").unwrap();
                padding -= 1;
                while padding > 0 {
                    buf_writer.write_all(b" ").unwrap();
                    padding -= 1;
                }

                buf_writer.write_all(welcome.as_bytes()).unwrap();
            } else {
                buf_writer.write_all(b"~").unwrap();
            }
        } else {
            let line = &config.rows[filerow].render;
            // returns 0 if result would be negative
            let line_len = line.len().saturating_sub(config.column_offset);

            if line_len > config.screen_cols {
                let line = &line[..config.screen_cols];
                buf_writer.write_all(line.as_bytes()).unwrap();
            } else {
                let line = if line_len == 0 {
                    ""
                } else {
                    &line[config.column_offset..]
                };
                buf_writer.write_all(line.as_bytes()).unwrap();
            }
        }

        // erases part of the line to the right of the cursor
        buf_writer.write_all(b"\x1b[K").unwrap();
        buf_writer.write_all(b"\r\n").unwrap();
    }
}

fn editor_draw_status_bar(buf_writer: &mut BufWriter<Stdout>, config: &EditorConfig) {
    // invert colors
    buf_writer.write_all(b"\x1b[7m").unwrap();
    // clear line
    buf_writer.write_all(b"\x1b[2K").unwrap();

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
    let modified = if config.dirty { " (modified)" } else { "" };

    buf_writer.write_all(status.as_bytes()).unwrap();
    buf_writer.write_all(modified.as_bytes()).unwrap();
    let end = config.screen_cols - (status.len() + modified.len() + line_pos.len());
    for _ in 0..end {
        buf_writer.write_all(b" ").unwrap();
    }
    buf_writer.write_all(line_pos.as_bytes()).unwrap();

    // newline
    buf_writer.write_all(b"\r\n").unwrap();

    // revert colors
    buf_writer.write_all(b"\x1b[m").unwrap();
}

fn editor_draw_message_bar(buf_writer: &mut BufWriter<Stdout>, config: &EditorConfig) {
    // clear line
    buf_writer.write_all(b"\x1b[2K").unwrap();

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
        buf_writer.write_all(message.as_bytes()).unwrap();
    }
}
