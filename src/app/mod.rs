//! The viewer application: owns the document, display protocol, terminal, and
//! view state, and runs the main event loop.
//!
//! View model: a page is rendered full at the current zoom into one cached RGBA
//! buffer; the visible viewport is an in-memory crop of that buffer, encoded and
//! cached per distinct view. Zoom is a factor relative to fit-page; fit-page and
//! fit-width are presets.
//!
//! Input is *coalesced*: a burst of keystrokes (e.g. holding a scroll key) is
//! drained and applied before a single redraw, so panning stays responsive even
//! though each encode+transmit costs tens of ms.

pub mod state;

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType};

use crate::Result;
use crate::bookmarks::{self, FileState};
use crate::cache::{CacheKey, RenderCache};
use crate::config::Config;
use crate::document::{Document, RenderRequest};
use crate::render::rgba::RgbaImage;
use crate::render::{Backend, EncodedImage, ImageProtocol, Placement, SrcRect, TermCaps};
use crate::terminal::tty;
use crate::terminal::{self, Terminal};
use state::{Fit, Rotation, Sharpness, Theme};

/// Encoded-crop cache budget (payloads are ~tens of KB, so this holds plenty).
const CACHE_BUDGET_BYTES: usize = 96 * 1024 * 1024;
/// Cap on a single full-page render, to bound memory and encode time. Beyond
/// this, zoom becomes "digital" (crop of a capped-resolution render).
const MAX_FULL_PIXELS: u64 = 16_000_000;
const ZOOM_STEP: f32 = 1.2;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 6.0;

/// Status-bar (bottom menu bar) colors: a light dark-grey bar with black text.
/// Fixed for both themes — the bar is chrome, not page content.
const STATUS_BG: Color = Color::Rgb {
    r: 0xa9,
    g: 0xa9,
    b: 0xa9,
};
const STATUS_FG: Color = Color::Black;

/// One full-page RGBA render (already rotated for display) kept around so panning
/// doesn't re-rasterize.
struct FullRender {
    page: usize,
    render_w: u32,
    rotation: Rotation,
    img: Rc<RgbaImage>,
}

/// A view to pre-encode into the cache during idle time.
#[derive(Clone, Copy)]
struct ViewJob {
    page: usize,
    fit: Fit,
    cx: f32,
    cy: f32,
}

/// The last interaction, used to predict the next view to pre-encode.
#[derive(Clone, Copy)]
enum Motion {
    None,
    /// Pan/scroll by (dcx, dcy) fractions.
    Pan(f32, f32),
    /// Zoom by a multiplicative factor.
    Zoom(f32),
}

/// What the UI is showing.
#[derive(Clone, Copy)]
enum Mode {
    /// Normal page view.
    View,
    /// The bookmark list overlay, with the highlighted entry index.
    BookmarkList { selected: usize },
    /// The help overlay.
    Help,
    /// The `:` go-to-page prompt (input is in `App::goto_buffer`).
    Goto,
}

pub struct App {
    term: Terminal,
    caps: TermCaps,
    proto: Box<dyn ImageProtocol>,
    doc: Box<dyn Document>,
    cache: RenderCache,
    page: usize,
    fit: Fit,
    sharpness: Sharpness,
    theme: Theme,
    /// View rotation (90° steps), applied to every page. Transient view state.
    rotation: Rotation,
    /// Pan center as a fraction [0,1] of the full render (zoom keeps this fixed).
    cx: f32,
    cy: f32,
    last_full: Option<FullRender>,
    last_area: Option<Placement>,
    /// Region-panning resident key `(page, render_w, rotation, theme)`: while it
    /// stays equal, panning just re-places the transmitted image instead of
    /// re-encoding a crop. Only used when the backend supports region panning;
    /// reset to `None` whenever the image is cleared so we re-transmit.
    resident: Option<(usize, u32, Rotation, u8)>,
    prefetch: VecDeque<ViewJob>,
    /// Last interaction, to predict and pre-encode the next view.
    motion: Motion,
    /// Set by input handling; cleared after a redraw.
    dirty: bool,
    /// Stop draining the current input burst and redraw now (used so reaching a
    /// scroll edge renders before a page flip can be processed).
    break_batch: bool,
    /// Force a full screen clear on the next redraw (after a resize).
    force_clear: bool,
    /// Path of the open document, for persisting state on exit.
    path: PathBuf,
    /// Short label for the input format (PDF / Markdown / LaTeX / PNG / …),
    /// shown in the status line.
    kind: &'static str,
    /// Bookmarks + resume state for this document.
    state: FileState,
    mode: Mode,
    /// Digits typed in the `:` go-to-page prompt.
    goto_buffer: String,
}

impl App {
    pub fn new(
        term: Terminal,
        caps: TermCaps,
        proto: Box<dyn ImageProtocol>,
        doc: Box<dyn Document>,
        path: PathBuf,
        defaults: Config,
    ) -> Self {
        let kind = crate::document::kind_label(&path);
        // Restore where we left off (page + fit + sharpness + theme), clamped.
        let state = bookmarks::load(&path);
        let last = doc.page_count().saturating_sub(1);
        let page = state.last_page.min(last);
        // `defaults` already folds CLI flags over the user config; here it in turn
        // overrides the remembered per-file state, which overrides the built-in
        // default. The page position is always resumed from the per-file state.
        let fit = defaults
            .fit
            .or_else(|| state.fit.as_deref().and_then(Fit::parse))
            .unwrap_or(Fit::Page);
        let sharpness = defaults
            .sharpness
            .or_else(|| state.sharpness.as_deref().and_then(Sharpness::parse))
            .unwrap_or_default();
        let theme = defaults
            .theme
            .or_else(|| state.theme.as_deref().and_then(Theme::parse))
            .unwrap_or_default();

        Self {
            term,
            caps,
            proto,
            doc,
            cache: RenderCache::new(CACHE_BUDGET_BYTES),
            page,
            fit,
            sharpness,
            theme,
            rotation: Rotation::None,
            cx: 0.5,
            cy: 0.0,
            last_full: None,
            last_area: None,
            resident: None,
            prefetch: VecDeque::new(),
            motion: Motion::None,
            dirty: false,
            break_batch: false,
            force_clear: false,
            path,
            kind,
            state,
            mode: Mode::View,
            goto_buffer: String::new(),
        }
    }

    /// Re-detect the window geometry after a resize and force a full redraw.
    /// Reached both from crossterm's `Event::Resize` and from our own SIGWINCH
    /// wake-up (see `terminal::tty`).
    fn on_resize(&mut self) -> Result<()> {
        let fresh = terminal::geometry::detect()?;
        self.caps.size = if self.caps.size.cell_px_known {
            fresh.with_cell_px(self.caps.size.cell_w, self.caps.size.cell_h)
        } else {
            fresh
        };
        self.last_full = None;
        self.force_clear = true;
        self.dirty = true;
        Ok(())
    }

    pub fn run(&mut self) -> Result<()> {
        self.set_window_title()?;
        self.render()?;
        self.enqueue_prefetch();
        terminal::query::drain_stdin();

        // Take over SIGWINCH for our own wait (see `terminal::tty`). The priming
        // poll builds crossterm's event source — and registers its handler —
        // first, so ours is the one that survives.
        let _ = event::poll(Duration::ZERO);
        tty::install_resize_wake();

        let mut quit = false;
        while !quit {
            let timeout = if self.prefetch.is_empty() {
                Duration::from_millis(200)
            } else {
                Duration::from_millis(0)
            };

            // Wait on the terminal ourselves rather than blocking inside
            // crossterm, whose event source spins forever once the terminal has
            // hung up (see `terminal::tty`). Crossterm is asked first with a zero
            // timeout — free, and it releases any event already parsed — and only
            // ever gets to read an fd we have just seen alive.
            if tty::hung_up() {
                break; // terminal gone: fall through to the state-saving exit
            }
            let mut ready = event::poll(Duration::ZERO)?;
            let mut resized = false;
            if !ready && !timeout.is_zero() {
                match tty::wait(timeout) {
                    tty::Wait::Hangup => break,
                    tty::Wait::Ready => ready = event::poll(Duration::ZERO)?,
                    tty::Wait::Resized => {
                        self.on_resize()?;
                        resized = true;
                    }
                    tty::Wait::Timeout => {}
                }
            }

            if ready {
                // Drain the whole burst of pending events, then redraw once.
                loop {
                    match event::read()? {
                        Event::Key(key) => {
                            if self.handle_key(key)? {
                                quit = true;
                                break;
                            }
                        }
                        Event::Mouse(mouse) => self.handle_mouse(mouse)?,
                        Event::Resize(_, _) => self.on_resize()?,
                        _ => {}
                    }
                    if self.break_batch {
                        self.break_batch = false;
                        break; // redraw the scroll edge before processing more input
                    }
                    if tty::hung_up() {
                        quit = true;
                        break;
                    }
                    if !event::poll(Duration::from_millis(0))? {
                        break;
                    }
                }
            }
            if ready || resized {
                if self.dirty {
                    self.render()?;
                    self.enqueue_prefetch();
                    self.dirty = false;
                }
            } else if let Some(job) = self.prefetch.pop_front() {
                let _ = self.warm(job);
            }
        }

        // Persist resume state + bookmarks (best-effort).
        self.state.last_page = self.page;
        self.state.fit = Some(self.fit.serialize());
        self.state.sharpness = Some(self.sharpness.label().to_string());
        self.state.theme = Some(self.theme.label().to_string());
        let _ = bookmarks::save(&self.path, &self.state);

        let s = self.caps.size;
        let full = Placement {
            col: 0,
            row: 0,
            cols: s.cols,
            rows: s.rows,
        };
        self.proto.clear(self.term.out(), &full)?;
        self.term.flush()?;
        Ok(())
    }

    /// Handle a key; mutate state and mark dirty. Returns `true` to quit.
    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        match self.mode {
            Mode::View => self.handle_view_key(key),
            Mode::BookmarkList { .. } => {
                self.handle_list_key(key);
                Ok(false)
            }
            Mode::Help => {
                self.close_overlay(); // any key dismisses help
                Ok(false)
            }
            Mode::Goto => {
                self.handle_goto_key(key);
                Ok(false)
            }
        }
    }

    /// Handle a mouse event: the wheel scrolls (flipping pages at a page edge,
    /// exactly like `j`/`k`), left click advances a page and right click goes
    /// back one. Only button *presses* act, so a click is one page, not two.
    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        match self.mode {
            Mode::View => match mouse.kind {
                MouseEventKind::ScrollDown => self.scroll_down()?,
                MouseEventKind::ScrollUp => self.scroll_up()?,
                // Horizontal wheel/trackpad pans, mirroring h/l.
                MouseEventKind::ScrollRight => self.pan_h(1.0)?,
                MouseEventKind::ScrollLeft => self.pan_h(-1.0)?,
                MouseEventKind::Down(MouseButton::Left) => self.next_page(),
                MouseEventKind::Down(MouseButton::Right) => self.prev_page(false),
                _ => {}
            },
            // Any click dismisses help, mirroring "any key closes".
            Mode::Help => {
                if matches!(mouse.kind, MouseEventKind::Down(_)) {
                    self.close_overlay();
                }
            }
            // In the list the wheel moves the selection; clicks are ignored so a
            // stray one can't jump the document out from under the overlay.
            Mode::BookmarkList { .. } => match mouse.kind {
                MouseEventKind::ScrollDown => self.list_move(true),
                MouseEventKind::ScrollUp => self.list_move(false),
                _ => {}
            },
            Mode::Goto => {}
        }
        Ok(())
    }

    fn handle_view_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(true);
            }

            KeyCode::Char('j') | KeyCode::Char('s') | KeyCode::Down => self.scroll_down()?,
            KeyCode::Char('k') | KeyCode::Char('w') | KeyCode::Up => self.scroll_up()?,
            KeyCode::Char('l') | KeyCode::Char('d') | KeyCode::Right => self.pan_h(1.0)?,
            KeyCode::Char('h') | KeyCode::Char('a') | KeyCode::Left => self.pan_h(-1.0)?,

            KeyCode::Char('n') | KeyCode::Char(' ') | KeyCode::PageDown => self.next_page(),
            KeyCode::Char('p') | KeyCode::PageUp => self.prev_page(false),
            KeyCode::Char('g') | KeyCode::Home => self.goto(0, 0.0),
            KeyCode::Char('G') | KeyCode::End => {
                self.goto(self.doc.page_count().saturating_sub(1), 0.0)
            }

            KeyCode::Char('+') => self.zoom_by(ZOOM_STEP)?,
            KeyCode::Char('-') => self.zoom_by(1.0 / ZOOM_STEP)?,
            KeyCode::Char('=') | KeyCode::Char('0') => self.set_fit(Fit::Page),
            KeyCode::Char('_') => self.set_fit(Fit::Width),

            // Sharpness (display resolution vs. speed).
            KeyCode::Char(']') => self.set_sharpness(self.sharpness.sharper()),
            KeyCode::Char('[') => self.set_sharpness(self.sharpness.softer()),

            // Color theme: toggle off <-> nord (dark).
            KeyCode::Char('i') => self.set_theme(self.theme.next()),

            // Rotation (90° steps): Shift+L clockwise, Shift+H counter-clockwise.
            KeyCode::Char('L') => self.rotate(self.rotation.cw()),
            KeyCode::Char('H') => self.rotate(self.rotation.ccw()),

            // Bookmarks.
            KeyCode::Char('m') => {
                self.state.toggle(self.page);
                self.dirty = true; // refresh the status indicator
            }
            KeyCode::Char('\'') | KeyCode::Char('`') => self.open_bookmark_list(),

            KeyCode::Char('?') => {
                self.mode = Mode::Help;
                self.dirty = true;
            }
            KeyCode::Char(':') => {
                self.goto_buffer.clear();
                self.mode = Mode::Goto;
                self.dirty = true;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_goto_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::View;
                self.dirty = true; // redraw to clear the prompt
            }
            KeyCode::Enter => {
                let target = self.goto_buffer.trim().parse::<usize>().ok();
                self.mode = Mode::View;
                match target {
                    Some(n) if n >= 1 => self.goto(n - 1, 0.0),
                    _ => {}
                }
                self.dirty = true;
            }
            KeyCode::Backspace => {
                self.goto_buffer.pop();
                self.dirty = true;
            }
            KeyCode::Char(c) if c.is_ascii_digit() && self.goto_buffer.len() < 9 => {
                self.goto_buffer.push(c);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn close_overlay(&mut self) {
        self.mode = Mode::View;
        self.force_clear = true;
        self.dirty = true;
    }

    fn handle_list_key(&mut self, key: KeyEvent) {
        let Mode::BookmarkList { selected } = self.mode else {
            return;
        };
        let count = self.state.bookmarks.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('\'') => self.close_bookmark_list(),
            KeyCode::Char('j') | KeyCode::Down => self.list_move(true),
            KeyCode::Char('k') | KeyCode::Up => self.list_move(false),
            KeyCode::Char('d') => {
                if selected < count {
                    self.state.bookmarks.remove(selected);
                    let new_count = self.state.bookmarks.len();
                    self.mode = Mode::BookmarkList {
                        selected: selected.min(new_count.saturating_sub(1)),
                    };
                    self.dirty = true;
                }
            }
            KeyCode::Enter => {
                let target = self.state.bookmarks.get(selected).map(|b| b.page);
                self.close_bookmark_list();
                if let Some(page) = target {
                    self.goto(page, 0.0);
                }
            }
            _ => {}
        }
    }

    /// Move the bookmark-list selection one entry down (`down`) or up, clamped at
    /// the ends. Shared by `j`/`k` and the mouse wheel.
    fn list_move(&mut self, down: bool) {
        let Mode::BookmarkList { selected } = self.mode else {
            return;
        };
        let count = self.state.bookmarks.len();
        let next = if down {
            (selected + 1).min(count.saturating_sub(1))
        } else {
            selected.saturating_sub(1)
        };
        if next != selected {
            self.mode = Mode::BookmarkList { selected: next };
            self.dirty = true;
        }
    }

    fn open_bookmark_list(&mut self) {
        // Start on the current page's bookmark if there is one.
        let selected = self
            .state
            .bookmarks
            .iter()
            .position(|b| b.page == self.page)
            .unwrap_or(0);
        self.mode = Mode::BookmarkList { selected };
        self.dirty = true;
    }

    fn close_bookmark_list(&mut self) {
        self.mode = Mode::View;
        self.force_clear = true;
        self.dirty = true;
    }

    // ---- view geometry ------------------------------------------------------

    fn viewport(&self) -> Viewport {
        let s = self.caps.size;
        let area_rows = s.rows.saturating_sub(1).max(1); // bottom row = status line
        Viewport {
            cols: s.cols,
            rows: area_rows,
            width_px: s.cols as u32 * s.cell_w as u32,
            height_px: area_rows as u32 * s.cell_h as u32,
            cell_w: s.cell_w,
            cell_h: s.cell_h,
        }
    }

    fn fit_page_width(&self, vp: &Viewport, aspect: f32) -> f32 {
        (vp.width_px as f32).min(vp.height_px as f32 * aspect)
    }

    /// The page's aspect ratio as it appears on screen: the intrinsic aspect,
    /// inverted when the current rotation swaps the width and height axes (90° /
    /// 270°). All view planning works in this displayed orientation.
    fn display_aspect(&self, page: usize) -> Result<f32> {
        let aspect = self.doc.page_info(page)?.aspect();
        Ok(if self.rotation.swaps_axes() {
            1.0 / aspect.max(0.0001)
        } else {
            aspect
        })
    }

    /// Set the terminal window title to the document title (or filename).
    fn set_window_title(&mut self) -> Result<()> {
        let title = self.doc.title().map(str::to_string).unwrap_or_else(|| {
            self.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("document")
                .to_string()
        });
        write!(self.term.out(), "\x1b]2;folio — {title}\x07")?;
        Ok(())
    }

    /// Whether the backend can scale an image to a fixed cell box (kitty `c`/`r`).
    /// If so, we can render fewer pixels and let the terminal upscale.
    fn cell_scaling(&self) -> bool {
        matches!(self.caps.backend, Backend::Kitty)
    }

    fn plan(&self, page: usize) -> Result<(Viewport, ViewPlan)> {
        let vp = self.viewport();
        let aspect = self.display_aspect(page)?;
        let plan = plan_view(
            aspect,
            &vp,
            self.fit,
            self.cx,
            self.cy,
            self.cell_scaling(),
            self.sharpness.max_display_pixels(),
        );
        Ok((vp, plan))
    }

    // ---- navigation / panning (state only; redraw is coalesced) -------------

    fn goto(&mut self, page: usize, cy: f32) {
        let last = self.doc.page_count().saturating_sub(1);
        let page = page.min(last);
        if page != self.page || cy != self.cy {
            self.page = page;
            self.cy = cy;
            self.motion = Motion::None;
            self.dirty = true;
        }
    }

    fn next_page(&mut self) {
        let last = self.doc.page_count().saturating_sub(1);
        if self.page < last {
            self.goto(self.page + 1, 0.0);
        }
    }

    fn prev_page(&mut self, land_bottom: bool) {
        if self.page > 0 {
            self.goto(self.page - 1, if land_bottom { 1.0 } else { 0.0 });
        }
    }

    fn scroll_down(&mut self) -> Result<()> {
        let (vp, plan) = self.plan(self.page)?;
        let max_y = plan.render_h.saturating_sub(plan.win_h);
        if max_y == 0 {
            // Page fits vertically — nothing to scroll, so advance immediately.
            self.next_page();
        } else if self.cy < 1.0 {
            // Scrollable: scroll down. The press that reaches the bottom renders
            // the edge as its own frame (break_batch) so the next press flips.
            let step = (vp.height_px / 6).max(1) as f32 / max_y as f32;
            self.cy = (self.cy + step).min(1.0);
            self.motion = Motion::Pan(0.0, step);
            self.dirty = true;
            if self.cy >= 1.0 {
                self.break_batch = true;
            }
        } else {
            // Already at the bottom edge: a second press flips to the next page.
            self.next_page();
        }
        Ok(())
    }

    fn scroll_up(&mut self) -> Result<()> {
        let (vp, plan) = self.plan(self.page)?;
        let max_y = plan.render_h.saturating_sub(plan.win_h);
        if max_y == 0 {
            self.prev_page(false);
        } else if self.cy > 0.0 {
            // Scrollable: scroll up; reaching the top renders that edge first.
            let step = (vp.height_px / 6).max(1) as f32 / max_y as f32;
            self.cy = (self.cy - step).max(0.0);
            self.motion = Motion::Pan(0.0, -step);
            self.dirty = true;
            if self.cy <= 0.0 {
                self.break_batch = true;
            }
        } else {
            // Already at the top edge: a second press flips to the previous page
            // (landing at its bottom so upward reading flows).
            self.prev_page(true);
        }
        Ok(())
    }

    fn pan_h(&mut self, dir: f32) -> Result<()> {
        let (vp, plan) = self.plan(self.page)?;
        let max_x = plan.render_w.saturating_sub(plan.win_w);
        if max_x == 0 {
            return Ok(()); // nothing to pan
        }
        let step = (vp.width_px / 6).max(1) as f32 / max_x as f32;
        self.cx = (self.cx + dir * step).clamp(0.0, 1.0);
        self.motion = Motion::Pan(dir * step, 0.0);
        self.dirty = true;
        Ok(())
    }

    fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
        self.cx = 0.5;
        self.cy = 0.0;
        self.motion = Motion::None;
        self.dirty = true;
    }

    fn set_sharpness(&mut self, sharpness: Sharpness) {
        if sharpness != self.sharpness {
            self.sharpness = sharpness;
            self.last_full = None; // render resolution changed -> re-rasterize
            self.motion = Motion::None;
            self.dirty = true;
        }
    }

    fn set_theme(&mut self, theme: Theme) {
        if theme != self.theme {
            // The theme recolors the encoded crop; the full render is untouched,
            // so no re-rasterize is needed (just re-encode under the new key).
            self.theme = theme;
            self.motion = Motion::None;
            self.dirty = true;
        }
    }

    fn rotate(&mut self, rotation: Rotation) {
        if rotation != self.rotation {
            self.rotation = rotation;
            // The scroll axes may have swapped, so reset to the top-center to frame
            // the new orientation sensibly instead of keeping a stale offset.
            self.cx = 0.5;
            self.cy = 0.0;
            self.motion = Motion::None;
            self.dirty = true;
        }
    }

    fn zoom_by(&mut self, factor: f32) -> Result<()> {
        let vp = self.viewport();
        let aspect = self.display_aspect(self.page)?;
        let fpw = self.fit_page_width(&vp, aspect);
        let current = logical_zoom(self.fit, &vp, fpw);
        let z = (current * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.fit = Fit::Zoom(z);
        self.motion = Motion::Zoom(factor);
        self.dirty = true;
        Ok(())
    }

    // ---- rendering ----------------------------------------------------------

    fn enqueue_prefetch(&mut self) {
        self.prefetch.clear();
        // Region panning re-places instantly, so warming encoded crops would be
        // wasted work (the region path never reads the crop cache).
        if self.proto.supports_region_panning() {
            return;
        }
        let last = self.doc.page_count().saturating_sub(1);

        // Predicted continuations of the current interaction come first — the
        // most likely next view — turning repeated pan/scroll/zoom into cache hits.
        match self.motion {
            Motion::Pan(dcx, dcy) => {
                for k in 1..=2u8 {
                    let cx = (self.cx + dcx * k as f32).clamp(0.0, 1.0);
                    let cy = (self.cy + dcy * k as f32).clamp(0.0, 1.0);
                    self.prefetch.push_back(ViewJob {
                        page: self.page,
                        fit: self.fit,
                        cx,
                        cy,
                    });
                }
            }
            Motion::Zoom(factor) => {
                if let Fit::Zoom(z) = self.fit {
                    for k in 1..=2i32 {
                        let nz = (z * factor.powi(k)).clamp(ZOOM_MIN, ZOOM_MAX);
                        self.prefetch.push_back(ViewJob {
                            page: self.page,
                            fit: Fit::Zoom(nz),
                            cx: self.cx,
                            cy: self.cy,
                        });
                    }
                }
            }
            Motion::None => {}
        }

        // Then neighbor pages, at their first-view (top) position.
        if self.page < last {
            self.prefetch.push_back(ViewJob {
                page: self.page + 1,
                fit: self.fit,
                cx: 0.5,
                cy: 0.0,
            });
        }
        if self.page > 0 {
            self.prefetch.push_back(ViewJob {
                page: self.page - 1,
                fit: self.fit,
                cx: 0.5,
                cy: 0.0,
            });
        }
    }

    /// Warm a predicted view's crop into the cache (without disturbing the
    /// retained full-render buffer for the current page).
    fn warm(&mut self, job: ViewJob) -> Result<()> {
        let vp = self.viewport();
        let aspect = self.display_aspect(job.page)?;
        let plan = plan_view(
            aspect,
            &vp,
            job.fit,
            job.cx,
            job.cy,
            self.cell_scaling(),
            self.sharpness.max_display_pixels(),
        );
        let _ = self.encoded(job.page, &plan, false)?;
        Ok(())
    }

    /// Encoded crop for `(page, plan)`, via the cache. `set_full` retains the full
    /// render as the pan buffer (only for the page actually on screen).
    fn encoded(&mut self, page: usize, plan: &ViewPlan, set_full: bool) -> Result<Rc<EncodedImage>> {
        let key = CacheKey {
            page,
            render_w: plan.render_w,
            win_x: plan.win_x,
            win_y: plan.win_y,
            win_w: plan.win_w,
            win_h: plan.win_h,
            theme: self.theme.discriminant(),
            rotation: self.rotation.quarter_turns(),
        };
        if let Some(enc) = self.cache.get(key) {
            return Ok(enc);
        }
        let full = self.full_render(page, plan.render_w, plan.render_h, set_full)?;
        let mut crop = full.crop(plan.win_x, plan.win_y, plan.win_w, plan.win_h);
        self.theme.apply(&mut crop.data); // Nord dark-mode color transform
        let encoded = self.proto.encode(&crop)?;
        Ok(self.cache.insert(key, encoded))
    }

    /// Render `page` at the display dimensions (`render_w` × `render_h`) and apply
    /// the current rotation. For a 90°/270° view the page is rasterized with its
    /// axes swapped (target width = `render_h`) so that, once rotated, it lands at
    /// exactly `render_w` × `render_h` — matching the crop window the plan computed.
    fn full_render(
        &mut self,
        page: usize,
        render_w: u32,
        render_h: u32,
        set_full: bool,
    ) -> Result<Rc<RgbaImage>> {
        let rotation = self.rotation;
        if let Some(f) = &self.last_full
            && f.page == page
            && f.render_w == render_w
            && f.rotation == rotation
        {
            return Ok(f.img.clone());
        }
        // Rasterize in the page's own orientation; for axis-swapping rotations the
        // unrotated width is the display *height*.
        let target_width_px = if rotation.swaps_axes() {
            render_h
        } else {
            render_w
        };
        let req = RenderRequest {
            target_width_px,
            max_height_px: None,
        };
        let rendered = self.doc.render_page(page, &req)?;
        let img = Rc::new(match rotation {
            Rotation::None => rendered,
            r => rendered.rotated(r.quarter_turns()),
        });
        if set_full {
            self.last_full = Some(FullRender {
                page,
                render_w,
                rotation,
                img: img.clone(),
            });
        }
        Ok(img)
    }

    fn render(&mut self) -> Result<()> {
        match self.mode {
            Mode::View => self.render_page(),
            Mode::BookmarkList { selected } => self.render_bookmark_list(selected),
            Mode::Help => self.render_help(),
            Mode::Goto => self.render_goto_prompt(),
        }
    }

    fn render_page(&mut self) -> Result<()> {
        let (vp, plan) = self.plan(self.page)?;

        // Cell footprint comes from the plan (logical window), not the encoded
        // pixels — the image may be a downscaled bitmap scaled up to fill it.
        let at = Placement {
            col: vp.cols.saturating_sub(plan.foot_cols) / 2,
            row: vp.rows.saturating_sub(plan.foot_rows) / 2,
            cols: plan.foot_cols,
            rows: plan.foot_rows,
        };

        if self.force_clear {
            queue!(self.term.out(), Clear(ClearType::All))?;
            self.force_clear = false;
            self.last_area = None;
            self.resident = None; // force a re-transmit in region-panning mode
        }

        // Emit the page image: region panning (transmit once, then re-place a
        // sub-rectangle while panning) where the backend supports it, else the
        // per-window crop-encode path. A failed render (rare) shouldn't kill the
        // viewer — show a message and let the user page away or quit.
        let emitted = if self.proto.supports_region_panning() {
            self.emit_region(&plan, &at)
        } else {
            self.emit_crop(&plan, &at)
        };
        if emitted.is_err() {
            return self.render_message(&format!("could not render page {}", self.page + 1));
        }

        // Erase only the cells the previous image covered but this one doesn't.
        // For panning (same area) there's nothing to erase — no flash.
        if let Some(prev) = self.last_area
            && prev != at
        {
            self.clear_residual(prev, at)?;
        }
        self.last_area = Some(at);

        self.draw_status(&plan)?;
        self.term.flush()?;
        Ok(())
    }

    /// Per-window path: crop the full render to the visible window, encode it, and
    /// transmit+display it. Each distinct view re-encodes; used by sixel and the
    /// tmux/placeholder kitty path.
    fn emit_crop(&mut self, plan: &ViewPlan, at: &Placement) -> Result<()> {
        // Draw the new image first (kitty double-buffers, so no blank gap).
        let enc = self.encoded(self.page, plan, true)?;
        self.proto.draw(self.term.out(), &enc, at)
    }

    /// Region-panning path: keep the full render resident in the terminal and, for
    /// each view, display only the visible sub-rectangle. Panning sends no pixels
    /// — the costly encode+transmit happens once per zoom/page/theme, not per
    /// frame, which is the big win for dense (photographic) images.
    fn emit_region(&mut self, plan: &ViewPlan, at: &Placement) -> Result<()> {
        let key = (self.page, plan.render_w, self.rotation, self.theme.discriminant());
        let fresh = self.resident != Some(key);
        // Build + encode the full image only when the resident view changed.
        let enc_full = if fresh {
            let full = self.full_render(self.page, plan.render_w, plan.render_h, true)?;
            let enc = if self.theme == Theme::Off {
                self.proto.encode(&full)?
            } else {
                // The theme recolors the whole image once, before it's transmitted.
                let mut themed = (*full).clone();
                self.theme.apply(&mut themed.data);
                self.proto.encode(&themed)?
            };
            Some(enc)
        } else {
            None
        };
        let src = SrcRect {
            x: plan.win_x,
            y: plan.win_y,
            w: plan.win_w,
            h: plan.win_h,
        };
        self.proto
            .draw_region(self.term.out(), enc_full.as_ref(), src, at)?;
        self.resident = Some(key);
        Ok(())
    }

    /// Erase the cells covered by `prev` but not by `new` (margins left behind
    /// when the image shrinks or moves), so old content doesn't linger.
    fn clear_residual(&mut self, prev: Placement, new: Placement) -> Result<()> {
        let out = self.term.out();
        let (nr0, nr1) = (new.row, new.row + new.rows);
        let (nc0, nc1) = (new.col, new.col + new.cols);
        for r in prev.row..(prev.row + prev.rows) {
            if r < nr0 || r >= nr1 {
                erase_cells(out, r, prev.col, prev.cols)?;
            } else {
                if prev.col < nc0 {
                    erase_cells(out, r, prev.col, nc0 - prev.col)?;
                }
                let prev_c1 = prev.col + prev.cols;
                if nc1 < prev_c1 {
                    erase_cells(out, r, nc1, prev_c1 - nc1)?;
                }
            }
        }
        Ok(())
    }

    /// Show a centered message instead of a page (e.g. a render failure), keeping
    /// the viewer usable.
    fn render_message(&mut self, msg: &str) -> Result<()> {
        if let Some(prev) = self.last_area.take() {
            self.proto.clear(self.term.out(), &prev)?;
            self.resident = None; // clearing the image invalidates the resident
        }
        let s = self.caps.size;
        let hint = format!(
            "page {}/{}  ·  n/p change page · q quit",
            self.page + 1,
            self.doc.page_count()
        );
        let row = (s.rows / 2).saturating_sub(1);
        let msg_col = s.cols.saturating_sub(msg.chars().count() as u16) / 2;
        let hint_col = s.cols.saturating_sub(hint.chars().count() as u16) / 2;

        let out = self.term.out();
        queue!(out, Clear(ClearType::All))?;
        queue!(out, MoveTo(msg_col, row), Print(msg))?;
        queue!(out, MoveTo(hint_col, row + 2), Print(&hint))?;
        self.term.flush()?;
        Ok(())
    }

    fn render_bookmark_list(&mut self, selected: usize) -> Result<()> {
        // Remove the page image, then draw the overlay as plain text.
        if let Some(prev) = self.last_area.take() {
            self.proto.clear(self.term.out(), &prev)?;
            self.resident = None;
        }
        let cols = self.caps.size.cols as usize;
        let total = self.doc.page_count();
        let current = self.page;

        let mut rows: Vec<(String, bool)> = vec![
            ("Bookmarks".to_string(), false),
            (
                "j/k move · Enter jump · d delete · Esc close".to_string(),
                false,
            ),
            (String::new(), false),
        ];
        if self.state.bookmarks.is_empty() {
            rows.push(("  (none yet — press m on a page to add one)".to_string(), false));
        } else {
            for (i, bm) in self.state.bookmarks.iter().enumerate() {
                let here = if bm.page == current { "  (current)" } else { "" };
                let label = bm.label.as_deref().unwrap_or("");
                let text = format!("  page {}/{}{}  {}", bm.page + 1, total, here, label);
                rows.push((text, i == selected));
            }
        }

        let out = self.term.out();
        queue!(out, Clear(ClearType::All))?;
        for (i, (text, highlighted)) in rows.iter().enumerate() {
            queue!(out, MoveTo(0, i as u16))?;
            if *highlighted {
                queue!(
                    out,
                    SetAttribute(Attribute::Reverse),
                    Print(truncate(text, cols)),
                    SetAttribute(Attribute::Reset),
                )?;
            } else {
                queue!(out, Print(truncate(text, cols)))?;
            }
        }
        self.term.flush()?;
        Ok(())
    }

    fn render_help(&mut self) -> Result<()> {
        if let Some(prev) = self.last_area.take() {
            self.proto.clear(self.term.out(), &prev)?;
            self.resident = None;
        }
        let cols = self.caps.size.cols as usize;
        let lines = [
            "folio — help",
            "",
            "  j / k   (s / w, ↓ / ↑)   scroll; at a page edge, press again to flip",
            "  h / l   (a / d, ← / →)   pan left / right",
            "  n / Space / p            next / previous page",
            "  g / G                    first / last page",
            "  :<n> Enter               go to page n",
            "  + / -                    zoom in / out",
            "  = / 0                    fit-page        _   fit-width",
            "  H / L                    rotate 90° counter-clockwise / clockwise",
            "  [ / ]                    sharpness softer / sharper (low…full)",
            "  i                        toggle Nord dark mode (off ↔ nord)",
            "  m                        toggle bookmark on this page",
            "  '  (or `)                bookmark list",
            "  ?                        this help",
            "  q / Esc                  quit (resumes here next time)",
            "",
            "  mouse: wheel scrolls · left click next page · right click previous",
            "",
            "  press any key to close",
        ];
        let out = self.term.out();
        queue!(out, Clear(ClearType::All))?;
        for (i, line) in lines.iter().enumerate() {
            queue!(out, MoveTo(0, i as u16), Print(truncate(line, cols)))?;
        }
        self.term.flush()?;
        Ok(())
    }

    /// Draw the `:` go-to-page prompt on the status row, leaving the page image
    /// untouched (so typing digits doesn't re-transmit the image).
    fn render_goto_prompt(&mut self) -> Result<()> {
        let s = self.caps.size;
        let prompt = format!(" goto page: {}   (Enter, Esc to cancel)", self.goto_buffer);
        let out = self.term.out();
        queue!(
            out,
            MoveTo(0, s.rows.saturating_sub(1)),
            Clear(ClearType::CurrentLine),
            SetBackgroundColor(STATUS_BG),
            SetForegroundColor(STATUS_FG),
            Print(fit_line(&prompt, s.cols as usize)),
            ResetColor,
        )?;
        self.term.flush()?;
        Ok(())
    }

    fn draw_status(&mut self, plan: &ViewPlan) -> Result<()> {
        let s = self.caps.size;
        let scroll = if plan.render_h > plan.win_h {
            let pct = plan.win_y as f32 / (plan.render_h - plan.win_h) as f32 * 100.0;
            format!("  y {pct:.0}%")
        } else {
            String::new()
        };
        let mark = if self.state.is_bookmarked(self.page) {
            " *"
        } else {
            ""
        };
        let theme = if self.theme == Theme::Off {
            String::new()
        } else {
            format!("  {}", self.theme.label())
        };
        let rotation = if self.rotation == Rotation::None {
            String::new()
        } else {
            format!("  rot {}", self.rotation.label())
        };
        let status = format!(
            " {}  {}/{}{}  {}  {}  sharp:{}{}{}{}  | jk scroll · +/- zoom · HL rotate · i theme · ? help · q quit ",
            self.kind,
            self.page + 1,
            self.doc.page_count(),
            mark,
            self.fit.label(),
            self.proto.name(),
            self.sharpness.label(),
            theme,
            rotation,
            scroll,
        );
        queue!(
            self.term.out(),
            MoveTo(0, s.rows.saturating_sub(1)),
            SetBackgroundColor(STATUS_BG),
            SetForegroundColor(STATUS_FG),
            Print(fit_line(&status, s.cols as usize)),
            ResetColor,
        )?;
        Ok(())
    }
}

struct Viewport {
    cols: u16,
    rows: u16,
    width_px: u32,
    height_px: u32,
    cell_w: u16,
    cell_h: u16,
}

/// The render size, the visible crop window within it, and the on-screen cell
/// footprint. `render_*`/`win_*` are in (possibly downscaled) render pixels;
/// `foot_*` is the cell box the window fills (independent of resolution).
struct ViewPlan {
    render_w: u32,
    render_h: u32,
    win_x: u32,
    win_y: u32,
    win_w: u32,
    win_h: u32,
    foot_cols: u16,
    foot_rows: u16,
}

/// Logical zoom (multiplier on fit-page width) implied by a fit mode.
fn logical_zoom(fit: Fit, vp: &Viewport, fit_page_w: f32) -> f32 {
    match fit {
        Fit::Page => 1.0,
        Fit::Width => (vp.width_px as f32 / fit_page_w).max(1.0),
        Fit::Zoom(z) => z,
    }
}

/// Compute the render dimensions, visible crop, and cell footprint.
///
/// "Logical" pixels are the page at full zoom resolution; `q` then scales the
/// actual rendered/encoded pixels down (when `cell_scaling`, i.e. kitty) so the
/// encoded window stays within [`MAX_DISPLAY_PIXELS`] — the terminal upscales the
/// result to the cell footprint. The footprint itself is derived from the logical
/// window, so it always fills the same screen area.
fn plan_view(
    aspect: f32,
    vp: &Viewport,
    fit: Fit,
    cx: f32,
    cy: f32,
    cell_scaling: bool,
    max_display_pixels: Option<u64>,
) -> ViewPlan {
    let aspect = aspect.max(0.01);
    let fit_page_w = (vp.width_px as f32).min(vp.height_px as f32 * aspect);
    let logical_w = (fit_page_w * logical_zoom(fit, vp, fit_page_w)).max(1.0);
    let logical_h = (logical_w / aspect).max(1.0);

    // Visible window in logical pixels.
    let lwin_w = (vp.width_px as f32).min(logical_w);
    let lwin_h = (vp.height_px as f32).min(logical_h);

    // Cell footprint — independent of render resolution.
    let foot_cols = div_ceil_u16(lwin_w.round() as u32, vp.cell_w).min(vp.cols);
    let foot_rows = div_ceil_u16(lwin_h.round() as u32, vp.cell_h).min(vp.rows);

    // Resolution scale q: shrink the encoded window to the display budget (kitty
    // only), and bound the full render for memory.
    let mut q = 1.0f32;
    if cell_scaling
        && let Some(cap) = max_display_pixels
    {
        let win_px = (lwin_w * lwin_h).max(1.0);
        q = (cap as f32 / win_px).sqrt().min(1.0);
    }
    let full_px = (logical_w * logical_h).max(1.0);
    if (full_px * q * q) as u64 > MAX_FULL_PIXELS {
        q = ((MAX_FULL_PIXELS as f64 / full_px as f64).sqrt() as f32).min(q);
    }

    let mut render_w = (logical_w * q).round().max(1.0) as u32;
    let mut render_h = (logical_h * q).round().max(1.0) as u32;
    // Hard floor-cap so rounding can't push the product past the memory budget.
    if render_w as u64 * render_h as u64 > MAX_FULL_PIXELS {
        let s = (MAX_FULL_PIXELS as f64 / (render_w as u64 * render_h as u64) as f64).sqrt() as f32;
        render_w = ((render_w as f32 * s) as u32).max(1);
        render_h = ((render_h as f32 * s) as u32).max(1);
    }
    let win_w = ((lwin_w * q).round() as u32).clamp(1, render_w);
    let win_h = ((lwin_h * q).round() as u32).clamp(1, render_h);
    // cx/cy are scroll positions in [0,1]: 0 = top/left edge, 1 = bottom/right.
    // (A direct mapping — not a centered one — so there's no dead zone at edges.)
    let max_x = (render_w - win_w) as f32;
    let max_y = (render_h - win_h) as f32;
    let win_x = (cx.clamp(0.0, 1.0) * max_x).round() as u32;
    let win_y = (cy.clamp(0.0, 1.0) * max_y).round() as u32;

    ViewPlan {
        render_w,
        render_h,
        win_x,
        win_y,
        win_w,
        win_h,
        foot_cols,
        foot_rows,
    }
}

fn div_ceil_u16(value: u32, cell: u16) -> u16 {
    if cell == 0 {
        return 1;
    }
    value.div_ceil(cell as u32).max(1).min(u16::MAX as u32) as u16
}

/// Erase `n` cells starting at `(row, col)` (0-based) without disturbing the rest
/// of the row — `CSI row;col H` then `CSI n X` (erase character).
fn erase_cells(out: &mut impl Write, row: u16, col: u16, n: u16) -> Result<()> {
    write!(out, "\x1b[{};{}H\x1b[{}X", row as u32 + 1, col as u32 + 1, n)?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Truncate *or* pad `s` to exactly `max` columns, so a colored line covers the
/// whole row — an unpadded status bar would stop mid-row, leaving the rest of the
/// bar in the terminal's own background.
fn fit_line(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n >= max {
        return s.chars().take(max).collect();
    }
    let mut line = String::with_capacity(s.len() + (max - n));
    line.push_str(s);
    line.extend(std::iter::repeat_n(' ', max - n));
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(w: u32, h: u32) -> Viewport {
        Viewport {
            cols: (w / 10) as u16,
            rows: (h / 20) as u16,
            width_px: w,
            height_px: h,
            cell_w: 10,
            cell_h: 20,
        }
    }

    const ASPECT: f32 = 0.75;

    // Geometry tests use cell_scaling=false so render pixels stay predictable
    // (no display-resolution reduction).

    #[test]
    fn fit_page_shows_whole_page_no_pan() {
        let v = vp(1000, 800);
        let p = plan_view(ASPECT, &v, Fit::Page, 0.5, 0.0, false, None);
        assert_eq!((p.render_w, p.render_h), (600, 800));
        assert_eq!((p.win_x, p.win_y), (0, 0));
        assert_eq!((p.win_w, p.win_h), (600, 800));
        assert_eq!((p.foot_cols, p.foot_rows), (60, 40));
    }

    #[test]
    fn rotating_a_page_90_degrees_plans_as_landscape() {
        // A 90°/270° rotation feeds plan_view the inverted aspect (what
        // `display_aspect` computes). A portrait page (0.75) then plans like a
        // landscape one: width-bound instead of height-bound, with swapped dims.
        let v = vp(1000, 800);
        let upright = plan_view(ASPECT, &v, Fit::Page, 0.5, 0.0, false, None);
        let rotated = plan_view(1.0 / ASPECT, &v, Fit::Page, 0.5, 0.0, false, None);
        assert_eq!((upright.render_w, upright.render_h), (600, 800)); // height-bound
        assert_eq!(rotated.render_w, 1000); // now fills width
        assert!(rotated.render_h < rotated.render_w); // landscape footprint
    }

    #[test]
    fn fit_width_fills_width_and_scrolls_vertically() {
        let v = vp(1000, 800);
        let top = plan_view(ASPECT, &v, Fit::Width, 0.5, 0.0, false, None);
        assert_eq!(top.render_w, 1000);
        assert!(top.render_h > v.height_px);
        assert_eq!((top.win_x, top.win_y), (0, 0));
        let bottom = plan_view(ASPECT, &v, Fit::Width, 0.5, 1.0, false, None);
        assert_eq!(bottom.win_y, top.render_h - top.win_h);
    }

    #[test]
    fn zoom_makes_render_bigger_and_pannable() {
        let v = vp(1000, 800);
        let p = plan_view(ASPECT, &v, Fit::Zoom(2.0), 0.5, 0.5, false, None);
        assert_eq!(p.render_w, 1200);
        assert!(p.win_w < p.render_w);
        assert!(p.win_x > 0 && p.win_x <= p.render_w - p.win_w);
    }

    #[test]
    fn extreme_zoom_is_capped_for_memory() {
        let v = vp(2000, 1600);
        let p = plan_view(ASPECT, &v, Fit::Zoom(ZOOM_MAX), 0.5, 0.5, false, None);
        assert!(p.render_w as u64 * p.render_h as u64 <= MAX_FULL_PIXELS);
    }

    #[test]
    fn cell_scaling_caps_encoded_window_but_keeps_footprint() {
        // Hi-dpi 6 MP viewport. With cell scaling, the encoded window is capped
        // to the display budget while the footprint still fills the screen width.
        let v = vp(3000, 2000);
        let cap = Sharpness::Medium.max_display_pixels();
        let p = plan_view(ASPECT, &v, Fit::Width, 0.5, 0.0, true, cap);
        let win_px = p.win_w as u64 * p.win_h as u64;
        assert!(
            win_px <= cap.unwrap() * 11 / 10,
            "encoded window {}x{} = {win_px} too large",
            p.win_w,
            p.win_h
        );
        assert_eq!(p.foot_cols, v.cols); // still fills the screen width
    }

    #[test]
    fn full_sharpness_does_not_downscale() {
        let v = vp(3000, 2000);
        // None cap (Full) -> render at native viewport resolution.
        let p = plan_view(ASPECT, &v, Fit::Width, 0.5, 0.0, true, None);
        assert_eq!(p.render_w, v.width_px);
    }

    #[test]
    fn status_line_spans_exactly_the_row() {
        // Short lines pad out (so the bar's background covers the whole row) and
        // long ones are cut — either way the row is exactly `cols` wide.
        assert_eq!(fit_line(" folio ", 10), " folio    ");
        assert_eq!(fit_line("abcdefghij", 4), "abcd");
        assert_eq!(fit_line("· rot 90°", 20).chars().count(), 20); // multi-byte
    }

    #[test]
    fn sharpness_tiers_step_and_saturate() {
        assert_eq!(Sharpness::Low.softer(), Sharpness::Low);
        assert_eq!(Sharpness::Low.sharper(), Sharpness::Medium);
        assert_eq!(Sharpness::Full.sharper(), Sharpness::Full);
        assert_eq!(Sharpness::parse("HIGH"), Some(Sharpness::High));
    }
}
