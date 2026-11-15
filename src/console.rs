//! What the console can draw - and, on the classic Windows console, getting
//! it to draw more.
//!
//! The map is at its sharpest in braille: eight dots to a character. Most
//! terminals can show that, with one large exception. A program started by
//! double-clicking it on Windows 10 gets the classic console, whose default
//! font (Consolas) has no braille at all, and which will not borrow the
//! glyphs from another font the way modern terminals do.
//!
//! So rather than guess, `prepare` asks: is this the classic console, and
//! does its font have the glyphs? If not, it looks for an installed font
//! that does and switches this one window to it for as long as the program
//! runs. Failing that, the map falls back to line art that any font can draw.
//!
//! The same console is the one place a program can change the size of its
//! own text, which matters here: the map is made of dots, eight to a
//! character, so smaller text is the only road to a finer map. That is
//! offered as a control (`resize_text`) and never done unasked.

/// Settle a charset setting ("rich", "safe", or anything else for "work it
/// out") into one of the three answers `ui::theme::Glyphs::detect` takes:
///
/// - `"rich"`: braille and any other symbol; the terminal finds a font.
/// - `"braille"`: braille, but nothing the console's one font may lack.
/// - `"safe"`: only what every console font has.
///
/// May change the console font; `restore` undoes that.
///
/// `text_size` is a text height in pixels that the user chose earlier (with
/// `resize_text`, the settings file or `--text-size`), applied on the
/// classic Windows console; 0 leaves the text exactly as it is. The size is
/// never chosen for the user and the window is never resized for them: how
/// big the text is, and how big the window, are theirs to decide.
pub fn prepare(charset: &str, text_size: u16) -> &'static str {
    let answer = match charset.to_ascii_lowercase().as_str() {
        "rich" => "rich",
        "safe" => "safe",
        _ => auto(),
    };
    #[cfg(windows)]
    if text_size > 0 && classic::is_classic_console() {
        classic::set_height(text_size.min(72) as i16);
    }
    #[cfg(not(windows))]
    let _ = text_size;
    answer
}

/// Put back the console font `prepare` or `resize_text` changed, if they
/// changed it. Safe to call more than once, and from a panic hook.
pub fn restore() {
    #[cfg(windows)]
    classic::restore();
}

/// The text on the system clipboard, for pasting where the terminal does
/// not paste by itself.
///
/// That is the classic Windows console: while a full-screen program runs
/// there, Ctrl+V reaches the program as a keystroke instead of being
/// turned into a paste, and a right-click does nothing. Elsewhere the
/// terminal pastes for us and the text simply arrives, so the answer is
/// `None`.
pub fn clipboard_text() -> Option<String> {
    #[cfg(windows)]
    {
        clipboard::text()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Text heights offered, in pixels.
const TEXT_SIZES: [i16; 12] = [6, 8, 10, 12, 14, 16, 18, 20, 24, 28, 32, 36];

/// Make the console's text one step smaller (`-1`: more characters, so a
/// finer map) or larger (`+1`). Returns the new height in pixels, or `None`
/// where the program has no say: every terminal but the classic Windows
/// console keeps the text size in the user's hands (Ctrl and minus; Cmd and
/// minus on a Mac).
pub fn resize_text(step: i32) -> Option<u16> {
    #[cfg(windows)]
    {
        if !classic::is_classic_console() {
            return None;
        }
        let now = classic::current_font()?.dwFontSize.Y;
        let wanted = step_size(now, step);
        (wanted != now && classic::set_height(wanted))
            .then_some(wanted as u16)
            .or(Some(now as u16))
    }
    #[cfg(not(windows))]
    {
        let _ = step;
        None
    }
}

/// The size `step` places along the ladder from the one nearest to `now`.
#[cfg_attr(not(windows), allow(dead_code))]
fn step_size(now: i16, step: i32) -> i16 {
    let nearest = (0..TEXT_SIZES.len())
        .min_by_key(|i| (TEXT_SIZES[*i] - now).abs())
        .unwrap_or(0);
    let last = TEXT_SIZES.len() as i32 - 1;
    TEXT_SIZES[(nearest as i32 + step).clamp(0, last) as usize]
}

/// A guess from the environment, for when there is no console to ask
/// (rendering to text, tests).
pub fn looks_modern() -> bool {
    let has = |name: &str| std::env::var_os(name).is_some();
    if cfg!(windows) {
        // Windows Terminal and third-party terminals announce themselves;
        // the classic console sets none of these.
        [
            "WT_SESSION",
            "TERM_PROGRAM",
            "WEZTERM_PANE",
            "ALACRITTY_WINDOW_ID",
            "ConEmuANSI",
            "TERM",
        ]
        .into_iter()
        .any(has)
    } else {
        // The bare Linux virtual console has a 512-glyph font: no braille.
        std::env::var("TERM").map(|t| t != "linux").unwrap_or(true)
    }
}

fn guess() -> &'static str {
    if looks_modern() { "rich" } else { "safe" }
}

#[cfg(not(windows))]
fn auto() -> &'static str {
    guess()
}

#[cfg(windows)]
fn auto() -> &'static str {
    // Windows Terminal, VS Code and the like run programs behind a
    // pseudoconsole and draw the text themselves, falling back to other
    // fonts for glyphs the chosen one lacks. Anything goes there.
    if !classic::is_classic_console() {
        return "rich";
    }
    let Some(font) = classic::current_font() else {
        return guess();
    };
    // The classic console draws with exactly one font. Either that font
    // has braille, or there is one installed that does and can be lent to
    // this window, or the map is drawn another way.
    let braille = classic::draws_braille(&classic::face_of(&font))
        || classic::FONTS
            .iter()
            .any(|face| classic::draws_braille(face) && classic::switch_to(&font, face));
    if braille { "braille" } else { "safe" }
}

/// The classic Windows console (conhost), and the font it draws with.
///
/// Everything here is a call into Win32. Each `unsafe` block passes only
/// pointers to live local values of the right type and size, and releases
/// every GDI handle it creates.
#[cfg(windows)]
mod classic {
    use std::sync::Mutex;

    use windows_sys::Win32::{
        Foundation::RECT,
        Graphics::Gdi::{
            CreateCompatibleDC, CreateFontW, DeleteDC, DeleteObject, GetGlyphIndicesW,
            GetTextFaceW, GetTextMetricsW, SelectObject, TEXTMETRICW,
        },
        System::Console::{
            CONSOLE_FONT_INFOEX, GetConsoleWindow, GetCurrentConsoleFontEx, GetStdHandle,
            STD_OUTPUT_HANDLE, SetCurrentConsoleFontEx,
        },
        UI::WindowsAndMessaging::{GetWindowRect, IsWindowVisible, SetWindowPos},
    };

    /// Set the console's font, and leave its window exactly where it was.
    ///
    /// Left to itself, the console answers a font change by keeping the
    /// same number of characters and resizing the window around them: ask
    /// for smaller text and the window shrinks, showing no more than
    /// before. What is wanted is the reverse - the same window, holding
    /// more characters. So the window's place and size are noted first and
    /// put back afterwards, and the console then works out for itself how
    /// many rows and columns of the new font fit.
    fn set_font(font: &CONSOLE_FONT_INFOEX) -> bool {
        // SAFETY: `before` and `after` are plain structs the calls fill in;
        // `font` outlives the call; the window handle is checked for null.
        unsafe {
            let window = GetConsoleWindow();
            let mut before: RECT = std::mem::zeroed();
            let placed = !window.is_null() && GetWindowRect(window, &mut before) != 0;
            let set = SetCurrentConsoleFontEx(GetStdHandle(STD_OUTPUT_HANDLE), 0, font) != 0;
            if set && placed {
                // The console resizes its window a moment after the call
                // returns, so putting it back straight away can be undone.
                // Try, look, and try again until it has stayed put.
                for _ in 0..6 {
                    const KEEP_ORDER_AND_FOCUS: u32 = 0x0004 | 0x0010; // SWP_NOZORDER | SWP_NOACTIVATE
                    let (width, height) = (before.right - before.left, before.bottom - before.top);
                    SetWindowPos(
                        window,
                        std::ptr::null_mut(),
                        before.left,
                        before.top,
                        width,
                        height,
                        KEEP_ORDER_AND_FOCUS,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    let mut after: RECT = std::mem::zeroed();
                    let same = GetWindowRect(window, &mut after) != 0
                        && (after.right - after.left - width).abs() <= SNAP
                        && (after.bottom - after.top - height).abs() <= SNAP;
                    if same {
                        break;
                    }
                }
            }
            set
        }
    }

    /// The console trims its window to a whole number of characters, so
    /// "the same size" can be out by up to one character's worth of pixels.
    const SNAP: i32 = 40;

    /// Fixed-pitch fonts known to carry braille, best first. Cascadia ships
    /// with Windows 11, Windows Terminal and Visual Studio; the rest are
    /// common programmers' fonts. Each is checked before it is used, so a
    /// wrong entry here costs nothing.
    pub const FONTS: [&str; 9] = [
        "Cascadia Mono",
        "Cascadia Code",
        "DejaVu Sans Mono",
        "JetBrains Mono",
        "Fira Code",
        "Iosevka",
        "Hack",
        "Noto Sans Mono",
        "FreeMono",
    ];

    /// The font to put back on exit.
    static SAVED: Mutex<Option<CONSOLE_FONT_INFOEX>> = Mutex::new(None);

    /// A real console window is on screen. Behind a pseudoconsole the
    /// window exists too, but is never shown.
    pub fn is_classic_console() -> bool {
        // SAFETY: both calls take no pointers of ours; a null window handle
        // is checked before use.
        unsafe {
            let window = GetConsoleWindow();
            !window.is_null() && IsWindowVisible(window) != 0
        }
    }

    pub fn current_font() -> Option<CONSOLE_FONT_INFOEX> {
        // SAFETY: `info` is a plain struct of integers, valid when zeroed,
        // with `cbSize` set as the API requires before the call fills it.
        unsafe {
            let mut info: CONSOLE_FONT_INFOEX = std::mem::zeroed();
            info.cbSize = size_of::<CONSOLE_FONT_INFOEX>() as u32;
            (GetCurrentConsoleFontEx(GetStdHandle(STD_OUTPUT_HANDLE), 0, &mut info) != 0)
                .then_some(info)
        }
    }

    pub fn face_of(font: &CONSOLE_FONT_INFOEX) -> String {
        let name = font.FaceName;
        let end = name
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end])
    }

    /// Does the installed font called `face` have braille glyphs, and is it
    /// fixed-pitch? Asked of GDI, the same machinery the console draws with.
    pub fn draws_braille(face: &str) -> bool {
        if face.is_empty() {
            return false;
        }
        let wide: Vec<u16> = face.encode_utf16().chain([0]).collect();
        // SAFETY: `wide` is NUL-terminated and outlives the calls. The DC
        // and font are checked for null, the previous object is selected
        // back before the font is deleted, and both handles are released.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return false;
            }
            let font = CreateFontW(0, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0, wide.as_ptr());
            let mut capable = false;
            if !font.is_null() {
                let previous = SelectObject(dc, font);
                // Ask for a font that is not installed and GDI quietly
                // hands over a different one. Check it is the one named.
                let mut actual = [0u16; 64];
                GetTextFaceW(dc, actual.len() as i32, actual.as_mut_ptr());
                let end = actual.iter().position(|unit| *unit == 0).unwrap_or(0);
                let found = String::from_utf16_lossy(&actual[..end]).eq_ignore_ascii_case(face);

                let mut metrics: TEXTMETRICW = std::mem::zeroed();
                // The low bit set means *variable* pitch, despite its name.
                let fixed = GetTextMetricsW(dc, &mut metrics) != 0
                    && (metrics.tmPitchAndFamily as u32) & 1 == 0;

                // A sparse pattern, a dense one, and all eight dots.
                let probe: [u16; 3] = [0x2801, 0x2847, 0x28FF];
                let mut glyphs = [0u16; 3];
                const MARK_MISSING: u32 = 1; // GGI_MARK_NONEXISTING_GLYPHS
                let answered =
                    GetGlyphIndicesW(dc, probe.as_ptr(), 3, glyphs.as_mut_ptr(), MARK_MISSING);
                let present = answered != u32::MAX && glyphs.iter().all(|glyph| *glyph != 0xFFFF);

                capable = found && fixed && present;
                SelectObject(dc, previous);
                DeleteObject(font);
            }
            DeleteDC(dc);
            capable
        }
    }

    /// Change this console window's font to `face`, keeping its size, and
    /// remember `original` so `restore` can put it back.
    pub fn switch_to(original: &CONSOLE_FONT_INFOEX, face: &str) -> bool {
        let mut wanted = *original;
        wanted.nFont = 0;
        wanted.dwFontSize.X = 0; // the console picks the width that suits the height
        wanted.FontFamily = 54; // FF_MODERN | TMPF_VECTOR | TMPF_TRUETYPE
        wanted.FontWeight = 400;
        wanted.FaceName = [0; 32];
        for (slot, unit) in wanted.FaceName.iter_mut().zip(face.encode_utf16().take(31)) {
            *slot = unit;
        }
        // The console refuses fonts it does not like by silently keeping
        // the old one, so believe only what it reports back.
        let took = set_font(&wanted)
            && current_font().is_some_and(|now| face_of(&now).eq_ignore_ascii_case(face));
        if took {
            remember(original);
        }
        took
    }

    /// Note the font to go back to, the first time anything is changed.
    fn remember(original: &CONSOLE_FONT_INFOEX) {
        if let Ok(mut saved) = SAVED.lock() {
            saved.get_or_insert(*original);
        }
    }

    /// Change the text height, keeping the font.
    pub fn set_height(pixels: i16) -> bool {
        let Some(original) = current_font() else {
            return false;
        };
        let mut wanted = original;
        wanted.nFont = 0;
        wanted.dwFontSize.X = 0;
        wanted.dwFontSize.Y = pixels;
        let set = set_font(&wanted);
        if set {
            remember(&original);
        }
        set
    }

    pub fn restore() {
        // `try_lock`, not `lock`: this can run inside a panic hook, where
        // waiting on a lock the panicking thread holds would hang.
        let Ok(mut saved) = SAVED.try_lock() else {
            return;
        };
        if let Some(original) = saved.take() {
            // The window keeps its size on the way out too: whoever owns
            // this console gets it back as they left it, to the pixel.
            set_font(&original);
        }
    }
}

#[cfg(windows)]
mod clipboard {
    use windows_sys::Win32::System::{
        DataExchange::{
            CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        },
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    };

    const UNICODE_TEXT: u32 = 13; // CF_UNICODETEXT
    /// Far more than any key or airport name; a guard against pasting a book.
    const MOST_UNITS: usize = 4096;

    pub fn text() -> Option<String> {
        // SAFETY: the clipboard is opened before it is read and closed on
        // every path after that. The data handle stays the clipboard's: it
        // is only read, while locked, and never past the size the system
        // reports for it.
        unsafe {
            if IsClipboardFormatAvailable(UNICODE_TEXT) == 0
                || OpenClipboard(std::ptr::null_mut()) == 0
            {
                return None;
            }
            let mut text = None;
            let handle = GetClipboardData(UNICODE_TEXT);
            if !handle.is_null() {
                let data = GlobalLock(handle) as *const u16;
                if !data.is_null() {
                    let units = (GlobalSize(handle) / 2).min(MOST_UNITS);
                    let held = std::slice::from_raw_parts(data, units);
                    let end = held.iter().position(|unit| *unit == 0).unwrap_or(units);
                    text = Some(String::from_utf16_lossy(&held[..end]));
                    GlobalUnlock(handle);
                }
            }
            CloseClipboard();
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever is on the clipboard - text, a picture, nothing - reading it
    /// must neither fail nor hang. (What it holds is nobody's business
    /// here, so it is not looked at.)
    #[test]
    fn reading_the_clipboard_is_safe_whatever_it_holds() {
        let _ = clipboard_text();
        let _ = clipboard_text(); // and it was closed properly the first time
    }

    #[test]
    fn an_explicit_charset_is_obeyed_without_touching_the_console() {
        assert_eq!(prepare("rich", 0), "rich");
        assert_eq!(prepare("RICH", 0), "rich");
        assert_eq!(prepare("safe", 0), "safe");
        restore(); // nothing was changed, so nothing to undo; must not panic
    }

    #[test]
    fn text_sizes_step_along_the_ladder_and_stop_at_its_ends() {
        assert_eq!(step_size(24, -1), 20);
        assert_eq!(step_size(24, 1), 28);
        assert_eq!(
            step_size(15, -1),
            12,
            "an odd size snaps to its nearest rung first"
        );
        assert_eq!(step_size(6, -1), 6);
        assert_eq!(step_size(36, 1), 36);
    }

    /// The probe must tell a font with braille from one without. Consolas
    /// ships with every Windows since Vista and has none; a made-up name
    /// must not be mistaken for whatever GDI substitutes for it.
    #[cfg(windows)]
    #[test]
    fn the_font_probe_is_not_fooled() {
        assert!(!classic::draws_braille("Consolas"));
        assert!(!classic::draws_braille("No Such Font 4471"));
        assert!(!classic::draws_braille(""));
        // Proportional fonts are no use to a console even with the glyphs.
        assert!(!classic::draws_braille("Segoe UI Symbol"));
    }
}
