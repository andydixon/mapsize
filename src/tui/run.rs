//! The terminal runtime: raw mode and alternate screen, an input thread,
//! timers and background commands feeding one message loop, and a
//! line-diffing frame writer.
//!
//! The terminal is restored on every exit path: normal quit, errors and
//! panics (a panic hook restores it before the default hook prints; the
//! scanner aborts the process on worker panics after the hook has run).

use super::model::{debug_panic, Button, Cmd, Key, Model, Msg, Options, Profile};
use crate::brand;
use crate::cancel::Cancel;
use crate::textutil;
use crossbeam_channel::{after, select, Receiver, Sender};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::{cursor, execute, terminal};
use std::fmt::Write as _;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

/// Whether the terminal is currently in TUI mode (raw, alternate screen).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set by the SIGTERM handler; the input thread turns it into a quit.
static TERMINATED: AtomicBool = AtomicBool::new(false);

/// Mouse off (1000/1002/1003/1015/1006), bracketed paste off, cursor on,
/// main screen.
const RESTORE: &str =
    "\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?2004l\x1b[?25h\x1b[?1049l";

fn write_raw(s: &str) {
    let mut b = s.as_bytes();
    while !b.is_empty() {
        let n = unsafe { libc::write(1, b.as_ptr().cast(), b.len().min(1 << 30) as _) };
        if n <= 0 {
            return;
        }
        b = &b[n as usize..];
    }
}

/// Leaves TUI mode. Safe to call repeatedly and from the panic hook (it
/// writes directly to fd 1, so it cannot block on a stdout lock).
fn restore_terminal() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        write_raw(RESTORE);
        let _ = terminal::disable_raw_mode();
    }
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            prev(info);
        }));
        extern "C" fn on_term(_: libc::c_int) {
            TERMINATED.store(true, Ordering::SeqCst);
        }
        unsafe { libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t) };
    });
}

fn forced_profile(c: &str) -> Option<Profile> {
    match c {
        "truecolor" | "24bit" => Some(Profile::TrueColor),
        "256" => Some(Profile::Ansi256),
        "16" => Some(Profile::Ansi),
        "none" => Some(Profile::Ascii),
        _ => None,
    }
}

/// Detects the colour profile from the environment (NO_COLOR, COLORTERM,
/// TERM). Over SSH COLORTERM is often not forwarded; --color overrides.
fn detect_profile() -> Profile {
    let env = |k: &str| std::env::var(k).unwrap_or_default().to_lowercase();
    let (term, ct) = (env("TERM"), env("COLORTERM"));
    if !env("NO_COLOR").is_empty() || term == "dumb" {
        return Profile::Ascii;
    }
    if ct == "truecolor" || ct == "24bit" || term.ends_with("-direct") {
        return Profile::TrueColor;
    }
    if [
        "kitty",
        "alacritty",
        "wezterm",
        "ghostty",
        "foot",
        "contour",
        "rio",
    ]
    .iter()
    .any(|t| term.contains(t))
    {
        return Profile::TrueColor;
    }
    if term.contains("256color") || !ct.is_empty() {
        return Profile::Ansi256;
    }
    Profile::Ansi
}

/// Starts the interactive UI.
pub fn run(opts: Options) -> Result<(), crate::Error> {
    let cancel = Cancel::new();
    let (s, t) = (opts.start)(cancel.clone())?;
    let forced = forced_profile(&opts.color);
    if forced.is_none() && !opts.color.is_empty() && opts.color != "auto" {
        cancel.cancel();
        return Err(format!(
            "invalid --color {:?} (auto, truecolor, 256, 16, none)",
            opts.color
        )
        .into());
    }
    let mouse = opts.mouse;
    let mut m = Model::new(opts, s, t, cancel.clone());
    m.profile = forced.unwrap_or_else(detect_profile);

    install_panic_hook();
    if debug_panic() == "scanner" {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            // Mirrors the scanner's guard: a worker panic aborts after the hook.
            if std::panic::catch_unwind(|| {
                panic!("MAPSIZE_DEBUG_PANIC=scanner: deliberate panic to test terminal restoration")
            })
            .is_err()
            {
                std::process::abort();
            }
        });
    }
    let res = enter_terminal(mouse).and_then(|_| event_loop(&mut m));
    restore_terminal();
    cancel.cancel();
    m.cancel.cancel(); // a rescan may have replaced the token
    Ok(res?)
}

fn enter_terminal(mouse: bool) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = io::stdout();
    execute!(
        out,
        terminal::EnterAlternateScreen,
        cursor::Hide,
        event::EnableBracketedPaste,
        terminal::SetTitle(brand::NAME)
    )?;
    if mouse {
        execute!(out, event::EnableMouseCapture)?;
    }
    Ok(())
}

/// The keystroke name (see `Key`) for a key event.
fn convert_key(ev: KeyEvent) -> Option<Key> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let m = ev.modifiers;
    let (ctrl, alt, shift) = (
        m.contains(KeyModifiers::CONTROL),
        m.contains(KeyModifiers::ALT),
        m.contains(KeyModifiers::SHIFT),
    );
    let mut prefix = String::new();
    if ctrl {
        prefix += "ctrl+";
    }
    if alt {
        prefix += "alt+";
    }
    let named = |n: &str| {
        Some(Key {
            name: format!("{prefix}{}{n}", if shift { "shift+" } else { "" }),
            text: String::new(),
            ctrl_alt: ctrl || alt,
        })
    };
    match ev.code {
        KeyCode::Char(c) => {
            let base = if c == ' ' {
                "space".to_string()
            } else if ctrl || alt {
                c.to_lowercase().collect()
            } else {
                c.to_string()
            };
            let text = if ctrl || alt {
                String::new()
            } else {
                c.to_string()
            };
            Some(Key {
                name: prefix + &base,
                text,
                ctrl_alt: ctrl || alt,
            })
        }
        KeyCode::BackTab => Some(Key {
            name: format!("{prefix}shift+tab"),
            ..Default::default()
        }),
        KeyCode::Enter => named("enter"),
        KeyCode::Esc => named("esc"),
        KeyCode::Tab => named("tab"),
        KeyCode::Backspace => named("backspace"),
        KeyCode::Delete => named("delete"),
        KeyCode::Insert => named("insert"),
        KeyCode::Home => named("home"),
        KeyCode::End => named("end"),
        KeyCode::PageUp => named("pgup"),
        KeyCode::PageDown => named("pgdown"),
        KeyCode::Up => named("up"),
        KeyCode::Down => named("down"),
        KeyCode::Left => named("left"),
        KeyCode::Right => named("right"),
        KeyCode::F(n) => named(&format!("f{n}")),
        _ => None,
    }
}

fn convert(ev: Event) -> Option<Msg> {
    match ev {
        Event::Key(k) => convert_key(k).map(Msg::Key),
        Event::Resize(w, h) => Some(Msg::Size(w as i32, h as i32)),
        Event::Mouse(me) => {
            let (x, y) = (me.column as i32, me.row as i32);
            match me.kind {
                MouseEventKind::Down(b) => {
                    let button = match b {
                        MouseButton::Left => Button::Left,
                        MouseButton::Right => Button::Right,
                        MouseButton::Middle => Button::Middle,
                    };
                    Some(Msg::Click { x, y, button })
                }
                MouseEventKind::ScrollUp => Some(Msg::Wheel { x, y, up: true }),
                MouseEventKind::ScrollDown => Some(Msg::Wheel { x, y, up: false }),
                MouseEventKind::Moved | MouseEventKind::Drag(_) => Some(Msg::Motion { x, y }),
                _ => None,
            }
        }
        // Pastes are ignored.
        _ => None,
    }
}

/// Reads terminal events until stopped.
fn input_thread(tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        if TERMINATED.load(Ordering::SeqCst) {
            let _ = tx.send(Msg::Quit);
            return;
        }
        match event::poll(Duration::from_millis(100)) {
            Ok(true) => match event::read() {
                Ok(ev) => {
                    if let Some(msg) = convert(ev) {
                        if tx.send(msg).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => return,
            },
            Ok(false) => {}
            Err(_) => return,
        }
    }
}

/// Executes commands: background closures on threads, timers in the loop,
/// clipboard writes into the output.
struct Runtime {
    tx: Sender<Msg>,
    timers: Vec<(Instant, Msg)>,
    out: String, // pending escape sequences (clipboard)
}

impl Runtime {
    fn exec(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::None => {}
            Cmd::Batch(v) => v.into_iter().for_each(|c| self.exec(c)),
            Cmd::Run(f) => {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(f());
                });
            }
            Cmd::Tick(d, msg) => self.timers.push((Instant::now() + d, msg)),
            Cmd::Clipboard(s) => {
                let _ = write!(self.out, "\x1b]52;c;{}\x07", base64(s.as_bytes()));
            }
        }
    }
}

fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Writes frames, rewriting only what changed since the last one: within a
/// changed line, the span between the first and last differing style run.
#[derive(Default)]
struct Screen {
    prev: Vec<String>,
    size: (i32, i32),
    buf: String,
}

/// Splits a serialised line into its SGR-led runs as (start byte, start
/// column). The canvas emits an escape only to start a run (text is
/// sanitised), and the trailing reset is a zero-width run.
fn runs(l: &str) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = l.match_indices("\x1b[").map(|(i, _)| i).collect();
    let mut col = 0;
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let seg = &l[s..starts.get(k + 1).copied().unwrap_or(l.len())];
        out.push((s, col));
        col += textutil::width(&seg[seg.find('m').map_or(seg.len(), |p| p + 1)..]);
    }
    out
}

/// The changed parts of a line as (column, byte range of new): runs of new
/// that do not appear unchanged at the same column in old, grouped into
/// contiguous segments. Empty if the lines are equal.
fn line_diff(old: &str, new: &str) -> Vec<(usize, std::ops::Range<usize>)> {
    if old == new {
        return Vec::new();
    }
    let (ro, rn) = (runs(old), runs(new));
    let end = |l: &str, r: &[(usize, usize)], k: usize| r.get(k + 1).map_or(l.len(), |x| x.0);
    let mut changed = vec![true; rn.len()];
    let (mut i, mut j) = (0, 0);
    while i < ro.len() && j < rn.len() {
        let (co, cn) = (ro[i].1, rn[j].1);
        if co == cn {
            changed[j] = old[ro[i].0..end(old, &ro, i)] != new[rn[j].0..end(new, &rn, j)];
            (i, j) = (i + 1, j + 1);
        } else if co < cn {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut out: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
    for (k, &c) in changed.iter().enumerate() {
        if !c {
            continue;
        }
        let r = rn[k].0..end(new, &rn, k);
        match out.last_mut() {
            Some(last) if last.1.end == r.start => last.1.end = r.end,
            _ => out.push((rn[k].1, r)),
        }
    }
    if out.is_empty() {
        out.push((0, 0..new.len())); // a line without runs (cannot happen)
    }
    out
}

impl Screen {
    fn draw(
        &mut self,
        out: &mut impl Write,
        frame: &str,
        w: i32,
        h: i32,
        extra: &str,
    ) -> io::Result<()> {
        self.buf.clear();
        self.buf.push_str("\x1b[?2026h"); // synchronized output, where supported
        if (w, h) != self.size {
            self.buf.push_str("\x1b[0m\x1b[2J");
            self.prev.clear();
            self.size = (w, h);
        }
        let mut n = 0;
        for (y, l) in frame.split('\n').enumerate() {
            match self.prev.get_mut(y) {
                Some(p) => {
                    let d = line_diff(p, l);
                    for (col, r) in &d {
                        let _ = write!(self.buf, "\x1b[{};{}H{}", y + 1, col + 1, &l[r.clone()]);
                    }
                    if !d.is_empty() {
                        p.clear();
                        p.push_str(l);
                    }
                }
                None => {
                    let _ = write!(self.buf, "\x1b[{};1H{l}", y + 1);
                    self.prev.push(l.to_string());
                }
            }
            n = y + 1;
        }
        self.prev.truncate(n);
        self.buf.push_str(extra);
        self.buf.push_str("\x1b[?2026l");
        out.write_all(self.buf.as_bytes())?;
        out.flush()
    }
}

fn next_msg(rx: &Receiver<Msg>, timers: &[(Instant, Msg)]) -> Option<Msg> {
    match timers.iter().map(|t| t.0).min() {
        Some(d) => select! {
            recv(rx) -> msg => msg.ok(),
            recv(after(d.saturating_duration_since(Instant::now()))) -> _ => None,
        },
        None => rx.recv().ok(),
    }
}

fn event_loop(m: &mut Model) -> io::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded::<Msg>();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (tx, stop) = (tx.clone(), stop.clone());
        std::thread::spawn(move || input_thread(tx, stop));
    }
    let (w, h) = terminal::size()?;
    let mut rt = Runtime {
        tx,
        timers: Vec::new(),
        out: String::new(),
    };
    rt.exec(m.update(Msg::Size(w as i32, h as i32)));
    let init = m.init();
    rt.exec(init);
    let mut screen = Screen::default();
    let mut stdout = io::stdout();
    let mut last = String::new();
    let res = loop {
        if m.quit {
            break Ok(());
        }
        let (w, h) = (m.w, m.h);
        let frame = m.view();
        if frame != last || !rt.out.is_empty() {
            if let Err(e) = screen.draw(&mut stdout, frame, w, h, &rt.out) {
                break Err(e);
            }
            last.clear();
            last.push_str(frame);
            rt.out.clear();
        }
        if let Some(msg) = next_msg(&rx, &rt.timers) {
            let c = m.update(msg);
            rt.exec(c);
        }
        // Fire due timers, then drain whatever else is queued before the next
        // frame, so bursts (resize storms) cost one render.
        let now = Instant::now();
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut rt.timers)
            .into_iter()
            .partition(|t| t.0 <= now);
        rt.timers = later;
        for (_, msg) in due {
            let c = m.update(msg);
            rt.exec(c);
        }
        while let Ok(msg) = rx.try_recv() {
            if m.quit {
                break;
            }
            let c = m.update(msg);
            rt.exec(c);
        }
    };
    stop.store(true, Ordering::Relaxed);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn screen_rewrites_only_changed_lines() {
        let mut s = Screen::default();
        let mut out = Vec::new();
        s.draw(&mut out, "a\nb\nc", 1, 3, "").unwrap();
        out.clear();
        s.draw(&mut out, "a\nX\nc", 1, 3, "").unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\x1b[?2026h\x1b[2;1HX\x1b[?2026l"
        );
        // Within a line only the differing style runs are rewritten.
        let a = "\x1b[0;1mab\x1b[0;7m日x\x1b[0mcd\x1b[0m";
        let b = "\x1b[0;1mab\x1b[0;7m日y\x1b[0mcd\x1b[0m";
        assert_eq!(line_diff(a, b), vec![(2, 8..18)]);
        assert_eq!(&b[8..18], "\x1b[0;7m日y");
        assert!(line_diff(a, a).is_empty());
        let c = "\x1b[0;2mab\x1b[0;7m日x\x1b[0mcz\x1b[0m";
        assert_eq!(line_diff(a, c), vec![(0, 0..8), (5, 18..24)]);
    }

    #[test]
    fn keys_use_bubbletea_names() {
        let k = |code, m| convert_key(KeyEvent::new(code, m)).unwrap().name;
        assert_eq!(k(KeyCode::Char('c'), KeyModifiers::CONTROL), "ctrl+c");
        assert_eq!(k(KeyCode::Char('G'), KeyModifiers::SHIFT), "G");
        assert_eq!(k(KeyCode::Char(' '), KeyModifiers::NONE), "space");
        assert_eq!(k(KeyCode::BackTab, KeyModifiers::SHIFT), "shift+tab");
        assert_eq!(k(KeyCode::PageDown, KeyModifiers::NONE), "pgdown");
        assert_eq!(k(KeyCode::Backspace, KeyModifiers::ALT), "alt+backspace");
    }
}
