//! How each widget face draws, at every size. Coordinates are logical pixels inside the
//! widget. Five shapes:
//!   Mini   — the dock at one icon wide: one visual and a short value
//!   Dock   — the dock, wider: visual, value and caption side by side
//!   Small  — half a Launchpad column
//!   Medium — a full column, one row tall (the reference layout)
//!   Large  — a full column, two rows tall: the medium layout plus details
//! Most faces describe themselves once as a `Sum` (visual, value, caption, chart, meter)
//! and share the Mini/Dock/Small layouts; faces with their own look draw every shape.

use std::borrow::Cow;

use laminar_sys::feed::{Feed, FeedError};
use windows::Win32::Graphics::Imaging::{IWICBitmapSource, IWICImagingFactory};

use super::canvas::{alpha, hex, Align, Badge, Canvas, ChartOpts, Col, Font};
use super::style::{self, Type};
use super::catalog::*;
use super::{compact, dur, headline, hms, money, Ctx, Painted, WAct, DAYS, MONTHS};

const BLUE: Col = hex(0x0A84FF);
const GREEN: Col = hex(0x30D158);
const RED: Col = hex(0xFF453A);
const ORANGE: Col = hex(0xFF9F0A);
const PURPLE: Col = hex(0xBF5AF2);
const YELLOW: Col = hex(0xFFD60A);
const CLAUDE: Col = hex(0xD97757);
const WHITE: Col = (1.0, 1.0, 1.0, 1.0);

// Segoe Fluent Icons
const G_PLAY: &str = "\u{E768}";
const G_PAUSE: &str = "\u{E769}";
const G_PREV: &str = "\u{E892}";
const G_NEXT: &str = "\u{E893}";
const G_RESET: &str = "\u{E72C}";
const G_WIFI: &str = "\u{E701}";
const G_BT: &str = "\u{E702}";
const G_MOON: &str = "\u{E708}";
const G_SUN: &str = "\u{E706}";
const G_VOL: &str = "\u{E767}";
const G_MUTE: &str = "\u{E74F}";
const G_MIC: &str = "\u{E720}";
const G_UP: &str = "\u{E70E}";
const G_BELL: &str = "\u{EA8F}";
const G_DESKTOP: &str = "\u{E7F4}";
const G_SEARCH: &str = "\u{E721}";
const G_RUN: &str = "\u{E756}";
const G_TRASH: &str = "\u{E74D}";
const G_NOTE: &str = "\u{E70B}";
const G_CHAT: &str = "\u{E8BD}";
const G_KEYBOARD: &str = "\u{E765}";
const G_MUSIC: &str = "\u{E8D6}";
const G_CLOCK: &str = "\u{E823}";
const G_PLUG: &str = "\u{E83E}";
const G_COFFEE: &str = "\u{EC32}";
const G_DND: &str = "\u{E7ED}";
const G_SETTINGS: &str = "\u{E713}";
const G_UPDATE: &str = "\u{E895}";
const G_SNIP: &str = "\u{E7C5}";
const G_LOCK: &str = "\u{E72E}";
const G_CHEV: &str = "\u{E76C}";
const G_DRIVE: &str = "\u{EDA2}";
const G_CAL: &str = "\u{E787}";
const G_SCREEN: &str = "\u{E7F4}";
const G_IMAGE: &str = "\u{E91B}";
const G_AI: &str = "\u{E99A}";
const G_WARN: &str = "\u{E7BA}";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Mini,
    Dock,
    Small,
    Medium,
    Large,
}

fn shape_of(c: &Canvas, dense: bool) -> Shape {
    if dense {
        if c.w < c.h * 1.5 {
            Shape::Mini
        } else {
            Shape::Dock
        }
    } else if c.w < 180.0 {
        Shape::Small
    } else if c.h > 150.0 {
        Shape::Large
    } else {
        Shape::Medium
    }
}

// ------------------------------------------------------------------ shared pieces

/// Data for a connector face: its feed, or why there isn't one.
enum Got<'a> {
    Feed(Cow<'a, Feed>),
    Setup(String),
    Loading,
    Error(String),
}

fn got<'a>(x: &'a Ctx, conn: &str) -> Got<'a> {
    match (x.hub.feed(conn), x.hub.err(conn)) {
        (Some(f), None) => Got::Feed(Cow::Borrowed(f)),
        _ if x.sample => Got::Feed(Cow::Owned(super::hub::sample(conn))),
        (_, Some(FeedError::NeedsSetup(m))) | (_, Some(FeedError::Unauthorized(m))) => Got::Setup(m.clone()),
        (Some(f), Some(_)) => Got::Feed(Cow::Borrowed(f)),
        (None, Some(FeedError::Failed(m))) => Got::Error(m.clone()),
        (None, None) => Got::Loading,
    }
}

/// The card: fill and a 1 px inner edge, with the dock or Launchpad corner radius.
fn bg(c: &Canvas, dense: bool) {
    let r = if dense { style::DOCK_R } else { style::CARD_R };
    c.rr(0.0, 0.0, c.w, c.h, r, c.card());
    c.rr_stroke(0.0, 0.0, c.w, c.h, r, c.card_edge(), 1.0);
}

fn up_down(d: f64) -> Col {
    if d >= 0.0 {
        GREEN
    } else {
        RED
    }
}

// ------------------------------------------------------------------ formats (§2.2)

/// A change figure: "+12.4%" / "−3.1%" (U+2212 minus), no triangles.
fn fmt_delta_text(d: f64) -> String {
    if d.abs() < 0.05 {
        return "0.0%".into();
    }
    format!("{}{:.1}%", if d > 0.0 { "+" } else { "\u{2212}" }, d.abs())
}

/// A change figure and its colour: green up, red down, text2 for no change. Only use the
/// colour where "up" is a judgment (money, prices, followers).
fn fmt_delta(c: &Canvas, d: f64) -> (String, Col) {
    (fmt_delta_text(d), if d.abs() < 0.05 { c.text2() } else { up_down(d) })
}

/// "1.9M", "412K", "12.4K", "1,234".
#[allow(dead_code)]
fn fmt_compact(v: f64) -> String {
    compact(v)
}

/// The tightest form of a number: "5.3K", "17.5K", "412K", "1.9M", "83".
fn compact_tight(v: f64) -> String {
    let a = v.abs();
    let (n, unit) = if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        return format!("{}", v.round() as i64);
    };
    let s = if n.abs() >= 100.0 { format!("{n:.0}") } else { format!("{n:.1}") };
    format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
}

/// The compact form of a formatted value ("$5,336" → "$5.3K", "$83.46" → "$83"), keeping
/// its prefix and suffix. None when there is no shorter form.
fn compact_str(s: &str) -> Option<String> {
    let start = s.find(|ch: char| ch.is_ascii_digit())?;
    let end = s[start..].find(|ch: char| !(ch.is_ascii_digit() || ch == ',' || ch == '.')).map(|e| start + e).unwrap_or(s.len());
    let num = &s[start..end];
    let v: f64 = num.replace(',', "").parse().ok()?;
    let short = compact_tight(v);
    (short.len() < num.len()).then(|| format!("{}{short}{}", &s[..start], &s[end..]))
}

/// Money (§2.2): cents under $100, "$5,336" to $9,999, "$12.4K" from $10,000; shortened
/// further ("$5.3K") when it has more than `budget` characters.
#[allow(dead_code)]
fn fmt_money(v: f64, currency: Option<&str>, budget: usize) -> String {
    let full = money(v, currency);
    if full.chars().count() <= budget {
        return full;
    }
    compact_str(&full).unwrap_or(full)
}

/// A key value that must never be cut: the full string if it fits `max_w` at the role's
/// size, else its compact form, else that shrunk (down to the role's minimum).
/// Draw the result with a generous `max_w` so it is never ellipsized.
fn fit_value(c: &Canvas, s: &str, role: Type, max_w: f32) -> (String, f32) {
    let w = c.measure(s, role.size, role.weight, Font::Text).0;
    if w <= max_w || w <= 0.0 {
        return (s.to_string(), role.size);
    }
    let s2 = compact_str(s).unwrap_or_else(|| s.to_string());
    let w2 = c.measure(&s2, role.size, role.weight, Font::Text).0;
    if w2 <= max_w {
        return (s2, role.size);
    }
    let mut size = (role.size * max_w / w2).max(role.min);
    // Width isn't quite linear in size (hinting, optical sizes): step down until it fits.
    while size > role.min && c.measure(&s2, size, role.weight, Font::Text).0 > max_w {
        size = (size - 0.5).max(role.min);
    }
    (s2, size)
}

/// The largest size (from `start` down to `min`) at which `s` fits in `max_w`.
fn fit(c: &Canvas, s: &str, start: f32, min: f32, weight: u32, font: Font, max_w: f32) -> f32 {
    let w = c.measure(s, start, weight, font).0;
    if w <= max_w || w <= 0.0 {
        return start;
    }
    (start * max_w / w).max(min)
}

fn open_hit(out: &mut Painted, c: &Canvas, open: &str) {
    if !open.is_empty() {
        out.hits.insert(0, ((0.0, 0.0, c.w, c.h), WAct::Open(open.to_string())));
    }
}

// ------------------------------------------------------------------ template helpers (§3)
// Family layouts build on these so margins, type and spacing stay the same everywhere.

/// The third line of a Glance block: a footnote, or a meter.
#[allow(dead_code)]
enum Line {
    Text(String),
    Meter(f32, Col),
}

/// Header template (Medium/Large): badge 20 and title on one row, an optional trailing
/// value at the right. Returns where content starts (y 44).
#[allow(dead_code)]
fn header(c: &Canvas, badge: &Badge, title: &str, trailing: Option<(&str, Col)>) -> f32 {
    let m = style::M_CARD;
    let bs = style::BADGE_HEADER;
    c.badge(badge, m, m, bs);
    let mut right = c.w - m;
    if let Some((t, col)) = trailing {
        let (v, fs) = fit_value(c, t, style::VALUE, (c.w - 2.0 * m) * 0.5);
        let w = c.measure(&v, fs, style::VALUE.weight, Font::Text).0;
        c.text_base(&v, c.w - m, style::HEADER_BASE, fs, style::VALUE.weight, col, Align::Right, c.w);
        right -= w + style::SP[3];
    }
    let tx = m + bs + style::SP[3];
    c.text_base(title, tx, style::HEADER_BASE, style::TITLE.size, style::TITLE.weight, c.text1(), Align::Left, right - tx);
    style::CONTENT_TOP
}

/// Glance template (Medium, and the top of Large): hero value, caption, optional third
/// line or meter, badge 24 top-right.
#[allow(dead_code)]
fn glance(c: &Canvas, badge: &Badge, value: &str, caption: &str, third: Option<Line>) {
    let m = style::M_CARD;
    let bs = style::BADGE_GLANCE;
    c.badge(badge, c.w - m - bs, m, bs);
    let (v, fs) = fit_value(c, value, style::HERO, c.w - 2.0 * m - bs - style::SP[3]);
    c.text_cap(&v, m, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
    c.text_base(caption, m, 58.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
    match third {
        Some(Line::Text(t)) => {
            c.text_base(&t, m, 76.0, style::FOOT.size, style::FOOT.weight, c.text3(), Align::Left, c.w - 2.0 * m);
        }
        Some(Line::Meter(f, col)) => c.bar(m, 70.0, c.w - 2.0 * m, style::METER_H, f, col),
        None => {}
    }
}

/// A hairline across the card at `y`, inside the margins.
fn separator(c: &Canvas, y: f32) {
    c.line(style::M_CARD, y + 0.5, c.w - style::M_CARD, y + 0.5, 1.0, c.sep());
}

/// The Large detail region under a Glance block: (x, y, w, h), inside the margins.
fn detail_region(c: &Canvas) -> (f32, f32, f32, f32) {
    let m = style::M_CARD;
    (m, style::DETAIL_TOP, c.w - 2.0 * m, c.h - m - style::DETAIL_TOP)
}

/// List rows from `top`, one per `pitch`, at most `max`: label at the left, trailing
/// value right-aligned. Returns the bottom of the last row.
fn rows(c: &Canvas, items: &[(&str, &str)], top: f32, pitch: f32, max: usize) -> f32 {
    let small = c.w < 180.0;
    let m = if small { style::M_SMALL } else { style::M_CARD };
    let ls = if small { style::FOOT.size } else { style::BODY.size };
    let inner = c.w - 2.0 * m;
    let n = items.len().min(max);
    for (k, (label, value)) in items.iter().take(n).enumerate() {
        let base = top + k as f32 * pitch + (pitch + c.cap_h(ls)) / 2.0;
        let mut vw = 0.0;
        if !value.is_empty() {
            // The value gets what the label leaves, but never squeezes the label under 45%.
            let lw = c.measure(label, ls, style::BODY.weight, Font::Text).0;
            let vmax = (inner - lw - style::SP[3]).max(inner * 0.45);
            vw = c.measure(value, style::FOOT.size, 500, Font::Text).0.min(vmax) + style::SP[3];
            c.text_base(value, c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, vmax);
        }
        c.text_base(label, m, base, ls, style::BODY.weight, c.text1(), Align::Left, inner - vw);
    }
    top + n as f32 * pitch
}

/// Three equal columns of value (17/600) over a footnote label, from `top` (cap top).
#[allow(dead_code)]
fn stat_footer(c: &Canvas, items: [(&str, &str); 3], top: f32) {
    let m = style::M_CARD;
    let cw = (c.w - 2.0 * m) / 3.0;
    let base = top + c.cap_h(style::VALUE.size);
    for (i, (v, label)) in items.iter().enumerate() {
        let x = m + i as f32 * cw;
        let (v, fs) = fit_value(c, v, style::VALUE, cw - style::SP[3]);
        c.text_base(&v, x, base, fs, style::VALUE.weight, c.text1(), Align::Left, c.w);
        c.text_base(label, x, base + style::SP[6], style::FOOT.size, style::FOOT.weight, c.text2(), Align::Left, cw - style::SP[3]);
    }
}

/// A loading placeholder bar.
fn skeleton(c: &Canvas, x: f32, y: f32, w: f32, h: f32) {
    c.rr(x, y, w, h, (h / 2.0).min(6.0), c.track());
}

/// A fully round blue button with a white label; adds its hit area.
#[allow(clippy::too_many_arguments)]
fn pill_button(c: &Canvas, out: &mut Painted, x: f32, y: f32, w: f32, h: f32, label: &str, act: WAct) {
    c.rr(x, y, w, h, h / 2.0, c.blue());
    let fs = style::CAPTION.size;
    c.text_cap(label, x + w / 2.0, y + (h - c.cap_h(fs)) / 2.0, fs, 600, WHITE, Align::Center, w);
    out.hits.push(((x, y, w, h), act));
}

/// What a face shows at a glance.
#[derive(Clone)]
enum Vis {
    Badge(Badge),
    /// Progress ring: fraction, colour, text inside (may be empty).
    Ring(f32, Col, String),
    Glyph(&'static str, Col),
    Emoji(&'static str),
}

struct Sum {
    vis: Vis,
    value: String,
    caption: String,
    delta: Option<f64>,
    series: Vec<f64>,
    meter: Option<(f32, Col)>,
}

impl Sum {
    fn new(vis: Vis, value: impl Into<String>, caption: impl Into<String>) -> Self {
        Sum { vis, value: value.into(), caption: caption.into(), delta: None, series: Vec::new(), meter: None }
    }
    /// The trend's colour: up/down for a change, else the meter's (identity) colour, else blue.
    fn spark_col(&self) -> Col {
        match (self.delta, self.meter) {
            (Some(d), _) => up_down(d),
            (None, Some((_, col))) => col,
            _ => BLUE,
        }
    }
}

fn draw_vis(c: &Canvas, v: &Vis, x: f32, y: f32, size: f32) {
    match v {
        Vis::Badge(b) => c.badge(b, x, y, size),
        Vis::Ring(f, col, t) => {
            let r = size / 2.0;
            let th = (size * style::RING_T).max(3.0);
            c.ring(x + r, y + r, r - th / 2.0, th, *f, *col);
            // Under 30 px the number inside would be unreadable (the value is shown beside or below).
            if !t.is_empty() && size >= 30.0 {
                let fs = (size * 0.26).max(style::MINI_VALUE.min);
                if c.measure(t, fs, 600, Font::Text).0 <= size - 2.0 * th - 4.0 {
                    c.text_cap(t, x + r, y + r - c.cap_h(fs) / 2.0, fs, 600, c.text1(), Align::Center, size);
                }
            }
        }
        Vis::Glyph(g, col) => {
            c.rr(x, y, size, size, size * style::BADGE_R, c.plate());
            c.glyph(g, x + size / 2.0, y + size / 2.0, size * 0.5, *col);
        }
        Vis::Emoji(e) => {
            c.text(e, x + size / 2.0, y + size * 0.02, size * 0.8, 400, c.text1(), Align::Center, size * 1.4, Font::Text);
        }
    }
}

/// What a Dock L pill shows in its accessory slot.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    None,
    Spark,
    Meter,
    Delta,
}

/// Mini, Dock M/L and Small layouts shared by most faces (§3).
fn compact_sum(c: &Canvas, s: &Sum, sh: Shape) {
    let vcol = c.text1();
    match sh {
        Shape::Mini => {
            // Dock sizes follow the icon size; the spec is drawn at 54.
            let k = c.h / 54.0;
            if let Vis::Ring(f, col, t) = &s.vis {
                // Visual-led: the ring is the icon, its number inside.
                let v = style::BADGE_MINI_VIS * k;
                let t = if t.is_empty() { s.value.clone() } else { t.clone() };
                draw_vis(c, &Vis::Ring(*f, *col, t), (c.w - v) / 2.0, (c.h - v) / 2.0, v);
                return;
            }
            if s.value.is_empty() {
                let v = style::BADGE_MINI_VIS * k;
                draw_vis(c, &s.vis, (c.w - v) / 2.0, (c.h - v) / 2.0, v);
                return;
            }
            // Value-led: a small mark over one compact value.
            let v = style::BADGE_MINI * k;
            draw_vis(c, &s.vis, (c.w - v) / 2.0, 8.0 * k, v);
            let role = Type { size: style::MINI_VALUE.size * k, ..style::MINI_VALUE };
            let (val, fs) = fit_value(c, &s.value, role, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&val, c.w / 2.0, 45.0 * k, fs, role.weight, vcol, Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            draw_vis(c, &s.vis, m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            // Dock L (the wide pill) adds one accessory at the right; Dock M never does.
            let slot = if c.w < c.h * 3.4 {
                Slot::None
            } else if s.series.len() > 2 {
                Slot::Spark
            } else if s.meter.is_some() {
                Slot::Meter
            } else if s.delta.is_some() {
                Slot::Delta
            } else {
                Slot::None
            };
            let slot_w = (c.w * 0.34).round();
            let slot_x = c.w - m - slot_w;
            let right = if slot == Slot::None { c.w - m } else { slot_x - style::SP[4] * k };
            let col_meter = s.meter.filter(|_| slot != Slot::Meter);
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let (val, fs) = fit_value(c, &s.value, role, right - x0);
            // With a meter under the caption the three lines move up as one block, so the
            // caption keeps clear of the value (baseline 40 under a value at 30 touched it).
            let (val_base, cap_base, bar_y) = if col_meter.is_some() { (23.0, 36.0, 41.0) } else { (30.0, 44.0, 45.0) };
            let (val_base, cap_base, bar_y) = (val_base * k, cap_base * k, bar_y * k);
            c.text_base(&val, x0, val_base, fs, role.weight, vcol, Align::Left, c.w * 2.0);
            let cs = style::FOOT.size * k;
            // The change shares the caption line only when both fit whole; otherwise the
            // caption alone (Dock L shows the trend in its slot instead).
            let cap = short_caption(&s.caption);
            let both = s.delta.filter(|_| slot != Slot::Delta).map(|d| fmt_delta(c, d)).filter(|(dt, _)| {
                let gap = if cap.is_empty() { 0.0 } else { style::SP[2] * k };
                c.measure(&cap, cs, 400, Font::Text).0 + gap + c.measure(dt, cs, 600, Font::Text).0 <= right - x0
            });
            match both {
                Some((dt, dcol)) => {
                    let cw = if cap.is_empty() { 0.0 } else { c.text_base(&cap, x0, cap_base, cs, 400, c.text2(), Align::Left, right - x0) + style::SP[2] * k };
                    c.text_base(&dt, x0 + cw, cap_base, cs, 600, dcol, Align::Left, right - x0 - cw);
                }
                None => {
                    c.text_base(&s.caption, x0, cap_base, cs, 400, c.text2(), Align::Left, right - x0);
                }
            }
            if let Some((f, col)) = col_meter {
                c.bar(x0, bar_y, right - x0, 3.0 * k, f, col);
            }
            let (sy0, sy1) = (14.0 * k, 40.0 * k);
            match slot {
                Slot::Spark => c.area(slot_x, sy0, slot_w, sy1 - sy0, &s.series, s.spark_col(), true),
                Slot::Meter => {
                    if let Some((f, col)) = s.meter {
                        let pct = format!("{:.0}%", (f * 100.0).clamp(0.0, 999.0));
                        c.text_base(&pct, c.w - m, 23.0 * k, cs, 500, c.text2(), Align::Right, slot_w);
                        c.bar(slot_x, 29.0 * k, slot_w, style::METER_H_COMPACT * k, f, col);
                    }
                }
                Slot::Delta => {
                    if let Some(d) = s.delta {
                        let (dt, dcol) = fmt_delta(c, d);
                        c.text_base(&dt, c.w - m, 32.0 * k, style::TITLE.size * k, 600, dcol, Align::Right, slot_w);
                    }
                }
                Slot::None => {}
            }
        }
        _ => {
            // Small card, bottom-anchored: mark and accessory on top, value and caption below.
            let m = style::M_SMALL;
            let bs = style::BADGE_SMALL;
            draw_vis(c, &s.vis, m, m, bs);
            let (sx0, sx1) = (m + bs + style::SP[4], c.w - m);
            let mut sy0 = 16.0;
            if let Some(d) = s.delta {
                let (t, col) = fmt_delta(c, d);
                c.text_base(&t, sx1, 26.0, style::FOOT.size, 600, col, Align::Right, sx1 - sx0);
                sy0 = 30.0;
            }
            if s.series.len() > 2 {
                c.area(sx0, sy0, sx1 - sx0, 42.0 - sy0, &s.series, s.spark_col(), true);
            } else if let Some((f, col)) = s.meter {
                c.bar(sx0, if s.delta.is_some() { 36.0 } else { 27.0 }, sx1 - sx0, style::METER_H_COMPACT, f, col);
            }
            let (v, fs) = fit_value(c, &s.value, style::HERO_S, c.w - 2.0 * m);
            c.text_base(&v, m, 86.0, fs, style::HERO_S.weight, vcol, Align::Left, c.w * 2.0);
            c.text_base(&s.caption, m, 106.0, style::CAPTION.size, style::CAPTION.weight, c.text2(), Align::Left, c.w - 2.0 * m);
        }
    }
}

/// A caption short enough to share a dock pill with a change figure.
fn short_caption(cap: &str) -> String {
    let t = cap.trim();
    match t.strip_prefix("Last ") {
        Some(rest) if rest.ends_with(" days") => rest.to_string(),
        _ => t.to_string(),
    }
}

/// Large cards: the detail region under the medium layout (§3 Large): a history chart,
/// else up to the rows that fit, inside the margins.
fn details(c: &Canvas, f: &Feed, col: Col) {
    let (x, y, w, h) = detail_region(c);
    if f.series.len() > 2 {
        separator(c, style::SEP_Y_LARGE);
        c.chart(x, y, w, h, &f.series, col, ChartOpts { fill: true, dot: true, stroke: style::STROKE, smooth: f.series.len() >= 8 });
        return;
    }
    let items: Vec<(&str, &str)> = f.rows.iter().map(|r| (r.label.as_str(), r.value.as_str())).collect();
    if items.is_empty() {
        return;
    }
    separator(c, style::SEP_Y_LARGE);
    // Rows start a little above the region so the first one's capitals sit at its top.
    let top = y - style::SP[1];
    let fit_rows = ((c.h - style::M_CARD - top) / style::ROW_L).floor() as usize;
    rows(c, &items, top, style::ROW_L, fit_rows.min(style::ROWS_MAX_L));
}

/// A reason line: text2, starting with an orange warning glyph for errors.
#[allow(clippy::too_many_arguments)]
fn reason_line(c: &Canvas, s: &str, x: f32, base: f32, size: f32, err: bool, align: Align, max_w: f32) {
    let g = if err { 12.0 + style::SP[1] } else { 0.0 };
    let tw = c.measure(s, size, 400, Font::Text).0.min(max_w - g);
    let left = match align {
        Align::Left => x,
        Align::Center => x - (g + tw) / 2.0,
        Align::Right => x - g - tw,
    };
    if err {
        c.glyph(G_WARN, left + 6.0, base - c.cap_h(size) / 2.0, 12.0, ORANGE);
    }
    c.text_base(s, left + g, base, size, 400, c.text2(), Align::Left, max_w - g);
}

/// The whole-card fallback for connector faces that have no data (§7): a skeleton while
/// loading, the Connect layout when it needs setting up, the reason when it failed.
fn placeholder(c: &Canvas, out: &mut Painted, name: &str, badge: &Badge, conn: &'static str, g: &Got, sh: Shape) -> bool {
    let (loading, connect, reason) = match g {
        Got::Feed(_) => return false,
        Got::Loading => (true, false, String::new()),
        Got::Setup(m) => (false, true, m.clone()),
        Got::Error(m) => (false, false, m.clone()),
    };
    let err = !loading && !connect;
    if connect {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect(conn)));
    }
    match sh {
        Shape::Mini => {
            let k = c.h / 54.0;
            let bs = 30.0 * k;
            let (bx, by) = ((c.w - bs) / 2.0, (c.h - bs) / 2.0);
            c.badge(badge, bx, by, bs);
            if connect || err {
                let (dx, dy, r) = (bx + bs - 1.0 * k, by + 1.0 * k, 3.5 * k);
                c.circle(dx, dy, r + 1.5 * k, c.halo());
                c.circle(dx, dy, r, if connect { BLUE } else { ORANGE });
            }
        }
        Shape::Dock => {
            let k = c.h / 54.0;
            let m = style::M_DOCK * k;
            let bs = style::BADGE_DOCK * k;
            c.badge(badge, m, (c.h - bs) / 2.0, bs);
            let x0 = m + bs + style::SP[4] * k;
            let tw = c.w - m - x0;
            if loading {
                skeleton(c, x0, (c.h - 10.0 * k) / 2.0, (60.0 * k).min(tw), 10.0 * k);
            } else {
                c.text_base(name, x0, 30.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, tw);
                if connect {
                    c.text_base("Connect", x0, 44.0 * k, style::FOOT.size * k, 600, BLUE, Align::Left, tw);
                } else {
                    reason_line(c, &reason, x0, 44.0 * k, style::FOOT.size * k, err, Align::Left, tw);
                }
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            c.badge(badge, m, m, style::BADGE_SMALL);
            let inner = c.w - 2.0 * m;
            if loading {
                let ch = c.cap_h(style::HERO_S.size);
                skeleton(c, m, 86.0 - ch, inner * 0.6, ch);
                skeleton(c, m, 106.0 - 8.0, 40.0, 8.0);
            } else {
                // Name and reason sit above the pill on the bottom margin.
                let pill_y = c.h - m - 26.0;
                let (nb, rb) = if connect { (pill_y - 22.0, pill_y - 7.0) } else { (86.0, 106.0) };
                c.text_base(name, m, nb, style::TITLE.size, 600, c.text1(), Align::Left, inner);
                reason_line(c, &reason, m, rb, style::FOOT.size, err, Align::Left, inner);
                if connect {
                    pill_button(c, out, m, pill_y, inner, 26.0, "Connect", WAct::Connect(conn));
                }
            }
        }
        Shape::Medium | Shape::Large if loading => {
            let m = style::M_CARD;
            let bs = style::BADGE_GLANCE;
            c.badge(badge, c.w - m - bs, m, bs);
            skeleton(c, m, m, (c.w - 2.0 * m - bs - style::SP[3]) * 0.6, c.cap_h(style::HERO.size));
            skeleton(c, m, 58.0 - 8.0, 40.0, 8.0);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let (x, y, w, _) = detail_region(c);
                for (i, f) in [1.0, 0.75, 0.5].iter().enumerate() {
                    skeleton(c, x, y + 4.0 + i as f32 * style::ROW_L, w * f, 8.0);
                }
            }
        }
        Shape::Medium => {
            let m = style::M_CARD;
            let bs = style::BADGE_SMALL;
            c.badge(badge, m, m, bs);
            let x0 = m + bs + style::SP[5];
            c.text_base(name, x0, 29.0, 15.0, 600, c.text1(), Align::Left, c.w - m - x0);
            reason_line(c, &reason, x0, 46.0, style::CAPTION.size, err, Align::Left, c.w - m - x0);
            if connect {
                let (bw, bh) = (88.0, 28.0);
                pill_button(c, out, c.w - m - bw, c.h - m - bh, bw, bh, "Connect", WAct::Connect(conn));
            }
        }
        Shape::Large => {
            let bs = 48.0;
            c.badge(badge, (c.w - bs) / 2.0, 36.0, bs);
            c.text_base(name, c.w / 2.0, 108.0, 15.0, 600, c.text1(), Align::Center, c.w - 2.0 * style::M_CARD);
            reason_line(c, &reason, c.w / 2.0, 128.0, style::CAPTION.size, err, Align::Center, c.w - 2.0 * style::M_CARD);
            if connect {
                let (bw, bh) = (132.0, 30.0);
                pill_button(c, out, (c.w - bw) / 2.0, 150.0, bw, bh, "Connect", WAct::Connect(conn));
            }
        }
    }
    true
}


pub fn paint(c: &Canvas, def: &Def, x: &Ctx, dense: bool, wic: &IWICImagingFactory) -> Painted {
    let mut out = Painted::default();
    let sh = shape_of(c, dense);
    let custom_bg = matches!(def.face, Face::Weather(WeatherKind::Banner) | Face::Wallpaper | Face::Clock(ClockKind::DayPage | ClockKind::AnalogSquare) | Face::Media(MediaKind::Disc));
    if !custom_bg || dense {
        bg(c, dense);
    }
    match def.face {
        Face::Stat { conn, badge, open } => stat(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Chart { conn, badge, open } => chart(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Usage { conn, badge, open } => usage(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Heat { conn, badge, color, open } => heat(c, &mut out, def, x, conn, &badge, hex(color), open, sh),
        Face::List { conn, badge, open } => list(c, &mut out, def, x, conn, &badge, open, sh),
        Face::Watch { conn } => watch(c, &mut out, def, x, conn, sh),
        Face::Next => next_event(c, &mut out, def, x, sh),
        Face::AiAll => ai_all(c, &mut out, x, sh),
        Face::Goal => goal(c, &mut out, x, sh),
        Face::Wallpaper => wallpaper(c, &mut out, def, x, wic, sh),
        Face::Clock(k) => clock(c, &mut out, k, x, sh),
        Face::Weather(k) => weather(c, &mut out, k, x, sh),
        Face::Timer(k) => timer(c, &mut out, k, x, sh),
        Face::Media(k) => media(c, &mut out, k, x, wic, sh),
        Face::Sys(k) => sys(c, &mut out, k, x, sh),
        Face::Tool(k) => tool(c, &mut out, k, x, sh),
        Face::Progress(s) => progress(c, s, x, sh),
    }
    out
}

fn compactish(sh: Shape) -> bool {
    matches!(sh, Shape::Mini | Shape::Dock | Shape::Small)
}

// ------------------------------------------------------------------ connector faces

#[allow(clippy::too_many_arguments)]
fn stat(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let (value, caption) = if def.id == "github-streak" {
        (f.extra.get("streak").cloned().unwrap_or_else(|| "0".into()), "Day streak".to_string())
    } else {
        (headline(&f), if f.caption.is_empty() { def.name.to_string() } else { f.caption.clone() })
    };
    if compactish(sh) {
        let mut s = Sum::new(Vis::Badge(*badge), value, caption);
        s.delta = f.delta_pct;
        s.series = f.series.clone();
        compact_sum(c, &s, sh);
        return;
    }
    c.badge(badge, c.w - 16.0 - 34.0, 16.0, 34.0);
    c.num(&value, 16.0, 14.0, fit(c, &value, 32.0, 18.0, 600, Font::Display, c.w - 80.0), 600, c.text1());
    // Subscribers share the caption's line ("MRR · 812 subscribers"), not a floating corner.
    let caption = match f.extra.get("active_subscriptions") {
        Some(n) => format!("{caption} · {n} subscribers"),
        None => caption,
    };
    let cw = c.tw(&caption, 16.0, 58.0, 12.0, 400, c.text2(), c.w - 120.0);
    if let Some(d) = f.delta_pct {
        c.tw(&fmt_delta_text(d), 22.0 + cw, 58.0, 11.5, 600, up_down(d), 90.0);
    }
    // Mail: the newest subject under the count (Large lists it with the others instead).
    if conn == "gmail" && sh != Shape::Large {
        if let Some(r) = f.rows.first() {
            c.tw(&r.label, 16.0, 74.0, 11.0, 400, c.text3(), c.w - 32.0);
        }
    }
    if sh == Shape::Large {
        details(c, &f, f.delta_pct.map(up_down).unwrap_or(GREEN));
    } else if f.series.len() > 2 {
        c.area(c.w - 120.0, 64.0, 104.0, 20.0, &f.series, GREEN, false);
    }
}

#[allow(clippy::too_many_arguments)]
fn chart(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let col = match f.delta_pct {
        Some(d) if d < 0.0 => RED,
        _ => GREEN,
    };
    let title = match f.extra.get("symbol") {
        Some(s) => s.clone(),
        None => def.name.trim_end_matches(" Chart").to_string(),
    };
    let value = if f.unit == "tokens" { compact(f.value.unwrap_or(0.0)) } else { headline(&f) };
    if compactish(sh) {
        let mut s = Sum::new(Vis::Badge(*badge), value, title);
        s.delta = f.delta_pct;
        s.series = f.series.clone();
        compact_sum(c, &s, sh);
        return;
    }
    c.badge(badge, 14.0, 13.0, 20.0);
    c.tw(&title, 42.0, 14.0, 12.5, 600, c.text1(), c.w - 160.0);
    c.tr(&value, c.w - 14.0, 9.0, 16.0, 600, c.text1());
    match f.delta_pct {
        Some(d) => {
            c.tr(&fmt_delta_text(d), c.w - 14.0, 30.0, 10.5, 600, up_down(d));
        }
        None => {
            c.tr(&f.caption, c.w - 14.0, 30.0, 10.5, 400, c.text3());
        }
    }
    if f.series.len() > 1 {
        c.area(14.0, 48.0, c.w - 28.0, c.h - 60.0, &f.series, col, true);
    } else {
        c.tw("No chart data yet", 14.0, 60.0, 11.5, 400, c.text3(), c.w - 28.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn usage(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let tokens = f.unit == "tokens";
    let short = if tokens { compact(f.value.unwrap_or(0.0)) } else { headline(&f) };
    let (meter_label, frac) = if tokens {
        let w5 = f.extra.get("tokens_5h").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let wk = f.extra.get("tokens_week").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        // Share of the week's use in the last 5 hours (a sense of pace).
        (format!("5h window {} · week {}", compact(w5), compact(wk)), if wk > 0.0 { (w5 / wk) as f32 } else { 0.0 })
    } else {
        let lim = f.extra.get("total_credits").cloned().unwrap_or_default();
        (if lim.is_empty() { f.caption.clone() } else { format!("{}% of ${lim} used", (f.frac.unwrap_or(0.0) * 100.0).round() as i32) }, f.frac.unwrap_or(0.0))
    };
    if compactish(sh) {
        let mut s = Sum::new(Vis::Badge(*badge), short, if tokens { "tokens today".to_string() } else { f.caption.clone() });
        s.meter = Some((frac, CLAUDE));
        if sh == Shape::Small && f.series.len() > 2 {
            s.series = f.series.clone();
        }
        compact_sum(c, &s, sh);
        return;
    }
    let value = if tokens { format!("{short} tokens") } else { short };
    c.badge(badge, 16.0, 16.0, 36.0);
    c.tw(def.name, 64.0, 15.0, 12.5, 600, c.text2(), c.w - 80.0);
    c.num(&value, 64.0, 32.0, 19.0, 600, c.text1());
    c.bar(16.0, 70.0, c.w - 32.0, 6.0, frac, CLAUDE);
    c.tw(&meter_label, 16.0, 78.0, 10.0, 400, c.text3(), c.w - 32.0);
    if let Some(cost) = f.extra.get("cost_today_usd").and_then(|v| v.parse::<f64>().ok()) {
        c.tr(&format!("≈ {}", money(cost, None)), c.w - 16.0, 20.0, 11.0, 400, c.text3());
    }
    if sh == Shape::Large {
        details(c, &f, CLAUDE);
    }
}

#[allow(clippy::too_many_arguments)]
fn heat(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, color: Col, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    let open = match (conn, x.cfg.widget_settings.get(conn).and_then(|m| m.get("username"))) {
        ("github", Some(u)) => format!("https://github.com/{u}"),
        ("leetcode", Some(u)) => format!("https://leetcode.com/u/{u}"),
        _ => open.to_string(),
    };
    open_hit(out, c, &open);
    let streak = f.extra.get("streak").cloned();
    if sh == Shape::Mini {
        let s = Sum::new(Vis::Badge(*badge), streak.map(|s| format!("{s}d")).unwrap_or_else(|| headline(&f)), "");
        compact_sum(c, &s, sh);
        return;
    }
    let (gx, gy, gw, gh) = match sh {
        Shape::Dock => (8.0, 7.0, c.w - 16.0, c.h - 14.0),
        Shape::Small => {
            c.badge(badge, 12.0, 12.0, 18.0);
            c.tw(&headline(&f), 36.0, 12.0, 12.5, 600, c.text1(), c.w - 48.0);
            if let Some(s) = &streak {
                c.tw(&format!("{s}-day streak"), 12.0, c.h - 22.0, 10.5, 400, c.text2(), c.w - 24.0);
            }
            (12.0, 38.0, c.w - 24.0, c.h - 66.0)
        }
        _ => {
            c.badge(badge, 14.0, 12.0, 20.0);
            c.tw(def.name, 42.0, 14.0, 12.5, 600, c.text1(), c.w - 150.0);
            let st = streak.as_ref().map(|s| format!("{s}-day streak")).unwrap_or_default();
            c.tr(&format!("{}  {}", headline(&f), st), c.w - 14.0, 15.0, 11.5, 500, c.text2());
            let gh = if sh == Shape::Large { 100.0 } else { c.h - 50.0 };
            (14.0, 40.0, c.w - 28.0, gh)
        }
    };
    let cell_gap = if sh == Shape::Dock { 1.5 } else { 2.5 };
    let cell = ((gh - 6.0 * cell_gap) / 7.0).max(2.0);
    let cols = (((gw + cell_gap) / (cell + cell_gap)).floor() as usize).max(1);
    let n = f.grid.len();
    let today_row = x.now.weekday as usize;
    // The last column ends today; earlier days fill backwards.
    for col in 0..cols {
        for row in 0..7 {
            if row > today_row && col == cols - 1 {
                continue;
            }
            let back = (cols - 1 - col) * 7 + today_row;
            let idx = n as isize - 1 - (back as isize - row as isize);
            let v = if idx >= 0 && (idx as usize) < n { f.grid[idx as usize] } else { 0.0 };
            let fill = if v <= 0.0 { c.ink(0.08) } else { alpha(color, 0.3 + 0.7 * v) };
            c.rr(gx + col as f32 * (cell + cell_gap), gy + row as f32 * (cell + cell_gap), cell, cell, cell * 0.25, fill);
        }
    }
    if sh == Shape::Large {
        // Totals under the graph.
        let y = gy + gh + 14.0;
        let rows: Vec<_> = f.rows.iter().take(4).collect();
        let slot = (c.w - 28.0) / rows.len().max(1) as f32;
        for (k, r) in rows.iter().enumerate() {
            let cx = 14.0 + slot * k as f32;
            c.num(&r.value, cx, y, 18.0, 600, c.text1());
            c.tw(&r.label, cx, y + 24.0, 10.5, 400, c.text2(), slot - 6.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn list(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, badge: &Badge, open: &str, sh: Shape) {
    let g = got(x, conn);
    if placeholder(c, out, def.name, badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, open);
    let value = if conn == "calendar" { format!("{} today", f.value.unwrap_or(0.0)) } else { headline(&f) };
    if matches!(sh, Shape::Mini | Shape::Dock) {
        // A value and the label that belongs to it: the first row's own figure when it has
        // one ("42 Repositories", "12% Microsoft Edge"), not the headline beside a row name.
        let first = f.rows.first();
        let (v, cap) = match first {
            Some(r) if conn != "calendar" && conn != "recent_files" && !r.value.is_empty() && r.value.chars().count() <= 10 => (r.value.clone(), r.label.clone()),
            Some(r) => (value, r.label.clone()),
            None => (value, def.name.to_string()),
        };
        compact_sum(c, &Sum::new(Vis::Badge(*badge), v, cap), sh);
        return;
    }
    let small = sh == Shape::Small;
    let pad = if small { 12.0 } else { 14.0 };
    c.badge(badge, pad, 12.0, 22.0);
    if small {
        c.tr(&value, c.w - pad, 15.0, 11.5, 600, c.text2());
    } else {
        c.tw(def.name, 44.0, 15.0, 12.5, 600, c.text1(), c.w - 140.0);
        c.tr(&value, c.w - pad, 15.0, 12.0, 500, c.text2());
    }
    let top = 44.0;
    let rh = if c.h > 150.0 { 26.0 } else if small { 22.0 } else { 17.0 };
    let n = (((c.h - top - 6.0) / rh).floor() as usize).max(1);
    let rows: Vec<_> = f.rows.iter().take(n).collect();
    if rows.is_empty() {
        c.tw("Nothing to show", pad, top + 4.0, 11.5, 400, c.text3(), c.w - 2.0 * pad);
        return;
    }
    for (k, r) in rows.iter().enumerate() {
        let y = top + k as f32 * rh;
        let vmax = if small { (c.w - 2.0 * pad) * 0.4 } else { c.w * 0.45 };
        let vw = c.measure(&r.value, 11.0, 500, Font::Text).0.min(vmax);
        c.tw(&r.label, pad, y, 11.5, 400, c.text1(), c.w - 2.0 * pad - vw - 6.0);
        c.text(&r.value, c.w - pad, y, 11.0, 500, c.text2(), Align::Right, vmax, Font::Text);
        if let Some(fr) = r.frac.filter(|v| *v > 0.0 && rh >= 22.0) {
            c.bar(pad, y + 16.0, c.w - 2.0 * pad, 3.0, fr, if conn == "top_memory" { PURPLE } else { BLUE });
        }
        if conn == "recent_files" && !r.value.is_empty() {
            out.hits.push(((6.0, y - 3.0, c.w - 12.0, rh), WAct::File(r.value.clone())));
        }
    }
}

fn watch(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, conn: &'static str, sh: Shape) {
    let badge = super::canvas::gly("\u{E9D2}", 0x1F2A1F, 0x34C759);
    let g = got(x, conn);
    if placeholder(c, out, def.name, &badge, conn, &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    open_hit(out, c, "https://finance.yahoo.com");
    if matches!(sh, Shape::Mini | Shape::Dock) {
        if let Some(r) = f.rows.first() {
            let mut s = Sum::new(Vis::Badge(badge), if sh == Shape::Mini { r.label.clone() } else { r.value.clone() }, r.label.clone());
            s.delta = r.delta;
            compact_sum(c, &s, sh);
        }
        return;
    }
    let small = sh == Shape::Small;
    let n = f.rows.len().min(if c.h > 150.0 { 6 } else { 4 }).max(1);
    let rh = (c.h - 16.0) / n as f32;
    for (k, r) in f.rows.iter().take(n).enumerate() {
        let y = 8.0 + k as f32 * rh + (rh - 16.0) / 2.0;
        let d = r.delta.unwrap_or(0.0);
        let pw = if small { 54.0 } else { 66.0 };
        c.tw(&r.label, 14.0, y, 12.0, 600, c.text1(), if small { c.w - pw - 30.0 } else { 80.0 });
        if !small {
            c.text(&r.value, c.w * 0.62, y, 12.5, 500, c.text1(), Align::Right, 120.0, Font::Text);
        }
        c.rr(c.w - 14.0 - pw, y - 1.0, pw, 18.0, 5.0, alpha(up_down(d), 0.9));
        c.tc(&format!("{}{:.2}%", if d >= 0.0 { "+" } else { "" }, d), c.w - 14.0 - pw / 2.0, y + 1.0, 10.5, 600, WHITE, pw);
    }
}

fn next_event(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, sh: Shape) {
    let badge = super::canvas::gly(G_CAL, 0xFF3B30, 0xFFFFFF);
    let g = got(x, "calendar");
    if placeholder(c, out, def.name, &badge, "calendar", &g, sh) {
        return;
    }
    let Got::Feed(f) = g else { return };
    let title = f.extra.get("next_title").cloned().unwrap_or_else(|| "Nothing scheduled".into());
    let when = f.extra.get("next_time").cloned().unwrap_or_default();
    let mins = f.extra.get("next_minutes").and_then(|m| m.parse::<i64>().ok());
    let soon = match mins {
        Some(m) if m <= 0 => "Now".to_string(),
        Some(m) if m < 60 => format!("in {m} min"),
        Some(m) if m < 24 * 60 => format!("in {} h {} min", m / 60, m % 60),
        _ => String::new(),
    };
    match sh {
        Shape::Mini => {
            let short = match mins {
                Some(m) if (0..60).contains(&m) => format!("{m}m"),
                Some(m) if (0..24 * 60).contains(&m) => format!("{}h", m / 60),
                _ => "—".into(),
            };
            compact_sum(c, &Sum::new(Vis::Badge(badge), short, ""), sh);
        }
        Shape::Dock => compact_sum(c, &Sum::new(Vis::Badge(badge), title, if soon.is_empty() { when } else { soon }), sh),
        Shape::Small => {
            c.badge(&badge, 14.0, 14.0, 26.0);
            c.tw("Up next", 48.0, 20.0, 10.5, 600, RED, c.w - 60.0);
            c.text(&title, 14.0, 48.0, 13.5, 600, c.text1(), Align::Left, c.w - 28.0, Font::Text);
            c.tw(&when, 14.0, 70.0, 11.0, 400, c.text2(), c.w - 28.0);
            if !soon.is_empty() {
                c.tw(&soon, 14.0, c.h - 24.0, 11.0, 600, BLUE, c.w - 28.0);
            }
        }
        _ => {
            c.badge(&badge, 16.0, 16.0, 34.0);
            c.tw("Up next", 62.0, 16.0, 11.0, 600, RED, 100.0);
            c.tw(&title, 62.0, 32.0, 15.0, 600, c.text1(), c.w - 78.0);
            c.tw(&when, 62.0, 56.0, 11.5, 400, c.text2(), c.w - 160.0);
            if !soon.is_empty() {
                let pw = c.measure(&soon, 11.0, 600, Font::Text).0 + 16.0;
                c.rr(c.w - 14.0 - pw, 54.0, pw, 20.0, 10.0, alpha(BLUE, 0.22));
                c.tc(&soon, c.w - 14.0 - pw / 2.0, 56.0, 11.0, 600, BLUE, pw);
            }
            if sh == Shape::Large {
                let mut f2 = f.clone().into_owned();
                f2.rows.retain(|r| r.label != title);
                details(c, &f2, BLUE);
            }
        }
    }
}

fn ai_all(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let src: [(&'static str, &str, Col); 6] = [
        ("claude_code", "Claude Code", CLAUDE),
        ("codex", "Codex", hex(0x9AA0A6)),
        ("anthropic", "Claude API", CLAUDE),
        ("openai", "OpenAI API", hex(0x10A37F)),
        ("openrouter", "OpenRouter", hex(0x6467F2)),
        ("cursor", "Cursor", hex(0xBBBBBB)),
    ];
    let mut rows: Vec<(&str, String, f32, Col)> = Vec::new();
    for (conn, name, col) in src {
        let f = match (x.hub.feed(conn), x.hub.err(conn)) {
            (Some(f), None) => Cow::Borrowed(f),
            _ if x.sample && (conn == "claude_code" || conn == "openai" || conn == "codex") => Cow::Owned(super::hub::sample(conn)),
            _ => continue,
        };
        let v = if f.unit == "tokens" { format!("{} today", compact(f.value.unwrap_or(0.0))) } else { headline(&f) };
        rows.push((name, v, f.frac.unwrap_or(0.0), col));
    }
    let badge = super::canvas::gly(G_AI, 0x2C2C34, 0xD97757);
    if matches!(sh, Shape::Mini | Shape::Dock) {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("anthropic")));
        let s = match rows.first() {
            Some(r) => Sum::new(Vis::Badge(badge), r.1.split(' ').next().unwrap_or("").to_string(), r.0),
            None => Sum::new(Vis::Badge(badge), if sh == Shape::Mini { "" } else { "AI Usage" }, "Click to connect"),
        };
        compact_sum(c, &s, sh);
        return;
    }
    let small = sh == Shape::Small;
    c.tw("AI Usage", 14.0, 12.0, 12.5, 600, c.text2(), 140.0);
    if rows.is_empty() {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("anthropic")));
        c.text("Use Claude Code or Codex on this PC, or connect an API key.", 14.0, 40.0, 11.0, 400, c.text3(), Align::Left, c.w - 28.0, Font::Text);
        return;
    }
    let n = if c.h > 150.0 { 6 } else { 3 };
    let rh = if c.h > 150.0 || small { 28.0 } else { 20.0 };
    for (k, (name, v, frac, col)) in rows.iter().take(n).enumerate() {
        let y = 34.0 + k as f32 * rh;
        c.circle(18.0, y + 8.0, 4.0, *col);
        if small {
            c.tw(name, 28.0, y, 11.0, 500, c.text1(), c.w - 40.0);
            c.tw(v, 28.0, y + 13.0, 10.0, 400, c.text2(), c.w - 40.0);
        } else {
            c.tw(name, 28.0, y, 12.0, 500, c.text1(), 110.0);
            c.bar(140.0, y + 5.0, c.w - 140.0 - 96.0, 5.0, if *frac > 0.0 { *frac } else { 0.5 }, *col);
            c.tr(v, c.w - 14.0, y, 11.5, 500, c.text2());
        }
    }
}

fn goal(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let goal: f64 = x.cfg.widget_settings.get("revenue-goal").and_then(|m| m.get("goal")).and_then(|v| v.parse().ok()).unwrap_or(10_000.0);
    let mut total = 0.0;
    let mut n = 0;
    for conn in ["stripe", "lemonsqueezy", "paddle", "shopify", "gumroad", "polar"] {
        let f = match (x.hub.feed(conn), x.hub.err(conn)) {
            (Some(f), None) => Cow::Borrowed(f),
            _ if x.sample && conn == "stripe" => Cow::Owned(super::hub::sample(conn)),
            _ => continue,
        };
        total += f.value.unwrap_or(0.0);
        n += 1;
    }
    let frac = (total / goal) as f32;
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Connect("revenue-goal")));
    let pct = format!("{:.0}%", (frac * 100.0).min(999.0));
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let s = if sh == Shape::Mini { Sum::new(Vis::Ring(frac, GREEN, pct), "", "") } else { Sum::new(Vis::Ring(frac, GREEN, String::new()), money(total, Some("USD")), format!("{pct} of goal")) };
        compact_sum(c, &s, sh);
        return;
    }
    if sh == Shape::Small {
        let r = 30.0;
        c.ring(c.w / 2.0, 14.0 + r, r - 4.0, 7.0, frac, GREEN);
        c.tc(&pct, c.w / 2.0, 14.0 + r - 9.0, 13.0, 600, c.text1(), 60.0);
        c.tc(&money(total, Some("USD")), c.w / 2.0, 82.0, 15.0, 600, c.text1(), c.w - 16.0);
        c.tc(&format!("of {}", money(goal, Some("USD"))), c.w / 2.0, 100.0, 10.5, 400, c.text2(), c.w - 16.0);
        return;
    }
    let big = sh == Shape::Large;
    let r = if big { 56.0 } else { 30.0 };
    let (cx, cy) = (16.0 + r + 6.0, c.h / 2.0);
    c.ring(cx, cy, r, if big { 11.0 } else { 7.0 }, frac, GREEN);
    c.tc(&pct, cx, cy - if big { 14.0 } else { 9.0 }, if big { 20.0 } else { 13.0 }, 600, c.text1(), 2.0 * r);
    let tx = cx + r + 22.0;
    c.num(&money(total, Some("USD")), tx, cy - 30.0, 26.0, 600, c.text1());
    let sub = if n == 0 { "Connect a payment provider".to_string() } else { format!("of {} goal · last 30 days", money(goal, Some("USD"))) };
    c.text(&sub, tx, cy + 6.0, 11.5, 400, c.text2(), Align::Left, c.w - tx - 14.0, Font::Text);
}

fn wallpaper(c: &Canvas, out: &mut Painted, def: &Def, x: &Ctx, wic: &IWICImagingFactory, sh: Shape) {
    let badge = super::canvas::gly(G_IMAGE, 0x2C6BED, 0xFFFFFF);
    let g = got(x, "wallpaper");
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if let Got::Feed(f) = &g {
        let img = f.extra.get("image_path").and_then(|p| x.hub.wallpaper_img.as_ref().filter(|w| &w.0 == p).map(|w| w.1.clone()).or_else(|| load_image(wic, p)));
        let r = if dense { 12.0 } else { 18.0 };
        match img {
            Some(src) => c.image(&src, 0.0, 0.0, c.w, c.h, r),
            None => c.rr_grad(0.0, 0.0, c.w, c.h, r, hex(0x5B8DEF), hex(0x1C3D7A)),
        }
        if !dense {
            c.rr_grad(0.0, c.h * 0.45, c.w, c.h * 0.55, r, (0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.55));
            let title = f.extra.get("title").cloned().or_else(|| f.text.clone()).unwrap_or_else(|| "Image of the day".into());
            if sh == Shape::Small {
                c.tw(&title, 12.0, c.h - 24.0, 11.5, 600, WHITE, c.w - 24.0);
            } else {
                c.tw(&title, 14.0, c.h - 30.0, 12.5, 600, WHITE, c.w - 130.0);
                let (bw, bh) = (104.0, 24.0);
                c.rr(c.w - bw - 12.0, c.h - bh - 10.0, bw, bh, 12.0, (1.0, 1.0, 1.0, 0.92));
                c.tc("Set wallpaper", c.w - 12.0 - bw / 2.0, c.h - bh - 6.0, 11.0, 600, (0.1, 0.1, 0.12, 1.0), bw);
            }
        }
        if f.extra.contains_key("image_path") && !x.sample {
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::SetWallpaper));
        }
        return;
    }
    bg(c, dense);
    placeholder(c, out, def.name, &badge, "wallpaper", &g, sh);
}

/// Decode an image file, scaled down for widget use.
pub fn load_image(wic: &IWICImagingFactory, path: &str) -> Option<IWICBitmapSource> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    unsafe {
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let scaler = wic.CreateBitmapScaler().ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        let k = (640.0 / w.max(1) as f32).min(1.0);
        scaler.Initialize(&frame, ((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1), WICBitmapInterpolationModeFant).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let bmp = wic.CreateBitmapFromSource(&conv, WICBitmapCacheOnLoad).ok()?;
        bmp.cast().ok()
    }
}

pub fn load_image_bytes(wic: &IWICImagingFactory, bytes: &[u8]) -> Option<IWICBitmapSource> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::*;
    unsafe {
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let bmp = wic.CreateBitmapFromSource(&conv, WICBitmapCacheOnLoad).ok()?;
        bmp.cast().ok()
    }
}

// ------------------------------------------------------------------ clocks

fn h12(h: u32) -> u32 {
    if h % 12 == 0 {
        12
    } else {
        h % 12
    }
}
fn ampm(h: u32) -> &'static str {
    if h < 12 {
        "AM"
    } else {
        "PM"
    }
}

#[allow(clippy::too_many_arguments)]
fn dial(c: &Canvas, cx: f32, cy: f32, r: f32, h: u32, m: u32, s: Option<u32>, dark: bool) {
    let (face, ink) = if dark { ((0.11, 0.11, 0.13, 1.0), (1.0, 1.0, 1.0, 0.95)) } else { ((0.97, 0.97, 0.98, 1.0), (0.08, 0.08, 0.1, 1.0)) };
    c.circle(cx, cy, r, face);
    for i in 0..12 {
        let a = (i as f32 * 30.0).to_radians();
        let (r0, r1) = if i % 3 == 0 { (r * 0.78, r * 0.9) } else { (r * 0.83, r * 0.9) };
        c.line(cx + r0 * a.sin(), cy - r0 * a.cos(), cx + r1 * a.sin(), cy - r1 * a.cos(), if i % 3 == 0 { r * 0.06 } else { r * 0.035 }, alpha(ink, 0.7));
    }
    let hand = |deg: f32, len: f32, t: f32, col: Col| {
        let a = deg.to_radians();
        c.line(cx - len * 0.15 * a.sin(), cy + len * 0.15 * a.cos(), cx + len * a.sin(), cy - len * a.cos(), t, col);
    };
    hand((h % 12) as f32 * 30.0 + m as f32 * 0.5, r * 0.5, r * 0.09, ink);
    hand(m as f32 * 6.0 + s.unwrap_or(0) as f32 * 0.1, r * 0.74, r * 0.065, ink);
    if let Some(s) = s {
        hand(s as f32 * 6.0, r * 0.8, r * 0.03, ORANGE);
    }
    c.circle(cx, cy, r * 0.06, ORANGE);
}

/// The analog face in a rounded square, with numerals at 12, 3, 6 and 9 when there's room.
#[allow(clippy::too_many_arguments)]
fn square_dial(c: &Canvas, x0: f32, y0: f32, s: f32, h: u32, m: u32, sec: u32) {
    let dark = !c.light;
    let (face, ink) = if dark { ((0.11, 0.11, 0.13, 1.0), (1.0, 1.0, 1.0, 0.95)) } else { ((0.97, 0.97, 0.98, 1.0), (0.08, 0.08, 0.1, 1.0)) };
    c.rr(x0, y0, s, s, s * 0.24, face);
    let (cx, cy) = (x0 + s / 2.0, y0 + s / 2.0);
    if s >= 60.0 {
        let fs = s * 0.13;
        for (num, dx, dy) in [("12", 0.0, -1.0), ("3", 1.0, 0.0), ("6", 0.0, 1.0), ("9", -1.0, 0.0)] {
            let r = s * 0.37;
            c.text(num, cx + dx * r, cy + dy * r - fs * 0.66, fs, 600, alpha(ink, 0.8), Align::Center, fs * 2.0, Font::Display);
        }
    } else {
        for i in 0..4 {
            let a = (i as f32 * 90.0).to_radians();
            let r = s * 0.4;
            c.circle(cx + r * a.sin(), cy - r * a.cos(), s * 0.025, alpha(ink, 0.7));
        }
    }
    let hand = |deg: f32, len: f32, t: f32, col: Col| {
        let a = deg.to_radians();
        c.line(cx - len * 0.12 * a.sin(), cy + len * 0.12 * a.cos(), cx + len * a.sin(), cy - len * a.cos(), t, col);
    };
    hand((h % 12) as f32 * 30.0 + m as f32 * 0.5, s * 0.22, s * 0.045, ink);
    hand(m as f32 * 6.0 + sec as f32 * 0.1, s * 0.33, s * 0.032, ink);
    hand(sec as f32 * 6.0, s * 0.36, s * 0.015, ORANGE);
    c.circle(cx, cy, s * 0.03, ORANGE);
}

#[allow(clippy::too_many_arguments)]
fn seg_digit(c: &Canvas, x: f32, y: f32, w: f32, h: f32, d: u32, on: Col, off: Col) {
    // a b c d e f g
    const SEGS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];
    let m = SEGS[d as usize % 10];
    let t = w * 0.16;
    let hh = h / 2.0;
    let rects = [
        (x + t * 0.6, y, w - t * 1.2, t),
        (x + w - t, y + t * 0.6, t, hh - t * 0.9),
        (x + w - t, y + hh + t * 0.3, t, hh - t * 0.9),
        (x + t * 0.6, y + h - t, w - t * 1.2, t),
        (x, y + hh + t * 0.3, t, hh - t * 0.9),
        (x, y + t * 0.6, t, hh - t * 0.9),
        (x + t * 0.6, y + hh - t / 2.0, w - t * 1.2, t),
    ];
    for (i, r) in rects.iter().enumerate() {
        c.rr(r.0, r.1, r.2, r.3, t / 2.0, if m & (1 << i) != 0 { on } else { off });
    }
}

fn month_grid(c: &Canvas, n: &super::Now, x0: f32, y0: f32, w: f32, h: f32) {
    let colw = w / 7.0;
    let first = n.first_weekday();
    let rows = (first + n.days_in_month()).div_ceil(7) as f32;
    let head = (h * 0.12).clamp(10.0, 20.0);
    let rh = (h - head) / rows;
    let fs = (rh * 0.55).min(colw * 0.5).clamp(7.0, 12.5);
    for (i, d) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        c.tc(d, x0 + colw * (i as f32 + 0.5), y0, fs * 0.85, 600, c.text3(), colw);
    }
    for day in 1..=n.days_in_month() {
        let idx = first + day - 1;
        let cx = x0 + colw * ((idx % 7) as f32 + 0.5);
        let cy = y0 + head + rh * (idx / 7) as f32 + rh / 2.0;
        let today = day == n.day;
        if today {
            c.circle(cx, cy, (rh * 0.46).min(colw * 0.46), RED);
        }
        c.tc(&day.to_string(), cx, cy - fs * 0.68, fs, if today { 700 } else { 400 }, if today { WHITE } else { c.text1() }, colw);
    }
}

/// This week as a row: day letters and dates, today marked.
fn week_row(c: &Canvas, n: &super::Now, x0: f32, y0: f32, w: f32, h: f32) {
    let colw = w / 7.0;
    let r = (h * 0.28).min(colw * 0.42).min(16.0);
    let fs = (r * 0.95).max(8.0);
    let ls = (h * 0.2).clamp(8.0, 11.0);
    for i in 0..7u32 {
        let off = i as i64 - n.weekday as i64;
        let d = n.day as i64 + off;
        let dim = super::days_in_month(n.year, n.month) as i64;
        let prev = super::days_in_month(n.year, if n.month == 1 { 12 } else { n.month - 1 }) as i64;
        let shown = if d < 1 { d + prev } else if d > dim { d - dim } else { d };
        let cx = x0 + colw * (i as f32 + 0.5);
        let today = off == 0;
        // Today in red, as in every calendar face (month grid, date tile, day page).
        c.tc(&DAYS[i as usize][..1], cx, y0, ls, 600, if today { RED } else { c.text3() }, colw);
        let cy = y0 + ls * 1.5 + (h - ls * 1.5) / 2.0;
        if today {
            c.circle(cx, cy, r, RED);
        }
        c.tc(&shown.to_string(), cx, cy - fs * 0.68, fs, if today { 700 } else { 500 }, if today { WHITE } else { c.text1() }, colw);
    }
}

/// A tear-off calendar page for today.
fn day_tile(c: &Canvas, n: &super::Now, sh: Shape) {
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    let s = c.h.min(c.w) - if dense { 6.0 } else { 16.0 };
    let (x0, y0) = ((c.w - s) / 2.0, (c.h - s) / 2.0);
    if !dense && c.w > c.h * 1.3 {
        bg(c, false);
    }
    c.rr(x0, y0, s, s, s * 0.2, (0.98, 0.98, 0.99, 1.0));
    let hh = s * 0.28;
    c.rr(x0, y0, s, hh + s * 0.2, s * 0.2, RED);
    c.rr(x0, y0 + hh, s, s * 0.2, 0.0, (0.98, 0.98, 0.99, 1.0));
    c.tc(&MONTHS[n.month as usize - 1][..3].to_uppercase(), x0 + s / 2.0, y0 + hh * 0.18, hh * 0.62, 700, WHITE, s);
    c.text(&n.day.to_string(), x0 + s / 2.0, y0 + hh + s * 0.02, s * 0.5, 600, (0.1, 0.1, 0.12, 1.0), Align::Center, s, Font::Display);
}

fn clock(c: &Canvas, out: &mut Painted, k: ClockKind, x: &Ctx, sh: Shape) {
    let n = x.now;
    let time = format!("{}:{:02}", h12(n.hour), n.minute);
    let mon = MONTHS[n.month as usize - 1];
    let day = DAYS[n.weekday as usize];
    let date_long = format!("{day}, {mon} {}", n.day);
    let date_short = format!("{} {} {}", &day[..3], &mon[..3], n.day);
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:dateandtime".into())));
    match k {
        ClockKind::Digital | ClockKind::ClockWeather => {
            let weather = if k == ClockKind::ClockWeather { x.weather.map(|w| (weather_emoji(w.code, w.is_day), format!("{:.0}°", d0(w.temp_c)))) } else { None };
            match sh {
                Shape::Mini => {
                    let fs = fit(c, &time, c.h * 0.36, 9.0, 600, Font::Display, c.w - 6.0);
                    c.text(&time, c.w / 2.0, c.h * 0.2, fs, 600, c.text1(), Align::Center, c.w, Font::Display);
                    let sub = weather.as_ref().map(|w| w.1.clone()).unwrap_or_else(|| ampm(n.hour).to_string());
                    c.tc(&sub, c.w / 2.0, c.h * 0.2 + fs * 1.22, c.h * 0.18, 600, c.text2(), c.w);
                }
                Shape::Dock => {
                    // Time on the left; the date (or weather) beside it when there's room, else under it.
                    let ts = (c.h * 0.42).max(15.0);
                    let tw = c.measure(&time, ts, 600, Font::Display).0;
                    let aw = c.measure(ampm(n.hour), ts * 0.4, 600, Font::Text).0;
                    let beside = c.w > 12.0 + tw + aw + 10.0 + c.h * 1.2;
                    let x0 = 12.0;
                    if beside {
                        let ty = (c.h - ts * 1.3) / 2.0;
                        c.text(&time, x0, ty, ts, 600, c.text1(), Align::Left, c.w, Font::Display);
                        c.tw(ampm(n.hour), x0 + tw + 3.0, ty + ts * 0.66, ts * 0.38, 600, c.text2(), 30.0);
                        let rx = x0 + tw + aw + 16.0;
                        c.line(rx - 7.0, c.h * 0.28, rx - 7.0, c.h * 0.72, 1.0, c.ink(0.15));
                        let cs = (c.h * 0.2).max(9.5);
                        match &weather {
                            Some((e, t)) => {
                                c.text(e, rx, c.h / 2.0 - cs * 1.05, cs * 1.4, 400, c.text1(), Align::Left, 40.0, Font::Text);
                                c.tw(t, rx + cs * 1.9, c.h / 2.0 - cs * 0.85, cs * 1.25, 600, c.text1(), c.w - rx - cs * 1.9 - 6.0);
                            }
                            None => {
                                c.tw(&day[..3].to_uppercase(), rx, c.h / 2.0 - cs * 1.3, cs, 700, RED, c.w - rx - 6.0);
                                c.tw(&format!("{} {}", &mon[..3], n.day), rx, c.h / 2.0 - 1.0, cs * 1.15, 600, c.text1(), c.w - rx - 6.0);
                            }
                        }
                    } else {
                        let ty = c.h * 0.12;
                        c.text(&time, x0, ty, ts, 600, c.text1(), Align::Left, c.w, Font::Display);
                        c.tw(ampm(n.hour), x0 + tw + 3.0, ty + ts * 0.42, ts * 0.42, 500, c.text2(), 30.0);
                        let cap = weather.map(|(e, t)| format!("{e} {t}")).unwrap_or(date_short);
                        c.tw(&cap, x0, ty + ts * 1.24, (c.h * 0.19).max(9.5), 400, c.text2(), c.w - x0 - 4.0);
                    }
                }
                Shape::Small => {
                    // AM/PM beside the minutes, not on a line of its own.
                    let aw = c.measure(ampm(n.hour), 11.0, 600, Font::Text).0;
                    let fs = fit(c, &time, 40.0, 20.0, 300, Font::Display, c.w - 28.0 - aw - 3.0);
                    let tw = c.num(&time, 14.0, 16.0, fs, 300, c.text1());
                    c.tw(ampm(n.hour), 14.0 + tw + 3.0, 16.0 + fs * 0.62, 11.0, 600, c.text2(), 40.0);
                    match weather {
                        Some((e, t)) => {
                            c.text(e, 14.0, c.h - 36.0, 18.0, 400, c.text1(), Align::Left, 30.0, Font::Text);
                            c.tw(&t, 40.0, c.h - 32.0, 14.0, 600, c.text1(), c.w - 54.0);
                        }
                        None => {
                            c.tw(&date_short, 14.0, c.h - 28.0, 11.5, 400, c.text2(), c.w - 28.0);
                        }
                    }
                }
                Shape::Medium if k == ClockKind::Digital => {
                    let tw = c.num(&time, 16.0, 6.0, 50.0, 300, c.text1());
                    c.num(&format!(":{:02}", n.second), 18.0 + tw, 30.0, 22.0, 300, c.text3());
                    c.tw(ampm(n.hour), 20.0 + tw + 34.0, 34.0, 15.0, 500, c.text2(), 40.0);
                    c.tw(&date_long, 18.0, 68.0, 13.0, 400, c.text2(), c.w - 36.0);
                }
                Shape::Medium => {
                    let tw = c.num(&time, 16.0, 12.0, 40.0, 400, c.text1());
                    c.tw(ampm(n.hour), 20.0 + tw, 34.0, 13.0, 500, c.text2(), 40.0);
                    c.tw(&date_long, 18.0, 66.0, 12.0, 400, c.text2(), c.w * 0.6);
                    if let Some((e, t)) = weather {
                        c.text(e, c.w - 60.0, 16.0, 28.0, 400, c.text1(), Align::Center, 60.0, Font::Text);
                        c.tr(&t, c.w - 16.0, 56.0, 20.0, 500, c.text1());
                    }
                }
                Shape::Large => {
                    // Big time with seconds, the date, and this week underneath.
                    if weather.is_some() {
                        // The weather takes the right side: no seconds, so AM/PM has room.
                        let tw = c.num(&time, 16.0, 8.0, 58.0, 200, c.text1());
                        c.tw(ampm(n.hour), 20.0 + tw, 42.0, 16.0, 500, c.text2(), 40.0);
                    } else {
                        let tw = c.num(&time, 16.0, 6.0, 64.0, 200, c.text1());
                        c.num(&format!(":{:02}", n.second), 20.0 + tw, 38.0, 26.0, 300, c.text3());
                        c.tw(ampm(n.hour), 20.0 + tw + 42.0, 42.0, 16.0, 500, c.text2(), 40.0);
                    }
                    c.tw(&date_long, 18.0, 86.0, 13.0, 500, c.text2(), c.w - 36.0);
                    if let Some((e, t)) = weather {
                        c.text(e, c.w - 56.0, 18.0, 30.0, 400, c.text1(), Align::Center, 60.0, Font::Text);
                        c.tr(&t, c.w - 18.0, 62.0, 20.0, 500, c.text1());
                    }
                    week_row(c, &n, 14.0, 124.0, c.w - 28.0, c.h - 136.0);
                }
            }
        }
        ClockKind::Flip => {
            let digits = [h12(n.hour) / 10, h12(n.hour) % 10, n.minute / 10, n.minute % 10];
            let tile_col = if c.light { (0.13, 0.13, 0.15, 1.0) } else { (0.06, 0.06, 0.075, 1.0) };
            let tile = |tx: f32, ty: f32, tw: f32, th: f32, d: u32| {
                c.rr(tx, ty, tw, th, tw * 0.16, tile_col);
                // Upper half a touch lighter, like a real flap.
                c.rr(tx, ty, tw, th / 2.0, tw * 0.16, (1.0, 1.0, 1.0, 0.035));
                let fs = (th * 0.66).min(tw * 1.08);
                c.text(&d.to_string(), tx + tw / 2.0, ty + th / 2.0 - fs * 0.68, fs, 600, (0.97, 0.97, 0.98, 1.0), Align::Center, tw, Font::Display);
                c.line(tx + 1.0, ty + th / 2.0, tx + tw - 1.0, ty + th / 2.0, 1.0, (0.0, 0.0, 0.0, 0.55));
            };
            if matches!(sh, Shape::Mini | Shape::Small) {
                // Square spaces: hours above minutes, each a pair of flaps.
                let pad = if sh == Shape::Mini { 5.0 } else { 14.0 };
                let gap = if sh == Shape::Mini { 2.0 } else { 6.0 };
                let tw = (c.w - 2.0 * pad - gap) / 2.0;
                let th = ((c.h - 2.0 * pad - gap) / 2.0).min(tw * 1.25);
                let top = (c.h - 2.0 * th - gap) / 2.0;
                for (i, d) in digits.iter().enumerate() {
                    let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                    tile(pad + col * (tw + gap), top + row * (th + gap), tw, th, *d);
                }
            } else {
                let dense = sh == Shape::Dock;
                let pad = if dense { 6.0 } else { 14.0 };
                let gap = if dense { 2.0 } else { 5.0 };
                let mid = if dense { 9.0 } else { 18.0 };
                let tw = (c.w - 2.0 * pad - 2.0 * gap - mid) / 4.0;
                let th = (c.h - 2.0 * pad).min(tw * 1.45);
                let top = (c.h - th) / 2.0;
                let mut tx = pad;
                for (i, d) in digits.iter().enumerate() {
                    if i == 2 {
                        // The colon between hours and minutes.
                        let cx = tx - gap + mid / 2.0;
                        let r = (mid * 0.16).max(1.5);
                        c.circle(cx, top + th * 0.36, r, c.text2());
                        c.circle(cx, top + th * 0.64, r, c.text2());
                        tx += mid - gap;
                    }
                    tile(tx, top, tw, th, *d);
                    tx += tw + if i == 1 { 0.0 } else { gap };
                }
            }
        }
        ClockKind::World => {
            let cities = super::world_cities(x.cfg);
            let n_c = match sh {
                Shape::Mini => 1,
                Shape::Dock | Shape::Small => 2,
                _ => 3,
            }
            .min(cities.len());
            let labels = !matches!(sh, Shape::Mini | Shape::Dock);
            let slot = c.w / n_c as f32;
            for (i, (label, key)) in cities.iter().take(n_c).enumerate() {
                let (h, m, s, _) = super::tz_now(key).unwrap_or((n.hour, n.minute, n.second, n.weekday));
                let cx = slot * (i as f32 + 0.5);
                let room = if labels { c.h - if sh == Shape::Large { 50.0 } else { 34.0 } } else { c.h - 6.0 };
                let r = (room / 2.0).min(slot / 2.0 - 6.0);
                let cy = if labels { 10.0 + r } else { c.h / 2.0 };
                dial(c, cx, cy, r, h, m, Some(s), !(6..18).contains(&h));
                if labels {
                    let name = if sh == Shape::Large { label.clone() } else { label.chars().take(3).collect::<String>().to_uppercase() };
                    c.tc(&name, cx, cy + r + 6.0, 11.0, 600, c.text2(), slot);
                    if sh == Shape::Large {
                        c.tc(&format!("{}:{:02} {}", h12(h), m, ampm(h)), cx, cy + r + 22.0, 11.0, 400, c.text3(), slot);
                    }
                }
            }
        }
        ClockKind::WeekStrip => match sh {
            Shape::Mini | Shape::Small => day_tile(c, &n, sh),
            Shape::Dock => week_row(c, &n, 6.0, 4.0, c.w - 12.0, c.h - 8.0),
            _ => {
                c.tw(&format!("{mon} {}", n.year), 14.0, 10.0, 12.5, 600, c.text1(), c.w - 30.0);
                week_row(c, &n, 14.0, 32.0, c.w - 28.0, if sh == Shape::Large { 70.0 } else { c.h - 40.0 });
                if sh == Shape::Large {
                    month_grid(c, &n, 14.0, 110.0, c.w - 28.0, c.h - 116.0);
                }
            }
        },
        ClockKind::WorldTime => {
            let cities = super::world_cities(x.cfg);
            let (label, key) = cities.into_iter().nth(2).unwrap_or(("Tokyo".into(), "Tokyo Standard Time".into()));
            let (h, m, _, wd) = super::tz_now(&key).unwrap_or((n.hour, n.minute, 0, n.weekday));
            let dname = if wd == n.weekday { "Today" } else if wd == (n.weekday + 1) % 7 { "Tomorrow" } else { "Yesterday" };
            let diff = {
                let mut d = (h as i32 * 60 + m as i32) - (n.hour as i32 * 60 + n.minute as i32);
                if wd == (n.weekday + 1) % 7 {
                    d += 24 * 60;
                } else if wd != n.weekday {
                    d -= 24 * 60;
                }
                d as f32 / 60.0
            };
            let t = format!("{}:{:02}", h12(h), m);
            let offset = format!("{dname}, {}{:.0}h", if diff >= 0.0 { "+" } else { "" }, diff);
            match sh {
                Shape::Mini | Shape::Dock => compact_sum(c, &Sum::new(Vis::Glyph(G_CLOCK, c.text1()), t, label.clone()), sh),
                Shape::Small => {
                    c.circle(20.0, 22.0, 5.0, if (6..18).contains(&h) { YELLOW } else { hex(0x8E8EFF) });
                    c.tw(&label, 32.0, 14.0, 12.5, 600, c.text1(), c.w - 44.0);
                    let fs = fit(c, &t, 36.0, 18.0, 300, Font::Display, c.w - 50.0);
                    c.num(&t, 14.0, 40.0, fs, 300, c.text1());
                    c.tw(ampm(h), 18.0 + c.measure(&t, fs, 300, Font::Display).0, 40.0 + fs * 0.62, 11.0, 600, c.text2(), 30.0);
                    c.tw(&offset, 14.0, c.h - 26.0, 11.0, 400, c.text2(), c.w - 28.0);
                }
                _ => {
                    c.circle(22.0, 24.0, 5.0, if (6..18).contains(&h) { YELLOW } else { hex(0x8E8EFF) });
                    c.tw(&label, 34.0, 15.0, 13.0, 600, c.text1(), c.w * 0.5);
                    c.tw(&offset, 16.0, 40.0, 11.5, 400, c.text2(), c.w * 0.5);
                    let tw = c.measure(ampm(h), 14.0, 500, Font::Text).0;
                    c.text(&t, c.w - 22.0 - tw, 14.0, 40.0, 300, c.text1(), Align::Right, c.w, Font::Display);
                    c.tr(ampm(h), c.w - 16.0, 36.0, 14.0, 500, c.text2());
                    if sh == Shape::Large {
                        // The other cities too.
                        for (i, (l, key)) in super::world_cities(x.cfg).into_iter().take(2).enumerate() {
                            let (hh, mm, _, _) = super::tz_now(&key).unwrap_or((n.hour, n.minute, 0, n.weekday));
                            let y = 110.0 + i as f32 * 42.0;
                            c.line(16.0, y - 8.0, c.w - 16.0, y - 8.0, 1.0, c.ink(0.08));
                            c.tw(&l, 16.0, y + 4.0, 13.0, 600, c.text1(), c.w * 0.5);
                            c.tr(&format!("{}:{:02} {}", h12(hh), mm, ampm(hh)), c.w - 16.0, y, 20.0, 300, c.text1());
                        }
                    }
                }
            }
        }
        ClockKind::Today => {
            let next = x.hub.feed("calendar").and_then(|f| f.extra.get("next_title").map(|t| format!("{t} · {}", f.extra.get("next_time").cloned().unwrap_or_default())));
            match sh {
                Shape::Mini => day_tile(c, &n, sh),
                Shape::Dock => {
                    c.tw(&day.to_uppercase(), 10.0, c.h * 0.12, (c.h * 0.18).max(9.0), 700, RED, c.w - 12.0);
                    let t = format!("{} {}", &mon[..3], n.day);
                    c.num(&t, 10.0, c.h * 0.36, fit(c, &t, c.h * 0.36, 9.0, 600, Font::Display, c.w - 12.0), 600, c.text1());
                }
                Shape::Small => {
                    c.tw(&day.to_uppercase(), 14.0, 14.0, 11.0, 700, RED, c.w - 28.0);
                    c.num(&n.day.to_string(), 12.0, 28.0, 44.0, 400, c.text1());
                    c.tw(mon, 14.0, 84.0, 12.0, 600, c.text1(), c.w - 28.0);
                    c.tw(next.as_deref().unwrap_or("No more events"), 14.0, c.h - 20.0, 10.0, 400, c.text3(), c.w - 28.0);
                }
                _ => {
                    c.tw(&day.to_uppercase(), 16.0, 12.0, 11.5, 700, RED, 160.0);
                    c.num(&format!("{mon} {}", n.day), 16.0, 28.0, 26.0, 600, c.text1());
                    c.tr(&format!("{} {}", time, ampm(n.hour)), c.w - 16.0, 14.0, 13.0, 500, c.text2());
                    c.tw(&next.unwrap_or_else(|| "No more events today".into()), 16.0, 68.0, 11.5, 400, c.text2(), c.w - 32.0);
                    if sh == Shape::Large {
                        month_grid(c, &n, 14.0, 100.0, c.w - 28.0, c.h - 108.0);
                    }
                }
            }
        }
        ClockKind::Date => match sh {
            Shape::Mini => day_tile(c, &n, sh),
            Shape::Dock => {
                c.tw(day, 12.0, c.h * 0.14, (c.h * 0.19).max(9.5), 400, c.text2(), c.w);
                let t = format!("{} {}", &mon[..3], n.day);
                c.num(&t, 12.0, c.h * 0.38, fit(c, &t, c.h * 0.36, 10.0, 600, Font::Display, c.w - 16.0), 600, c.text1());
            }
            Shape::Small => {
                c.tw(day, 14.0, 16.0, 12.0, 400, c.text2(), c.w - 28.0);
                c.num(&n.day.to_string(), 12.0, 32.0, 46.0, 500, c.text1());
                c.tw(mon, 14.0, 90.0, 13.0, 600, c.text1(), c.w - 28.0);
            }
            _ => {
                c.tw(day, 18.0, 18.0, 13.0, 400, c.text2(), c.w);
                c.num(&format!("{mon} {}", n.day), 18.0, 36.0, 30.0, 600, c.text1());
                if sh == Shape::Large {
                    month_grid(c, &n, 14.0, 92.0, c.w - 28.0, c.h - 100.0);
                }
            }
        },
        ClockKind::DayPage => day_tile(c, &n, sh),
        ClockKind::Analog => {
            let r = c.h.min(c.w) / 2.0 - if matches!(sh, Shape::Mini | Shape::Dock) { 3.0 } else { 10.0 };
            dial(c, c.w / 2.0, c.h / 2.0, r, n.hour, n.minute, Some(n.second), !c.light);
        }
        ClockKind::AnalogSquare => {
            let dense = matches!(sh, Shape::Mini | Shape::Dock);
            let s = c.h.min(c.w) - if dense { 4.0 } else { 16.0 };
            if !dense && c.w > c.h * 1.3 {
                bg(c, false);
            }
            square_dial(c, (c.w - s) / 2.0, (c.h - s) / 2.0, s, n.hour, n.minute, n.second);
        }
        ClockKind::Segment => {
            let on = RED;
            let off = c.ink(0.06);
            let dense = matches!(sh, Shape::Mini | Shape::Dock);
            let pad = if dense { 6.0 } else { 14.0 };
            let top = if dense || sh == Shape::Small { pad } else { 26.0 };
            // Four digits (0.55 wide, spaced 1.25) and a colon (0.45), fitted both ways.
            let units = 4.0 * 0.55 * 1.25 + 0.45 * 0.55;
            let dh = (c.h - top - pad).min((c.w - 2.0 * pad) / units);
            let dw = dh * 0.55;
            let colon = dw * 0.45;
            let total = 4.0 * dw + 3.0 * dw * 0.25 + colon;
            let mut sx = (c.w - total) / 2.0;
            let y0 = top + (c.h - top - pad - dh) / 2.0;
            if !dense && sh != Shape::Small {
                c.tw(ampm(n.hour), 16.0, 8.0, 10.5, 700, RED, 40.0);
                c.tr(&date_short.to_uppercase(), c.w - 16.0, 8.0, 10.5, 700, c.text3());
            }
            let d = [h12(n.hour) / 10, h12(n.hour) % 10, n.minute / 10, n.minute % 10];
            for (i, v) in d.iter().enumerate() {
                if i == 2 {
                    let blink = if n.second % 2 == 0 { on } else { off };
                    c.circle(sx + colon / 2.0 - dw * 0.12, y0 + dh * 0.3, dw * 0.09, blink);
                    c.circle(sx + colon / 2.0 - dw * 0.12, y0 + dh * 0.7, dw * 0.09, blink);
                    sx += colon;
                }
                if !(i == 0 && *v == 0) {
                    seg_digit(c, sx, y0, dw, dh, *v, on, off);
                } else {
                    seg_digit(c, sx, y0, dw, dh, 8, off, off);
                }
                sx += dw * 1.25;
            }
        }
        ClockKind::Month => match sh {
            Shape::Mini => day_tile(c, &n, sh),
            Shape::Dock => week_row(c, &n, 6.0, 4.0, c.w - 12.0, c.h - 8.0),
            Shape::Medium => {
                c.tw(mon, 16.0, 10.0, 13.0, 600, c.text1(), 160.0);
                c.tr(&n.year.to_string(), c.w - 16.0, 10.0, 13.0, 600, RED);
                week_row(c, &n, 14.0, 32.0, c.w - 28.0, c.h - 40.0);
            }
            _ => {
                let pad = if sh == Shape::Small { 10.0 } else { 14.0 };
                let hs = if sh == Shape::Small { 11.5 } else { 14.0 };
                c.tw(if sh == Shape::Small { &mon[..3] } else { mon }, pad + 2.0, 10.0, hs, 600, c.text1(), 160.0);
                c.tr(&n.year.to_string(), c.w - pad - 2.0, 10.0, hs, 600, RED);
                month_grid(c, &n, pad, 32.0, c.w - 2.0 * pad, c.h - 38.0);
            }
        },
    }
}

// ------------------------------------------------------------------ weather

pub fn weather_emoji(code: i32, day: bool) -> &'static str {
    match code {
        0 => {
            if day {
                "☀️"
            } else {
                "🌙"
            }
        }
        1 => {
            if day {
                "🌤️"
            } else {
                "🌙"
            }
        }
        2 => "⛅",
        3 => "☁️",
        45 | 48 => "🌫️",
        51..=57 | 80..=82 => "🌦️",
        61..=67 => "🌧️",
        71..=77 | 85 | 86 => "🌨️",
        95..=99 => "⛈️",
        _ => "🌡️",
    }
}

/// Rounded temperature without a "-0".
fn d0(t: f32) -> f32 {
    let r = t.round();
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

fn mins(hm: &str) -> f32 {
    let (h, m) = hm.split_once(':').unwrap_or(("0", "0"));
    h.parse::<f32>().unwrap_or(0.0) * 60.0 + m.parse::<f32>().unwrap_or(0.0)
}

fn weekday_of(ymd: &str) -> String {
    // Sakamoto's algorithm.
    let p: Vec<i32> = ymd.split('-').filter_map(|s| s.parse().ok()).collect();
    if p.len() != 3 {
        return String::new();
    }
    let (mut y, m, d) = (p[0], p[1], p[2]);
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    if m < 3 {
        y -= 1;
    }
    let wd = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d).rem_euclid(7);
    DAYS[wd as usize][..3].to_string()
}

/// Columns of hours or days: label, emoji, temperature.
fn forecast_cols(c: &Canvas, items: &[(String, &'static str, String)], x0: f32, y0: f32, w: f32, small_temp: bool) {
    let n = items.len().max(1);
    let colw = w / n as f32;
    for (i, (l, e, t)) in items.iter().enumerate() {
        let cx = x0 + colw * (i as f32 + 0.5);
        c.tc(l, cx, y0, 10.5, 500, c.text2(), colw);
        c.text(e, cx, y0 + 15.0, 16.0, 400, c.text1(), Align::Center, colw, Font::Text);
        c.tc(t, cx, y0 + 40.0, if small_temp { 10.0 } else { 12.0 }, 600, c.text1(), colw);
    }
}

type Day = (String, &'static str, String, String);

fn weather(c: &Canvas, out: &mut Painted, k: WeatherKind, x: &Ctx, sh: Shape) {
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(format!("https://www.msn.com/en-ca/weather/forecast/in-{}", x.cfg.weather_city.replace(' ', "-")))));
    let Some(w) = x.weather else {
        if k == WeatherKind::Banner && !matches!(sh, Shape::Mini | Shape::Dock) {
            bg(c, false);
        }
        compact_or_text(c, sh, Vis::Glyph(G_SUN, c.text3()), "—", &x.cfg.weather_city, "Fetching weather…");
        return;
    };
    let city = &x.cfg.weather_city;
    let temp = format!("{:.0}°", d0(w.temp_c));
    let emoji = weather_emoji(w.code, w.is_day);
    let hourly: Vec<(String, &'static str, String)> = w.hourly.iter().enumerate().map(|(i, (h, t, code))| (if i == 0 { "Now".to_string() } else { format!("{}{}", h12(*h as u32), if *h < 12 { "a" } else { "p" }) }, weather_emoji(*code, (6..19).contains(h)), format!("{:.0}°", d0(*t)))).collect();
    let daily: Vec<Day> = w.daily.iter().enumerate().map(|(i, (d, hi, lo, code))| (if i == 0 { "Today".to_string() } else { weekday_of(d) }, weather_emoji(*code, true), format!("{:.0}°", d0(*hi)), format!("{:.0}°", d0(*lo)))).collect();
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let s = if k == WeatherKind::Sun {
            // The next sunrise or sunset, not the weather.
            let now_m = (x.now.hour * 60 + x.now.minute) as f32;
            let (label, hm, e) = if now_m >= mins(&w.sunrise) && now_m < mins(&w.sunset) { ("Sunset", &w.sunset, "🌇") } else { ("Sunrise", &w.sunrise, "🌅") };
            let m = mins(hm) as u32;
            let t = format!("{}:{:02}", h12(m / 60), m % 60);
            let t = if sh == Shape::Dock { format!("{t} {}", ampm(m / 60)) } else { t };
            Sum::new(Vis::Emoji(e), t, if sh == Shape::Dock { label } else { "" })
        } else {
            Sum::new(Vis::Emoji(emoji), temp, if sh == Shape::Dock { w.description.clone() } else { String::new() })
        };
        compact_sum(c, &s, sh);
        return;
    }
    if sh == Shape::Small {
        match k {
            WeatherKind::Banner => {
                let (top, bottom) = if w.is_day { (hex(0x4A90E2), hex(0x2D6CC0)) } else { (hex(0x1B2550), hex(0x0E1430)) };
                c.rr_grad(0.0, 0.0, c.w, c.h, 18.0, top, bottom);
                c.tw(city, 14.0, 14.0, 12.0, 600, WHITE, c.w - 28.0);
                c.num(&temp, 14.0, 32.0, 34.0, 300, WHITE);
                c.text(emoji, c.w - 30.0, 36.0, 24.0, 400, WHITE, Align::Center, 40.0, Font::Text);
                c.tw(&w.description, 14.0, c.h - 26.0, 11.0, 500, (1.0, 1.0, 1.0, 0.85), c.w - 28.0);
            }
            WeatherKind::Hourly | WeatherKind::Week => {
                c.tw(city, 12.0, 10.0, 11.5, 600, c.text1(), c.w - 50.0);
                c.tr(&temp, c.w - 12.0, 10.0, 11.5, 500, c.text2());
                let items: Vec<(String, &'static str, String)> = if k == WeatherKind::Hourly {
                    hourly.iter().take(3).cloned().collect()
                } else {
                    daily.iter().take(3).map(|(l, e, hi, _)| (if l == "Today" { l.clone() } else { l.chars().take(3).collect() }, *e, hi.clone())).collect()
                };
                forecast_cols(c, &items, 6.0, 40.0, c.w - 12.0, false);
            }
            WeatherKind::Sun => sun_arc(c, w, x, true),
            _ => {
                c.tw(city, 14.0, 12.0, 12.0, 600, c.text1(), c.w - 28.0);
                c.text(emoji, 14.0, 34.0, 26.0, 400, c.text1(), Align::Left, 40.0, Font::Text);
                c.num(&temp, 52.0, 30.0, 32.0, 300, c.text1());
                c.tw(&w.description, 14.0, 78.0, 11.0, 400, c.text2(), c.w - 28.0);
                c.tw(&format!("H:{:.0}° L:{:.0}°", d0(w.high_c), d0(w.low_c)), 14.0, 94.0, 10.5, 400, c.text3(), c.w - 28.0);
            }
        }
        return;
    }
    let large = sh == Shape::Large;
    match k {
        WeatherKind::Now => {
            c.text(emoji, 46.0, 18.0, 40.0, 400, c.text1(), Align::Center, 80.0, Font::Text);
            c.num(&temp, 90.0, 10.0, 40.0, 300, c.text1());
            c.tw(&format!("H:{:.0}°  L:{:.0}°", d0(w.high_c), d0(w.low_c)), 92.0, 62.0, 12.0, 400, c.text2(), 140.0);
            c.tr(city, c.w - 16.0, 16.0, 12.5, 600, c.text1());
            c.tr(&w.description, c.w - 16.0, 36.0, 11.5, 400, c.text2());
        }
        WeatherKind::Detail => {
            c.text(emoji, 34.0, 16.0, 30.0, 400, c.text1(), Align::Center, 60.0, Font::Text);
            c.num(&temp, 62.0, 10.0, 34.0, 300, c.text1());
            c.tw(&w.description, 18.0, 62.0, 12.0, 400, c.text2(), c.w * 0.5);
            let rows = [("Feels like", format!("{:.0}°", d0(w.feels_c))), ("Humidity", format!("{:.0}%", w.humidity)), ("Wind", format!("{:.0} km/h", w.wind_kmh)), ("UV", format!("{:.0}", w.uv))];
            for (i, (l, v)) in rows.iter().enumerate() {
                let y = 12.0 + i as f32 * 18.0;
                c.tw(l, c.w * 0.55, y, 11.0, 400, c.text3(), 80.0);
                c.tr(v, c.w - 16.0, y, 11.5, 600, c.text1());
            }
        }
        WeatherKind::Hourly => {
            c.tw(city, 16.0, 10.0, 12.0, 600, c.text1(), c.w * 0.5);
            c.tr(&format!("{temp}  {}", w.description), c.w - 16.0, 10.0, 11.5, 400, c.text2());
            forecast_cols(c, &hourly.iter().take(6).cloned().collect::<Vec<_>>(), 12.0, 32.0, c.w - 24.0, false);
        }
        WeatherKind::Week if !large => {
            c.tw(city, 16.0, 10.0, 12.0, 600, c.text1(), c.w * 0.5);
            c.tr(&format!("{temp}  {}", w.description), c.w - 16.0, 10.0, 11.5, 400, c.text2());
            // Day, icon, then the high bright and the low dimmed, so pairs read apart.
            let days: Vec<&Day> = daily.iter().take(7).collect();
            let colw = (c.w - 24.0) / days.len().max(1) as f32;
            for (i, (l, e, hi, lo)) in days.iter().enumerate() {
                let cx = 12.0 + colw * (i as f32 + 0.5);
                c.tc(l, cx, 32.0, 10.5, 500, c.text2(), colw);
                c.text(e, cx, 47.0, 16.0, 400, c.text1(), Align::Center, colw, Font::Text);
                let (hw, lw) = (c.measure(hi, 10.5, 600, Font::Text).0, c.measure(lo, 10.0, 400, Font::Text).0);
                let x0 = cx - (hw + 2.0 + lw) / 2.0;
                c.tw(hi, x0, 72.0, 10.5, 600, c.text1(), hw + 2.0);
                c.tw(lo, x0 + hw + 2.0, 72.5, 10.0, 400, c.text3(), lw + 2.0);
            }
        }
        WeatherKind::Week => {
            c.tw(city, 16.0, 12.0, 12.0, 600, c.text1(), c.w * 0.5);
            c.tr(&format!("{temp}  {}", w.description), c.w - 16.0, 12.0, 11.5, 400, c.text2());
        }
        WeatherKind::Banner => {
            let (top, bottom) = if w.is_day { (hex(0x4A90E2), hex(0x2D6CC0)) } else { (hex(0x1B2550), hex(0x0E1430)) };
            c.rr_grad(0.0, 0.0, c.w, c.h, 18.0, top, bottom);
            c.tw(&w.description, 18.0, 18.0, 16.0, 600, WHITE, c.w * 0.55);
            c.tw(city, 18.0, 42.0, 12.0, 400, (1.0, 1.0, 1.0, 0.8), c.w * 0.55);
            c.tw(&format!("H:{:.0}°  L:{:.0}°", d0(w.high_c), d0(w.low_c)), 18.0, 64.0, 11.5, 400, (1.0, 1.0, 1.0, 0.8), c.w * 0.55);
            c.text(&temp, c.w - 70.0, 18.0, 44.0, 300, WHITE, Align::Right, c.w, Font::Display);
            c.text(emoji, c.w - 40.0, 44.0, 30.0, 400, WHITE, Align::Center, 60.0, Font::Text);
        }
        WeatherKind::Sun => sun_arc(c, w, x, false),
    }
    if large && k != WeatherKind::Sun {
        // The week underneath (for the weekly card, from the top).
        let start = if k == WeatherKind::Week { 40.0 } else { 108.0 };
        if k != WeatherKind::Week {
            c.line(14.0, start - 8.0, c.w - 14.0, start - 8.0, 1.0, if k == WeatherKind::Banner { (1.0, 1.0, 1.0, 0.2) } else { c.ink(0.08) });
        }
        let ink = if k == WeatherKind::Banner { WHITE } else { c.text1() };
        let n = (((c.h - start - 6.0) / 22.0).floor() as usize).min(daily.len());
        day_rows(c, &daily[..n], start, ink);
    }
}

/// Rows of days with a temperature range bar.
fn day_rows(c: &Canvas, days: &[Day], start: f32, ink: Col) {
    let t = |s: &str| s.trim_end_matches('°').parse::<f32>().unwrap_or(0.0);
    let (lo_t, hi_t) = days.iter().fold((f32::MAX, f32::MIN), |acc, d| (acc.0.min(t(&d.3)), acc.1.max(t(&d.2))));
    let span = (hi_t - lo_t).max(1.0);
    for (i, (l, e, hi, lo)) in days.iter().enumerate() {
        let y = start + i as f32 * 22.0;
        c.tw(l, 16.0, y, 11.5, 500, ink, 60.0);
        c.text(e, 86.0, y - 2.0, 14.0, 400, ink, Align::Center, 30.0, Font::Text);
        c.tr(lo, 132.0, y, 11.5, 400, alpha(ink, 0.6));
        let (bx, bw) = (142.0, c.w - 142.0 - 50.0);
        c.rr(bx, y + 6.0, bw, 4.0, 2.0, alpha(ink, 0.12));
        let a = bx + bw * (t(lo) - lo_t) / span;
        let b = bx + bw * (t(hi) - lo_t) / span;
        c.rr(a, y + 6.0, (b - a).max(4.0), 4.0, 2.0, ORANGE);
        c.tr(hi, c.w - 16.0, y, 11.5, 600, ink);
    }
}

fn sun_arc(c: &Canvas, w: &laminar_sys::weather::Weather, x: &Ctx, small: bool) {
    let (rise, set) = (mins(&w.sunrise), mins(&w.sunset));
    let now_m = (x.now.hour * 60 + x.now.minute) as f32;
    let frac = if set > rise { ((now_m - rise) / (set - rise)).clamp(0.0, 1.0) } else { 0.5 };
    let bottom = if c.h > 150.0 { c.h - 40.0 } else { c.h - 22.0 };
    let r = (c.w * 0.36).min(bottom - 30.0).min(if c.h > 150.0 { 110.0 } else { 56.0 });
    let (cx, cy) = (c.w / 2.0, bottom);
    c.arc(cx, cy, r, 2.0, 180.0, 360.0, c.ink(0.2));
    c.arc(cx, cy, r, 2.5, 180.0, 180.0 + 180.0 * frac, ORANGE);
    let a = (180.0 + 180.0 * frac).to_radians();
    c.circle(cx + r * a.cos(), cy + r * a.sin(), 6.0, YELLOW);
    c.line(12.0, cy, c.w - 12.0, cy, 1.0, c.ink(0.15));
    let fs = if small { 10.0 } else { 11.5 };
    c.tw(&format!("↑ {}", w.sunrise), 12.0, cy + 4.0, fs, 500, c.text2(), 80.0);
    c.tr(&format!("{} ↓", w.sunset), c.w - 12.0, cy + 4.0, fs, 500, c.text2());
    if !small {
        c.tw("Sunrise & Sunset", 16.0, 10.0, 11.5, 600, c.text2(), 140.0);
        let left = (set - now_m).max(0.0) as u64 * 60;
        c.tr(&if left > 0 { format!("{} of daylight left", dur(left)) } else { "After sunset".into() }, c.w - 16.0, 10.0, 11.0, 400, c.text3());
    }
}

/// For faces without data yet: the compact layout in the dock, a line of text in cards.
fn compact_or_text(c: &Canvas, sh: Shape, vis: Vis, value: &str, caption: &str, text: &str) {
    match sh {
        Shape::Mini | Shape::Dock => compact_sum(c, &Sum::new(vis, value, caption), sh),
        _ => {
            c.tw(caption, 16.0, 16.0, 13.0, 600, c.text1(), c.w - 32.0);
            c.text(text, 16.0, 40.0, 12.0, 400, c.text2(), Align::Left, c.w - 32.0, Font::Text);
        }
    }
}

// ------------------------------------------------------------------ timers

fn timer(c: &Canvas, out: &mut Painted, k: TimerKind, x: &Ctx, sh: Shape) {
    let (col, label, text, running, frac, done) = match k {
        TimerKind::Stopwatch => {
            let e = x.hub.stopwatch.elapsed().as_secs_f32();
            (ORANGE, "Stopwatch".to_string(), hms(e), x.hub.stopwatch.start.is_some(), None, false)
        }
        _ => {
            let t = &x.hub.timers[&k];
            let left = t.left();
            let label = match (k, t.phase, t.done) {
                (_, _, true) => "Done!".to_string(),
                (TimerKind::Pomodoro, 0, _) => "Focus".to_string(),
                (TimerKind::Pomodoro, _, _) => "Break".to_string(),
                _ => "Timer".to_string(),
            };
            (if k == TimerKind::Pomodoro { RED } else { BLUE }, label, hms(left), t.running(), Some(1.0 - left / t.total.max(1) as f32), t.done)
        }
    };
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Timer(k)));
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if running || done {
        c.rr_stroke(0.0, 0.0, c.w, c.h, if dense { 12.0 } else { 18.0 }, alpha(col, 0.9), 1.6);
    }
    let play = if running { G_PAUSE } else { G_PLAY };
    match sh {
        Shape::Mini => {
            // A ring around the time.
            let r = c.h * 0.42;
            c.ring(c.w / 2.0, c.h / 2.0, r, 3.0, frac.unwrap_or(if running { 1.0 } else { 0.0 }), col);
            let fs = fit(c, &text, c.h * 0.24, 7.0, 600, Font::Display, r * 1.6);
            c.text(&text, c.w / 2.0, c.h / 2.0 - fs * 0.68, fs, 600, c.text1(), Align::Center, r * 2.0, Font::Display);
        }
        Shape::Dock => {
            let r = c.h * 0.3;
            let (cx, cy) = (c.h / 2.0, c.h / 2.0);
            match frac {
                Some(f) => c.ring(cx, cy, r, 3.5, f, col),
                None => c.circle(cx, cy, r, alpha(col, 0.9)),
            }
            c.glyph(play, cx, cy, r * 0.8, if frac.is_some() { col } else { WHITE });
            let x0 = c.h * 0.96;
            let vs = (c.h * 0.34).max(13.0);
            let cs = (c.h * 0.19).max(9.5);
            let top = (c.h - vs * 1.25 - cs * 1.3) / 2.0 - 1.0;
            c.text(&text, x0, top, vs, 600, c.text1(), Align::Left, c.w - x0 - 6.0, Font::Display);
            c.tw(&label, x0, top + vs * 1.25, cs, 400, c.text2(), c.w - x0 - 6.0);
        }
        Shape::Small => {
            c.tw(&label, 14.0, 12.0, 11.5, 600, col, c.w - 28.0);
            let fs = fit(c, &text, 30.0, 16.0, 400, Font::Display, c.w - 28.0);
            c.text(&text, 14.0, 28.0, fs, 400, c.text1(), Align::Left, c.w - 28.0, Font::Display);
            let r = 16.0;
            let (bx, by) = (c.w - 12.0 - r, c.h - 10.0 - r);
            c.circle(bx, by, r, col);
            c.glyph(play, bx, by, 13.0, WHITE);
            let rx = bx - r - 20.0;
            c.circle(rx, by, 13.0, c.ink(0.12));
            c.glyph(G_RESET, rx, by, 10.0, c.text1());
            out.hits.push(((rx - 14.0, by - 14.0, 28.0, 28.0), WAct::TimerReset(k)));
        }
        Shape::Medium => {
            c.tw(&label, 18.0, 16.0, 12.0, 600, col, 140.0);
            c.text(&text, 18.0, 34.0, 38.0, 400, c.text1(), Align::Left, c.w * 0.6, Font::Display);
            let r = 22.0;
            let (bx, by) = (c.w - 18.0 - r, c.h / 2.0);
            c.circle(bx, by, r, col);
            c.glyph(play, bx, by, 16.0, WHITE);
            let (rx, ry) = (bx - r - 26.0, by);
            c.circle(rx, ry, 15.0, c.ink(0.12));
            c.glyph(G_RESET, rx, ry, 12.0, c.text1());
            out.hits.push(((rx - 16.0, ry - 16.0, 32.0, 32.0), WAct::TimerReset(k)));
        }
        Shape::Large => {
            // A big ring with the time inside, buttons under it.
            // Card padding (16) above the ring and below the buttons, like the other cards.
            let r = ((c.h - 92.0) / 2.0).min(c.w / 2.0 - 30.0);
            let (cx, cy) = (c.w / 2.0, 20.0 + r);
            c.ring(cx, cy, r, 7.0, frac.unwrap_or(0.0), col);
            c.tc(&label, cx, cy - 34.0, 12.0, 600, col, 2.0 * r);
            let fs = fit(c, &text, 34.0, 16.0, 400, Font::Display, 2.0 * r - 24.0);
            c.text(&text, cx, cy - fs * 0.62, fs, 400, c.text1(), Align::Center, 2.0 * r, Font::Display);
            let by = c.h - 34.0;
            c.circle(cx + 26.0, by, 18.0, col);
            c.glyph(play, cx + 26.0, by, 14.0, WHITE);
            c.circle(cx - 26.0, by, 18.0, c.ink(0.12));
            c.glyph(G_RESET, cx - 26.0, by, 12.0, c.text1());
            out.hits.push(((cx - 46.0, by - 20.0, 40.0, 40.0), WAct::TimerReset(k)));
        }
    }
}

// ------------------------------------------------------------------ media

#[allow(clippy::too_many_arguments)]
fn art_src(x: &Ctx, wic: &IWICImagingFactory) -> Option<IWICBitmapSource> {
    x.hub.art_img.as_ref().map(|a| a.1.clone()).or_else(|| x.control.media.as_ref().and_then(|m| m.thumbnail_png.as_ref()).and_then(|b| load_image_bytes(wic, b)))
}

fn art(c: &Canvas, x: &Ctx, wic: &IWICImagingFactory, ax: f32, ay: f32, s: f32, r: f32) {
    match art_src(x, wic) {
        Some(img) => c.image(&img, ax, ay, s, s, r),
        None => {
            c.rr_grad(ax, ay, s, s, r, hex(0xFF5F6D), hex(0xC2185B));
            c.glyph(G_MUSIC, ax + s / 2.0, ay + s / 2.0, s * 0.42, (1.0, 1.0, 1.0, 0.95));
        }
    }
}

fn controls(c: &Canvas, out: &mut Painted, cx: f32, cy: f32, gap: f32, playing: bool) {
    for (i, (g, a)) in [(G_PREV, WAct::MediaPrev), (if playing { G_PAUSE } else { G_PLAY }, WAct::MediaPlay), (G_NEXT, WAct::MediaNext)].into_iter().enumerate() {
        let x = cx + (i as f32 - 1.0) * gap;
        c.glyph(g, x, cy, if i == 1 { 18.0 } else { 14.0 }, c.text1());
        out.hits.push(((x - 16.0, cy - 16.0, 32.0, 32.0), a));
    }
}

fn media(c: &Canvas, out: &mut Painted, k: MediaKind, x: &Ctx, wic: &IWICImagingFactory, sh: Shape) {
    let m = x.control.media.as_ref();
    let (title, artist, playing) = match m {
        Some(m) => (m.title.clone(), m.artist.clone(), m.playing),
        None => ("Nothing playing".to_string(), String::new(), false),
    };
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    match k {
        MediaKind::Output => {
            let vol = x.control.volume.unwrap_or(0.0);
            let muted = x.control.muted == Some(true);
            let g = if muted { G_MUTE } else { G_VOL };
            let pct = format!("{:.0}%", vol * 100.0);
            let dev = x.hub.output_name.clone().unwrap_or_else(|| "Sound".into());
            if dense {
                let mut s = Sum::new(Vis::Glyph(g, c.text1()), pct, dev);
                s.meter = Some((vol, BLUE));
                compact_sum(c, &s, sh);
                out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Mute));
                return;
            }
            let small = sh == Shape::Small;
            c.glyph(g, 26.0, 26.0, 16.0, c.text1());
            out.hits.push(((10.0, 10.0, 32.0, 32.0), WAct::Mute));
            c.tw(if small { "Sound" } else { &dev }, 46.0, 17.0, 12.5, 600, c.text1(), c.w - 100.0);
            c.tr(&pct, c.w - 16.0, 17.0, 12.0, 500, c.text2());
            let (bx, bw, bh) = (16.0, c.w - 32.0, 24.0);
            let by = if sh == Shape::Large { c.h / 2.0 - bh / 2.0 } else { c.h - bh - 16.0 };
            c.rr(bx, by, bw, bh, bh / 2.0, c.ink(0.12));
            c.rr(bx, by, (bw * vol).max(bh), bh, bh / 2.0, if muted { c.ink(0.3) } else { BLUE });
            out.hits.push(((bx, by - 4.0, bw, bh + 8.0), WAct::Volume));
            if small {
                c.tw(&dev, 16.0, 48.0, 10.5, 400, c.text3(), c.w - 32.0);
            }
            return;
        }
        MediaKind::Mic => {
            let muted = x.hub.mic_muted == Some(true);
            tile_sub(c, out, G_MIC, "Microphone", if muted { "Muted" } else { "On" }, muted, RED, WAct::Mic, dense);
            if muted {
                // A slash across the plate's microphone.
                let (cx, cy, s) = plate_center(c, dense);
                c.line(cx - s * 0.2, cy - s * 0.2, cx + s * 0.2, cy + s * 0.2, 2.0, WHITE);
            }
            return;
        }
        _ => {}
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::MediaPlay));
    if m.is_none() {
        if !dense && k == MediaKind::Disc {
            bg(c, false);
        }
        compact_or_text(c, sh, Vis::Glyph(G_MUSIC, c.text3()), if sh == Shape::Mini { "" } else { "Not playing" }, "Nothing playing", "Play something in any app to control it here.");
        return;
    }
    if k == MediaKind::Disc {
        if !dense {
            bg(c, false);
        }
        let s = c.h.min(c.w) - if dense { 6.0 } else { 16.0 };
        let (ax, ay) = ((c.w - s) / 2.0, (c.h - s) / 2.0);
        let (cx, cy) = (c.w / 2.0, c.h / 2.0);
        if art_src(x, wic).is_some() {
            art(c, x, wic, ax, ay, s, s / 2.0);
        } else {
            // No cover: a record, with grooves and a coloured label.
            c.circle(cx, cy, s / 2.0, (0.07, 0.07, 0.08, 1.0));
            for k in 1..5 {
                c.circle_stroke(cx, cy, s / 2.0 * (0.48 + k as f32 * 0.11), (1.0, 1.0, 1.0, 0.06), 1.0);
            }
            c.rr_grad(cx - s * 0.19, cy - s * 0.19, s * 0.38, s * 0.38, s * 0.19, hex(0xFF5F6D), hex(0xC2185B));
        }
        c.circle_stroke(cx, cy, s / 2.0 - 0.5, (1.0, 1.0, 1.0, 0.2), 1.0);
        if playing {
            c.circle(cx, cy, (s * 0.05).max(2.0), (0.1, 0.1, 0.12, 1.0));
        } else {
            // Paused: a clear play button where the spindle would be.
            c.circle(cx, cy, s * 0.17, (0.0, 0.0, 0.0, 0.55));
            c.glyph(G_PLAY, cx + s * 0.01, cy, s * 0.15, WHITE);
        }
        return;
    }
    match sh {
        Shape::Mini => {
            let s = c.h - 8.0;
            art(c, x, wic, (c.w - s) / 2.0, 4.0, s, 9.0);
            if !playing {
                c.glyph(G_PLAY, c.w / 2.0, c.h / 2.0, s * 0.3, WHITE);
            }
        }
        Shape::Dock => {
            let s = c.h - 10.0;
            art(c, x, wic, 5.0, 5.0, s, 8.0);
            let tx = s + 12.0;
            let room = if c.w > c.h * 3.2 { c.w - tx - 34.0 } else { c.w - tx - 6.0 };
            c.tw(&title, tx, c.h * 0.16, (c.h * 0.24).max(10.5), 600, c.text1(), room);
            c.tw(&artist, tx, c.h * 0.52, (c.h * 0.2).max(9.5), 400, c.text2(), room);
            if c.w > c.h * 3.2 {
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 18.0, c.h / 2.0, 13.0, c.text1());
            }
        }
        Shape::Small => {
            art(c, x, wic, 14.0, 14.0, 44.0, 9.0);
            c.tw(&title, 14.0, 64.0, 12.5, 600, c.text1(), c.w - 28.0);
            c.tw(&artist, 14.0, 80.0, 10.5, 400, c.text2(), c.w - 28.0);
            if k == MediaKind::Full {
                out.hits.clear();
                // The tile itself plays/pauses; the buttons along the bottom do the rest.
                out.hits.push(((0.0, 0.0, c.w, c.h - 26.0), WAct::MediaPlay));
                controls(c, out, c.w / 2.0, c.h - 12.0, 34.0, playing);
            } else {
                c.circle(c.w - 30.0, 36.0, 15.0, c.ink(0.12));
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 30.0, 36.0, 12.0, c.text1());
            }
        }
        Shape::Large => {
            let s = (c.h - 74.0).min(c.w * 0.5);
            art(c, x, wic, 16.0, 16.0, s, 14.0);
            let tx = 16.0 + s + 14.0;
            c.text(&title, tx, 20.0, 15.0, 600, c.text1(), Align::Left, c.w - tx - 14.0, Font::Text);
            c.tw(&artist, tx, 44.0, 12.0, 400, c.text2(), c.w - tx - 14.0);
            out.hits.clear();
            controls(c, out, c.w / 2.0, c.h - 28.0, 54.0, playing);
        }
        Shape::Medium => match k {
            MediaKind::Full => {
                art(c, x, wic, 14.0, 14.0, 68.0, 12.0);
                c.tw(&title, 94.0, 16.0, 13.5, 600, c.text1(), c.w - 108.0);
                c.tw(&artist, 94.0, 36.0, 11.5, 400, c.text2(), c.w - 108.0);
                out.hits.clear();
                controls(c, out, 140.0, 70.0, 36.0, playing);
            }
            MediaKind::Compact => {
                art(c, x, wic, 16.0, (c.h - 48.0) / 2.0, 48.0, 10.0);
                c.tw(&title, 76.0, c.h / 2.0 - 18.0, 13.0, 600, c.text1(), c.w - 140.0);
                c.tw(&artist, 76.0, c.h / 2.0 + 2.0, 11.5, 400, c.text2(), c.w - 140.0);
                c.circle(c.w - 36.0, c.h / 2.0, 18.0, c.ink(0.12));
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 36.0, c.h / 2.0, 14.0, c.text1());
            }
            _ => {
                art(c, x, wic, 16.0, 14.0, 40.0, 8.0);
                c.tw(&title, 66.0, 15.0, 13.0, 600, c.text1(), c.w - 120.0);
                c.tw(&artist, 66.0, 34.0, 11.5, 400, c.text2(), c.w - 120.0);
                c.glyph(if playing { G_PAUSE } else { G_PLAY }, c.w - 30.0, 34.0, 16.0, c.text1());
                // A level meter that sways while playing.
                let n = 28;
                let bw = (c.w - 32.0) / n as f32;
                for i in 0..n {
                    let ph = (x.now.second * 7 + i * 13) % 17;
                    let hgt = if playing { 3.0 + ph as f32 * 0.9 } else { 3.0 };
                    c.rr(16.0 + i as f32 * bw, c.h - 12.0 - hgt, bw * 0.6, hgt, 1.5, if playing { alpha(PURPLE, 0.85) } else { c.ink(0.2) });
                }
            }
        },
    }
}

// ------------------------------------------------------------------ system

fn rate(bps: f64) -> String {
    if bps >= 1_000_000.0 {
        format!("{:.1} MB/s", bps / 1_000_000.0)
    } else {
        format!("{:.0} KB/s", bps / 1000.0)
    }
}

/// A ring face (CPU, memory, battery, disk) at every size.
#[allow(clippy::too_many_arguments)]
fn ring_face(c: &Canvas, sh: Shape, frac: f32, col: Col, big: &str, label: &str, sub: &str, series: Option<Vec<f64>>) {
    match sh {
        Shape::Mini => compact_sum(c, &Sum::new(Vis::Ring(frac, col, big.to_string()), "", ""), sh),
        Shape::Dock => compact_sum(c, &Sum::new(Vis::Ring(frac, col, String::new()), big, label), sh),
        Shape::Small => {
            let r = 30.0;
            c.ring(c.w / 2.0, 14.0 + r, r - 4.0, 7.0, frac, col);
            c.tc(big, c.w / 2.0, 14.0 + r - 9.0, 13.5, 600, c.text1(), 2.0 * r);
            c.tc(label, c.w / 2.0, 82.0, 12.5, 600, c.text1(), c.w - 16.0);
            c.tc(sub, c.w / 2.0, 99.0, 10.5, 400, c.text2(), c.w - 16.0);
        }
        _ => {
            let big_card = sh == Shape::Large;
            let r = if big_card { 44.0 } else { 30.0 };
            let (cx, cy) = (18.0 + r, if big_card { 16.0 + r } else { c.h / 2.0 });
            c.ring(cx, cy, r, if big_card { 9.0 } else { 7.0 }, frac, col);
            c.tc(big, cx, cy - if big_card { 11.0 } else { 9.0 }, if big_card { 17.0 } else { 13.5 }, 600, c.text1(), 2.0 * r);
            let tx = cx + r + 18.0;
            c.tw(label, tx, cy - 22.0, 13.0, 600, c.text1(), c.w - tx - 14.0);
            c.tw(sub, tx, cy - 2.0, 11.5, 400, c.text2(), c.w - tx - 14.0);
            if let Some(sv) = series.filter(|s| s.len() > 2) {
                if big_card {
                    c.area(14.0, 2.0 * r + 34.0, c.w - 28.0, c.h - 2.0 * r - 48.0, &sv, col, true);
                } else {
                    c.area(tx, 60.0, c.w - tx - 16.0, 22.0, &sv, col, true);
                }
            }
        }
    }
}

fn sys(c: &Canvas, out: &mut Painted, k: SysKind, x: &Ctx, sh: Shape) {
    let st = x.stats;
    let cpu = st.map(|s| s.cpu_percent).unwrap_or(0.0);
    let (mu, mt) = st.map(|s| (s.mem_used_gb, s.mem_total_gb)).unwrap_or((0.0, 0.0));
    let memf = if mt > 0.0 { mu / mt } else { 0.0 };
    if matches!(k, SysKind::Resource | SysKind::Cpu | SysKind::Memory | SysKind::Network) {
        out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("taskmgr".into())));
    }
    let cpu_hist: Vec<f64> = x.hub.cpu_hist.iter().map(|v| *v as f64).collect();
    match k {
        SysKind::Resource => match sh {
            Shape::Mini => compact_sum(c, &Sum::new(Vis::Ring(cpu / 100.0, BLUE, format!("{cpu:.0}")), "", ""), sh),
            Shape::Dock => {
                let r = c.h * 0.3;
                c.ring(c.h / 2.0, c.h / 2.0, r, 4.0, cpu / 100.0, BLUE);
                let tx = c.h;
                let fs = (c.h * 0.22).max(10.0);
                c.tw(&format!("CPU {cpu:.0}%"), tx, c.h * 0.2, fs, 600, c.text1(), c.w - tx - 6.0);
                c.tw(&format!("RAM {:.0}%", memf * 100.0), tx, c.h * 0.54, fs, 600, c.text2(), c.w - tx - 6.0);
            }
            Shape::Small => {
                let r = 28.0;
                c.ring(c.w / 2.0, 12.0 + r, r - 4.0, 7.0, cpu / 100.0, GREEN);
                c.tc(&format!("{cpu:.0}%"), c.w / 2.0, 12.0 + r - 15.0, 13.0, 600, c.text1(), 2.0 * r);
                c.tc("CPU", c.w / 2.0, 12.0 + r + 1.0, 9.0, 600, c.text3(), 2.0 * r);
                let y = 82.0;
                c.tw("MEM", 14.0, y, 10.5, 600, c.text2(), 40.0);
                c.bar(48.0, y + 5.0, c.w - 48.0 - 46.0, 5.0, memf, PURPLE);
                c.tr(&format!("{:.0}%", memf * 100.0), c.w - 14.0, y, 11.0, 600, c.text1());
            }
            _ => {
                c.ring(48.0, 48.0, 30.0, 7.0, cpu / 100.0, GREEN);
                c.tc(&format!("{cpu:.0}%"), 48.0, 48.0 - 14.0, 13.5, 600, c.text1(), 60.0);
                c.tc("CPU", 48.0, 48.0 + 3.0, 9.5, 600, c.text3(), 60.0);
                for (i, (l, f, v, col)) in [("CPU", cpu / 100.0, format!("{cpu:.0}%"), BLUE), ("MEM", memf, format!("{:.0}%", memf * 100.0), PURPLE)].iter().enumerate() {
                    let y = 26.0 + i as f32 * 26.0;
                    c.tw(l, 96.0, y, 11.0, 600, c.text2(), 40.0);
                    c.bar(132.0, y + 5.0, c.w - 132.0 - 56.0, 5.0, *f, *col);
                    c.tr(v, c.w - 16.0, y, 12.0, 600, c.text1());
                }
                if sh == Shape::Large {
                    c.tw(&format!("{mu:.1} of {mt:.0} GB memory in use"), 16.0, 98.0, 11.0, 400, c.text3(), c.w - 32.0);
                    c.area(14.0, 120.0, c.w - 28.0, c.h - 134.0, &cpu_hist, BLUE, true);
                }
            }
        },
        SysKind::Cpu => ring_face(c, sh, cpu / 100.0, BLUE, &format!("{cpu:.0}%"), "CPU", "Processor load", Some(cpu_hist)),
        SysKind::Memory => ring_face(c, sh, memf, PURPLE, &format!("{:.0}%", memf * 100.0), "Memory", &format!("{mu:.1} of {mt:.0} GB"), None),
        SysKind::Network => {
            let (d, u) = st.map(|s| (s.net_down_bps, s.net_up_bps)).unwrap_or((0.0, 0.0));
            let down: Vec<f64> = x.hub.net_hist.iter().map(|v| v.0).collect();
            let up: Vec<f64> = x.hub.net_hist.iter().map(|v| v.1).collect();
            let short = |b: f64| rate(b).replace(" KB/s", "K").replace(" MB/s", "M");
            match sh {
                Shape::Mini => {
                    let fs = (c.h * 0.2).max(8.0);
                    c.tc(&format!("↓{}", short(d)), c.w / 2.0, c.h * 0.18, fs, 600, BLUE, c.w);
                    c.tc(&format!("↑{}", short(u)), c.w / 2.0, c.h * 0.54, fs, 600, GREEN, c.w);
                }
                Shape::Dock => {
                    let s = (c.h * 0.24).max(10.0);
                    c.tw(&format!("↓ {}", rate(d)), 10.0, c.h * 0.16, s, 600, BLUE, c.w);
                    c.tw(&format!("↑ {}", rate(u)), 10.0, c.h * 0.52, s, 600, GREEN, c.w);
                }
                Shape::Small => {
                    c.tw("Network", 14.0, 12.0, 11.5, 600, c.text2(), 100.0);
                    c.tw(&format!("↓ {}", rate(d)), 14.0, 32.0, 14.0, 600, BLUE, c.w - 28.0);
                    c.tw(&format!("↑ {}", rate(u)), 14.0, 52.0, 14.0, 600, GREEN, c.w - 28.0);
                    c.area(14.0, 80.0, c.w - 28.0, c.h - 92.0, &down, BLUE, true);
                }
                _ => {
                    c.tw("Network", 16.0, 12.0, 12.0, 600, c.text2(), 100.0);
                    c.tw(&format!("↓ {}", rate(d)), 16.0, 32.0, 15.0, 600, BLUE, c.w / 2.0);
                    c.tw(&format!("↑ {}", rate(u)), c.w / 2.0, 32.0, 15.0, 600, GREEN, c.w / 2.0 - 16.0);
                    let gh = c.h - 70.0;
                    c.area(16.0, 58.0, c.w - 32.0, gh, &down, BLUE, true);
                    c.area(16.0, 58.0, c.w - 32.0, gh, &up, GREEN, false);
                }
            }
        }
        SysKind::Battery | SysKind::Disk | SysKind::Storage | SysKind::Language | SysKind::Uptime => {
            let conn = match k {
                SysKind::Battery => "battery",
                SysKind::Language => "language",
                SysKind::Uptime => "uptime",
                _ => "storage",
            };
            let badge = super::canvas::gly(G_PLUG, 0x2C2C34, 0x30D158);
            let g = got(x, conn);
            let Got::Feed(f) = g else {
                let name = match k {
                    SysKind::Battery => "Battery",
                    SysKind::Language => "Keyboard Language",
                    SysKind::Uptime => "Uptime",
                    _ => "Storage",
                };
                placeholder(c, out, name, &badge, conn, &g, sh);
                return;
            };
            match k {
                SysKind::Battery => {
                    let charging = f.extra.get("charging").map(|v| v == "true").unwrap_or(false);
                    let frac = f.frac.unwrap_or(0.0);
                    let col = if frac < 0.2 && !charging { RED } else { GREEN };
                    let sub = if charging { "Charging".to_string() } else { f.extra.get("time_left").filter(|s| !s.is_empty()).map(|t| format!("{t} left")).unwrap_or_else(|| "On battery".into()) };
                    ring_face(c, sh, frac, col, &format!("{:.0}%", frac * 100.0), "Battery", &sub, None);
                }
                SysKind::Disk => {
                    let used = f.frac.unwrap_or(0.0);
                    ring_face(c, sh, used, ORANGE, &format!("{:.0}%", used * 100.0), "Disk C:", &format!("{} GB free", compact(f.value.unwrap_or(0.0))), None);
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:storagesense".into())));
                }
                SysKind::Storage => {
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:storagesense".into())));
                    let used = f.frac.unwrap_or(0.0);
                    if compactish(sh) {
                        let mut s = Sum::new(Vis::Glyph(G_DRIVE, c.text2()), format!("{:.0} GB", f.value.unwrap_or(0.0)), "Free on C:");
                        s.meter = Some((used, if used > 0.9 { RED } else { BLUE }));
                        compact_sum(c, &s, sh);
                        return;
                    }
                    c.num(&format!("{:.0} GB", f.value.unwrap_or(0.0)), 16.0, 14.0, 28.0, 600, c.text1());
                    c.tr("Free on C:", c.w - 16.0, 22.0, 12.0, 400, c.text2());
                    c.bar(16.0, 58.0, c.w - 32.0, 8.0, used, if used > 0.9 { RED } else { BLUE });
                    if sh == Shape::Large {
                        details(c, &f, BLUE);
                    } else {
                        let total = f.rows.first().map(|r| r.value.clone()).unwrap_or_default();
                        c.tw(&total, 16.0, 72.0, 11.0, 400, c.text3(), c.w - 32.0);
                    }
                }
                SysKind::Language => {
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Language));
                    let code = f.text.clone().unwrap_or_else(|| "—".into());
                    match sh {
                        Shape::Mini | Shape::Dock => {
                            let fs = fit(c, &code, c.h * 0.36, 9.0, 700, Font::Display, c.w - 8.0);
                            c.text(&code, c.w / 2.0, (c.h - fs * 1.3) / 2.0, fs, 700, c.text1(), Align::Center, c.w, Font::Display);
                        }
                        Shape::Small => {
                            c.glyph(G_KEYBOARD, 30.0, 30.0, 20.0, c.text2());
                            c.num(&code, 14.0, 52.0, 28.0, 700, c.text1());
                            c.tw(&f.caption, 14.0, 92.0, 10.5, 400, c.text2(), c.w - 28.0);
                        }
                        _ => {
                            c.glyph(G_KEYBOARD, 34.0, c.h / 2.0, 22.0, c.text2());
                            c.num(&code, 64.0, c.h / 2.0 - 30.0, 28.0, 700, c.text1());
                            c.tw(&f.caption, 64.0, c.h / 2.0 + 10.0, 12.0, 400, c.text2(), c.w - 80.0);
                        }
                    }
                }
                _ => {
                    let t = f.text.clone().unwrap_or_else(|| headline(&f));
                    if compactish(sh) {
                        compact_sum(c, &Sum::new(Vis::Glyph(G_CLOCK, c.text2()), t, "Uptime"), sh);
                        return;
                    }
                    c.glyph(G_CLOCK, 34.0, c.h / 2.0, 22.0, c.text2());
                    c.num(&t, 64.0, c.h / 2.0 - 28.0, 26.0, 600, c.text1());
                    c.tw("Since Windows started", 64.0, c.h / 2.0 + 10.0, 12.0, 400, c.text2(), c.w - 80.0);
                }
            }
        }
        SysKind::Brightness => {
            let b = x.hub.brightness;
            if matches!(sh, Shape::Mini | Shape::Dock) || (sh == Shape::Small && b.is_none()) {
                let mut s = Sum::new(Vis::Glyph(G_SUN, c.text1()), b.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| "—".into()), "Brightness");
                s.meter = b.map(|v| (v, YELLOW));
                compact_sum(c, &s, sh);
                out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:display".into())));
                return;
            }
            c.glyph(G_SUN, 26.0, 26.0, 16.0, c.text1());
            c.tw("Brightness", 46.0, 17.0, 12.5, 600, c.text1(), c.w - 100.0);
            match b {
                Some(v) => {
                    c.tr(&format!("{:.0}%", v * 100.0), c.w - 16.0, 17.0, 12.0, 500, c.text2());
                    let (bx, bw, bh) = (16.0, c.w - 32.0, 24.0);
                    let by = if sh == Shape::Large { c.h / 2.0 - bh / 2.0 } else { c.h - bh - 16.0 };
                    c.rr(bx, by, bw, bh, bh / 2.0, c.ink(0.12));
                    c.rr(bx, by, (bw * v).max(bh), bh, bh / 2.0, (1.0, 1.0, 1.0, 0.95));
                    c.glyph(G_SUN, bx + 14.0, by + bh / 2.0, 11.0, (0.1, 0.1, 0.12, 0.85));
                    out.hits.push(((bx, by - 4.0, bw, bh + 8.0), WAct::Brightness));
                }
                None => {
                    c.text("This monitor doesn't report its brightness (DDC/CI). Click for Display settings.", 16.0, 48.0, 11.0, 400, c.text3(), Align::Left, c.w - 32.0, Font::Text);
                    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open("ms-settings:display".into())));
                }
            }
        }
    }
}

// ------------------------------------------------------------------ tools
// One layout for every single-action tool (docs/widget-design.md §3, §5): a control mark
// (a circle for on/off toggles, a rounded plate for launchers), its name and state.
//   Mini   mark 40 centred
//   Dock   mark 30 at (12, 12), name 13/600 and state 11 at x 52; Dock L adds a switch,
//          the Windows shortcut or a value in the accessory slot
//   Small  mark 44 centred at y 22, name baseline 88, state baseline 104
//   Medium mark 44 at (16, 26), name 17/600 baseline 45, state 12 baseline 63
//   Large  Medium unchanged, a separator at 96, then up to three fact rows at a 24 pitch

const G_OPEN: &str = "\u{E8A7}";
const G_FOLDER: &str = "\u{E8B7}";
const G_ADD: &str = "\u{E710}";
const G_FOCUS: &str = "\u{EC43}";
const G_PALETTE: &str = "\u{E790}";
const G_POWER: &str = "\u{E7E8}";
const G_EYE: &str = "\u{E7B3}";
const G_CUT: &str = "\u{E8C6}";
const G_REGION: &str = "\u{F407}";
const G_WINDOW: &str = "\u{E737}";
const G_FULL: &str = "\u{E740}";
const WHATSAPP: Col = hex(0x25D366);

/// Mark sizes per shape (dock sizes are drawn at 54 and scale with the icon size).
const MARK_MINI: f32 = 40.0;
const MARK_CARD: f32 = 44.0;
/// Small: the mark's top, then the name and state baselines.
const SMALL_MARK_Y: f32 = 22.0;
const SMALL_NAME_BASE: f32 = 88.0;
const SMALL_STATE_BASE: f32 = 104.0;
/// Medium: the mark's top-left, the text column, name and state baselines.
const MED_MARK_Y: f32 = 26.0;
const MED_TEXT_X: f32 = 72.0;
const MED_NAME_BASE: f32 = 45.0;
const MED_STATE_BASE: f32 = 63.0;
/// Keycaps and switches.
const KEY_H: f32 = 20.0;
const KEY_R: f32 = 6.0;
const SWITCH_W: f32 = 36.0;
const SWITCH_H: f32 = 20.0;

/// The dock's scale (its layouts are specified at 54 tall).
fn dock_k(c: &Canvas) -> f32 {
    c.h / 54.0
}

/// A wide dock pill (Dock L), which has room for the accessory slot.
fn dock_wide(c: &Canvas) -> bool {
    c.w >= c.h * 3.4
}

/// The mark's box (x, y, size) for a shape.
fn mark_box(c: &Canvas, sh: Shape) -> (f32, f32, f32) {
    match sh {
        Shape::Mini => {
            let s = MARK_MINI * dock_k(c);
            ((c.w - s) / 2.0, (c.h - s) / 2.0, s)
        }
        Shape::Dock => {
            let k = dock_k(c);
            let s = style::BADGE_DOCK * k;
            (style::M_DOCK * k, (c.h - s) / 2.0, s)
        }
        Shape::Small => ((c.w - MARK_CARD) / 2.0, SMALL_MARK_Y, MARK_CARD),
        Shape::Medium | Shape::Large => (style::M_CARD, MED_MARK_Y, MARK_CARD),
    }
}

/// Where a tile's plate sits (centre and size), to draw on top of it.
fn plate_center(c: &Canvas, dense: bool) -> (f32, f32, f32) {
    let sh = if dense {
        if c.w > c.h * 1.5 {
            Shape::Dock
        } else {
            Shape::Mini
        }
    } else if c.w >= 180.0 {
        Shape::Medium
    } else {
        Shape::Small
    };
    let (x, y, s) = mark_box(c, sh);
    (x + s / 2.0, y + s / 2.0, s)
}

/// A control mark: a circle (toggles) or a rounded plate (launchers). Filled with `fill`
/// and a white glyph when given, else the plate token with a text1 glyph.
fn mark(c: &Canvas, g: &str, x: f32, y: f32, s: f32, round: bool, fill: Option<Col>) {
    let (bg, ink) = match fill {
        Some(f) => (f, WHITE),
        None => (c.plate(), c.text1()),
    };
    if round {
        c.circle(x + s / 2.0, y + s / 2.0, s / 2.0, bg);
        c.glyph(g, x + s / 2.0, y + s / 2.0, s * style::GLYPH_IN, ink);
    } else {
        plate(c, g, x, y, s, bg, ink);
    }
}

/// A red attention dot on a mark's top-right corner, ringed in the card colour.
fn mark_dot(c: &Canvas, x: f32, y: f32, s: f32, d: f32, col: Col) {
    let (cx, cy) = (x + s - 2.0, y + 2.0);
    c.circle(cx, cy, d / 2.0 + 1.5, c.halo());
    c.circle(cx, cy, d / 2.0, col);
}

/// A keycap ("Win + D") right-aligned at `right`, centred on `cy`. Returns its width.
fn keycap(c: &Canvas, keys: &str, right: f32, cy: f32, k: f32) -> f32 {
    let fs = style::MICRO.size * k;
    let w = c.measure(keys, fs, style::MICRO.weight, Font::Text).0 + 2.0 * style::SP[3] * k;
    let h = KEY_H * k;
    c.rr(right - w, cy - h / 2.0, w, h, KEY_R * k, c.plate());
    c.text_base(keys, right - w / 2.0, cy + c.cap_h(fs) / 2.0, fs, style::MICRO.weight, c.text2(), Align::Center, w);
    w
}

/// An on/off switch right-aligned at `right`, centred on `cy`. Returns its width.
fn switch(c: &Canvas, on: bool, right: f32, cy: f32, k: f32) -> f32 {
    let (w, h) = (SWITCH_W * k, SWITCH_H * k);
    let x = right - w;
    c.rr(x, cy - h / 2.0, w, h, h / 2.0, if on { c.blue() } else { c.track() });
    let r = h / 2.0 - style::SP[0] * k;
    let kx = if on { right - h / 2.0 } else { x + h / 2.0 };
    c.circle(kx, cy, r, WHITE);
    if c.light {
        c.circle_stroke(kx, cy, r, c.card_edge(), 1.0);
    }
    w
}

/// A fully round capsule with a white label (Dock L slot): returns its width.
fn capsule(c: &Canvas, s: &str, right: f32, cy: f32, fill: Col, k: f32) -> f32 {
    let fs = style::FOOT.size * k;
    let w = c.measure(s, fs, 600, Font::Text).0 + 2.0 * style::SP[3] * k;
    let h = KEY_H * k;
    c.rr(right - w, cy - h / 2.0, w, h, h / 2.0, fill);
    c.text_base(s, right - w / 2.0, cy + c.cap_h(fs) / 2.0, fs, 600, WHITE, Align::Center, w);
    w
}

/// What a tool's Dock L slot (and Medium's right edge) holds.
enum ToolSlot {
    None,
    /// On/off switch (toggles).
    Switch,
    /// The Windows shortcut.
    Key(&'static str),
    /// A value 17/600.
    Value(String),
    /// A white label on a coloured capsule.
    Capsule(&'static str, Col),
    /// A quiet note in text3.
    Note(String),
}

/// The end of a Large fact row.
enum Trail {
    Text(String),
    Key(&'static str),
    /// A link: chevron, and the row opens it.
    Go(WAct),
    None,
}

/// A Large fact row: glyph, label and trailing value. A row with no glyph is a footnote.
struct TRow {
    g: Option<&'static str>,
    label: String,
    trail: Trail,
}

fn row(g: &'static str, label: &str, trail: Trail) -> TRow {
    TRow { g: Some(g), label: label.to_string(), trail }
}

fn note_row(label: &str) -> TRow {
    TRow { g: None, label: label.to_string(), trail: Trail::None }
}

/// A single-action tool, described once and drawn at every size.
struct ToolFace {
    g: &'static str,
    /// Name on Small and up.
    name: &'static str,
    /// Name on the dock (never ellipsized).
    short: &'static str,
    /// Dock second line (empty: one line).
    state: String,
    /// Small and Medium captions.
    small: String,
    medium: String,
    /// Toggles are round; launchers are plates.
    round: bool,
    /// The mark's fill: blue for an on toggle, the identity colour, or None for the plate.
    fill: Option<Col>,
    slot: ToolSlot,
    /// Show the slot on Medium too (switches only).
    slot_medium: bool,
    rows: Vec<TRow>,
    act: WAct,
}

impl ToolFace {
    fn new(g: &'static str, name: &'static str, act: WAct) -> Self {
        ToolFace { g, name, short: name, state: String::new(), small: String::new(), medium: String::new(), round: false, fill: None, slot: ToolSlot::None, slot_medium: false, rows: Vec::new(), act }
    }
}

/// Draw a tool at any shape. Returns the mark's box so callers can badge it.
fn tool_face(c: &Canvas, out: &mut Painted, f: &ToolFace, sh: Shape) -> (f32, f32, f32) {
    out.hits.push(((0.0, 0.0, c.w, c.h), f.act.clone()));
    let (mx, my, ms) = mark_box(c, sh);
    mark(c, f.g, mx, my, ms, f.round, f.fill);
    match sh {
        Shape::Mini => {}
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                if let Some(w) = tool_slot(c, &f.slot, c.w - m, 27.0 * k, k, f.fill.is_some()) {
                    right = c.w - m - w - style::SP[5] * k;
                }
            }
            let ls = fit(c, f.short, style::TITLE.size * k, style::TITLE.min * k, 600, Font::Text, right - x0);
            if f.state.is_empty() {
                c.text_base(f.short, x0, 32.0 * k, ls, 600, c.text1(), Align::Left, c.w);
            } else {
                c.text_base(f.short, x0, 26.0 * k, ls, 600, c.text1(), Align::Left, c.w);
                c.text_base(&f.state, x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
            }
        }
        Shape::Small => {
            let inner = c.w - 2.0 * style::M_SMALL;
            let ls = fit(c, f.name, style::TITLE.size, style::TITLE.min, 600, Font::Text, inner);
            c.text_base(f.name, c.w / 2.0, SMALL_NAME_BASE, ls, 600, c.text1(), Align::Center, c.w);
            c.text_base(&f.small, c.w / 2.0, SMALL_STATE_BASE, style::FOOT.size, 400, c.text2(), Align::Center, inner);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let mut right = c.w - m;
            if f.slot_medium {
                if let Some(w) = tool_slot(c, &f.slot, c.w - m, my + ms / 2.0, 1.0, f.fill.is_some()) {
                    right -= w + style::SP[5];
                }
            }
            let ls = fit(c, f.name, style::VALUE.size, style::VALUE.min, 600, Font::Text, right - MED_TEXT_X);
            c.text_base(f.name, MED_TEXT_X, MED_NAME_BASE, ls, 600, c.text1(), Align::Left, c.w);
            c.text_base(&f.medium, MED_TEXT_X, MED_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, right - MED_TEXT_X);
            if sh == Shape::Large {
                tool_rows(c, out, &f.rows);
            }
        }
    }
    (mx, my, ms)
}

/// Draw a slot right-aligned at `right`, centred on `cy`. Returns its width (None if empty).
fn tool_slot(c: &Canvas, s: &ToolSlot, right: f32, cy: f32, k: f32, on: bool) -> Option<f32> {
    match s {
        ToolSlot::None => None,
        ToolSlot::Switch => Some(switch(c, on, right, cy, k)),
        ToolSlot::Key(keys) => Some(keycap(c, keys, right, cy, k)),
        ToolSlot::Capsule(t, col) => Some(capsule(c, t, right, cy, *col, k)),
        ToolSlot::Value(v) => {
            let fs = style::VALUE.size * k;
            Some(c.text_base(v, right, cy + c.cap_h(fs) / 2.0, fs, style::VALUE.weight, c.text1(), Align::Right, c.w))
        }
        ToolSlot::Note(t) => {
            let fs = style::FOOT.size * k;
            Some(c.text_base(t, right, cy + c.cap_h(fs) / 2.0, fs, 400, c.text3(), Align::Right, c.w))
        }
    }
}

/// The Large detail region: a separator at 96, then up to three rows at a 24 pitch.
fn tool_rows(c: &Canvas, out: &mut Painted, rows: &[TRow]) {
    if rows.is_empty() {
        return;
    }
    separator(c, style::SEP_Y_LARGE);
    let m = style::M_CARD;
    let gw = 16.0;
    for (i, r) in rows.iter().take(3).enumerate() {
        let top = style::DETAIL_TOP + i as f32 * style::ROW_L;
        let cy = top + style::ROW_L / 2.0;
        let base = cy + c.cap_h(style::BODY.size) / 2.0;
        let mut right = c.w - m;
        match &r.trail {
            Trail::Text(t) => {
                let w = c.text_base(t, right, cy + c.cap_h(style::FOOT.size) / 2.0, style::FOOT.size, 500, c.text2(), Align::Right, c.w);
                right -= w + style::SP[3];
            }
            Trail::Key(keys) => {
                let w = keycap(c, keys, right, cy, 1.0);
                right -= w + style::SP[3];
            }
            Trail::Go(act) => {
                c.glyph(G_CHEV, right - 4.0, cy, 10.0, c.text3());
                right -= 8.0 + style::SP[3];
                out.hits.push(((m, top, c.w - 2.0 * m, style::ROW_L), act.clone()));
            }
            Trail::None => {}
        }
        match r.g {
            Some(g) => {
                c.glyph(g, m + gw / 2.0, cy, 14.0, c.text2());
                let x = m + gw + style::SP[3];
                c.text_base(&r.label, x, base, style::BODY.size, style::BODY.weight, c.text1(), Align::Left, right - x);
            }
            None => {
                c.text_base(&r.label, m, cy + c.cap_h(style::FOOT.size) / 2.0, style::FOOT.size, 400, c.text2(), Align::Left, right - m);
            }
        }
    }
}

/// A control tile: a plate with an icon (filled with `col` when on) and its name.
#[allow(clippy::too_many_arguments)]
fn tile(c: &Canvas, out: &mut Painted, g: &str, label: &str, on: bool, col: Col, act: WAct, dense: bool) {
    tile_sub(c, out, g, label, "", on, col, act, dense);
}

/// A control tile with a state line. The shared tool layout, without Large rows.
#[allow(clippy::too_many_arguments)]
fn tile_sub(c: &Canvas, out: &mut Painted, g: &str, label: &str, sub: &str, on: bool, col: Col, act: WAct, dense: bool) {
    let sh = if dense {
        if c.w > c.h * 1.5 {
            Shape::Dock
        } else {
            Shape::Mini
        }
    } else if c.w >= 180.0 {
        Shape::Medium
    } else {
        Shape::Small
    };
    out.hits.push(((0.0, 0.0, c.w, c.h), act.clone()));
    let (mx, my, ms) = mark_box(c, sh);
    mark(c, g, mx, my, ms, false, on.then_some(col));
    if label.is_empty() || sh == Shape::Mini {
        return;
    }
    // Labels are runtime strings here, so draw them directly with the face's geometry.
    match sh {
        Shape::Dock => {
            let k = dock_k(c);
            let x0 = mx + ms + style::SP[4] * k;
            let tw = c.w - style::M_DOCK * k - x0;
            let ls = fit(c, label, style::TITLE.size * k, style::TITLE.min * k, 600, Font::Text, tw);
            if sub.is_empty() {
                c.text_base(label, x0, 32.0 * k, ls, 600, c.text1(), Align::Left, c.w);
            } else {
                c.text_base(label, x0, 26.0 * k, ls, 600, c.text1(), Align::Left, c.w);
                c.text_base(sub, x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, tw);
            }
        }
        Shape::Small => {
            let inner = c.w - 2.0 * style::M_SMALL;
            let ls = fit(c, label, style::TITLE.size, style::TITLE.min, 600, Font::Text, inner);
            c.text_base(label, c.w / 2.0, SMALL_NAME_BASE, ls, 600, c.text1(), Align::Center, c.w);
            c.text_base(sub, c.w / 2.0, SMALL_STATE_BASE, style::FOOT.size, 400, c.text2(), Align::Center, inner);
        }
        _ => {
            let tw = c.w - style::M_CARD - MED_TEXT_X;
            let ls = fit(c, label, style::VALUE.size, style::VALUE.min, 600, Font::Text, tw);
            c.text_base(label, MED_TEXT_X, MED_NAME_BASE, ls, 600, c.text1(), Align::Left, c.w);
            c.text_base(sub, MED_TEXT_X, MED_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, tw);
        }
    }
}

/// The icon plate used by tiles: radius 0.25 × size, glyph 0.45 × size.
fn plate(c: &Canvas, g: &str, x: f32, y: f32, p: f32, fill: Col, ink: Col) {
    c.rr(x, y, p, p, p * style::BADGE_R, fill);
    c.glyph(g, x + p / 2.0, y + p / 2.0, p * style::GLYPH_IN, ink);
}

/// The Recycle Bin's item count and size, queried off the UI thread at most every 15 s.
fn bin_stats(sample: bool) -> Option<(i64, i64)> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    if sample {
        return Some((38, 214 * 1024 * 1024));
    }
    // (last query, result, a query running)
    static BIN: Mutex<(Option<Instant>, Option<(i64, i64)>, bool)> = Mutex::new((None, None, false));
    let mut g = BIN.lock().ok()?;
    let stale = g.0.map(|t| t.elapsed() > Duration::from_secs(15)).unwrap_or(true);
    if stale && !g.2 {
        g.2 = true;
        std::thread::spawn(|| {
            let mut info = windows::Win32::UI::Shell::SHQUERYRBINFO { cbSize: std::mem::size_of::<windows::Win32::UI::Shell::SHQUERYRBINFO>() as u32, ..Default::default() };
            let ok = unsafe { windows::Win32::UI::Shell::SHQueryRecycleBinW(windows::core::PCWSTR::null(), &mut info) }.is_ok();
            if let Ok(mut g) = BIN.lock() {
                let (n, size) = (info.i64NumItems, info.i64Size);
                *g = (Some(Instant::now()), ok.then_some((n, size)), false);
            }
        });
    }
    g.1
}

/// Bytes as "820 KB", "214 MB", "1.2 GB".
fn bytes_str(b: i64) -> String {
    let b = b.max(0) as f64;
    let (v, unit) = if b >= 1e9 {
        (b / 1073741824.0, "GB")
    } else if b >= 1e6 {
        (b / 1048576.0, "MB")
    } else {
        (b / 1024.0, "KB")
    };
    if v < 10.0 && unit == "GB" {
        format!("{v:.1} {unit}")
    } else {
        format!("{} {unit}", v.round().max(if b > 0.0 { 1.0 } else { 0.0 }) as i64)
    }
}

/// "1 item" / "38 items".
fn items_str(n: i64) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// An app's display name: no ".exe", title-cased when it was a bare process name.
fn app_display(raw: &str) -> String {
    let t = raw.trim();
    let stem = if t.to_ascii_lowercase().ends_with(".exe") { &t[..t.len() - 4] } else { t };
    // Drop a release channel or edition in brackets: "T3 Code (Nightly)" becomes "T3 Code".
    let stem = match stem.find(" (") {
        Some(i) if stem.ends_with(')') && i > 0 => &stem[..i],
        _ => stem,
    };
    match stem.to_ascii_lowercase().as_str() {
        "explorer" => return "File Explorer".into(),
        "msedge" => return "Microsoft Edge".into(),
        "code" => return "VS Code".into(),
        "windowsterminal" => return "Terminal".into(),
        _ => {}
    }
    if stem.chars().any(|ch| ch.is_uppercase()) {
        return stem.to_string();
    }
    stem.split(' ')
        .map(|w| {
            let mut ch = w.chars();
            match ch.next() {
                Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Today's apps by time, display names merged, longest first.
fn screen_apps(x: &Ctx) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = Vec::new();
    for (name, secs) in &x.hub.screen.apps {
        let n = app_display(name);
        match v.iter_mut().find(|(k, _)| *k == n) {
            Some(e) => e.1 += secs,
            None => v.push((n, *secs)),
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// Category colours for the top apps; everything else is "other" (neutral).
const APP_COLS: [Col; 3] = [BLUE, PURPLE, ORANGE];

/// A stacked share bar: the top apps in their colours, the rest neutral, over a track.
fn share_bar(c: &Canvas, apps: &[(String, u64)], total: u64, x: f32, y: f32, w: f32, h: f32) {
    c.rr(x, y, w, h, h / 2.0, c.track());
    if total == 0 {
        return;
    }
    let mut segs: Vec<(f32, Col)> = Vec::new();
    let mut acc = 0u64;
    for (i, (_, s)) in apps.iter().take(APP_COLS.len()).enumerate() {
        acc += s;
        segs.push((acc as f32 / total as f32, APP_COLS[i]));
    }
    if acc < total {
        segs.push((1.0, c.ink(0.32)));
    }
    // Longest first, each shorter one on top: every segment keeps a round end.
    for (end, col) in segs.iter().rev() {
        c.rr(x, y, (w * end).max(h), h, h / 2.0, *col);
    }
}

/// Hour-by-hour bars on a baseline: no bar for empty hours, the current hour solid, the
/// rest lighter. One hour of use (`full`) fills 86% of the height.
#[allow(clippy::too_many_arguments)]
fn hour_bars(c: &Canvas, vals: &[f64], now: usize, full: f64, x: f32, y: f32, w: f32, h: f32, bw: f32) {
    let n = vals.len().max(1);
    let slot = w / n as f32;
    let hi = vals.iter().cloned().fold(full, f64::max);
    let past = alpha(BLUE, if c.light { 0.5 } else { 0.6 });
    let r = (bw / 2.0).min(1.5);
    for (i, v) in vals.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let ht = ((v / hi) as f32 * h * 0.86).max(2.0);
        let bx = x + slot * i as f32 + (slot - bw) / 2.0;
        c.rr(bx, y + h - ht, bw, ht, r, if i == now { BLUE } else { past });
    }
    c.line(x, y + h + 0.5, x + w, y + h + 0.5, 1.0, c.sep());
}

/// Hour ticks under hour bars: Micro text3, each left-aligned to its hour's bar.
fn hour_ticks(c: &Canvas, marks: &[(usize, &str)], x: f32, w: f32, bw: f32, base: f32) {
    let slot = w / 24.0;
    for (h, l) in marks {
        let bx = x + slot * *h as f32 + (slot - bw) / 2.0;
        c.text_base(l, bx, base, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Left, 40.0);
    }
}

/// The 24 hourly values (seconds), today.
fn hours_of(x: &Ctx) -> Vec<f64> {
    (0..24).map(|h| *x.hub.screen.hours.get(h).unwrap_or(&0) as f64).collect()
}

/// A time like "8 AM".
fn hour_str(h: usize) -> String {
    let h = h as u32;
    format!("{} {}", h12(h), ampm(h))
}

/// Compact duration for a Mini: "46m", "6.8h", "12h".
fn dur_mini(secs: u64) -> String {
    let h = secs as f64 / 3600.0;
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if h < 10.0 {
        let s = format!("{h:.1}");
        format!("{}h", s.strip_suffix(".0").unwrap_or(&s))
    } else {
        format!("{}h", h.round() as i64)
    }
}

fn tool(c: &Canvas, out: &mut Painted, k: ToolKind, x: &Ctx, sh: Shape) {
    let open = |s: &str| WAct::Open(s.to_string());
    match k {
        ToolKind::Tray => tray(c, out, x, sh),
        ToolKind::Connectivity => connectivity(c, out, x, sh),
        ToolKind::ControlTiles => control_tiles(c, out, x, sh),
        ToolKind::Toggles => toggles(c, out, x, sh),
        ToolKind::WinUpdate => win_update(c, out, x, sh),
        ToolKind::WinSettings => {
            let mut f = ToolFace::new(G_SETTINGS, "Windows Settings", open("ms-settings:"));
            f.short = "Settings";
            f.small = "System and apps".into();
            f.medium = "System, apps, network and more".into();
            f.slot = ToolSlot::Key("Win + I");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + I")),
                row(G_UPDATE, "Windows Update", Trail::Go(open("ms-settings:windowsupdate"))),
                row(G_PALETTE, "Personalisation", Trail::Go(open("ms-settings:personalization"))),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::DarkMode => {
            let on = x.control.dark;
            let mut f = ToolFace::new(G_MOON, "Dark mode", WAct::Dark);
            let st = if on { "On" } else { "Off" };
            f.state = st.into();
            f.small = st.into();
            f.medium = st.into();
            f.round = true;
            f.fill = on.then(|| c.blue());
            f.slot = ToolSlot::Switch;
            f.slot_medium = true;
            f.rows = vec![
                row(G_MOON, "Apps and Windows", Trail::Text(if on { "Dark" } else { "Light" }.into())),
                row(G_PALETTE, "Colours", Trail::Go(open("ms-settings:colors"))),
                note_row("Switches apps and Windows together"),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Focus => {
            let mut f = ToolFace::new(G_FOCUS, "Focus", open("ms-settings:quiethours"));
            f.small = "Do not disturb".into();
            f.medium = "Do not disturb settings".into();
            f.slot = ToolSlot::Key("Win + N");
            f.rows = vec![
                row(G_DND, "Do not disturb", Trail::Go(open("ms-settings:quiethours"))),
                row(G_SETTINGS, "Notification settings", Trail::Go(open("ms-settings:notifications"))),
                row(G_KEYBOARD, "Notification centre", Trail::Key("Win + N")),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Notifications => {
            let mut f = ToolFace::new(G_BELL, "Notifications", open("ms-actioncenter:"));
            f.small = "Open centre".into();
            f.medium = "Open notification centre".into();
            f.slot = ToolSlot::Key("Win + N");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + N")),
                row(G_DND, "Do not disturb", Trail::Go(open("ms-settings:quiethours"))),
                row(G_SETTINGS, "Notification settings", Trail::Go(open("ms-settings:notifications"))),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::ShowDesktop => {
            let on = x.hub.desktop_shown;
            let mut f = ToolFace::new(G_DESKTOP, "Show desktop", WAct::ShowDesktop);
            f.short = "Desktop";
            f.state = if on { "Shown".into() } else { String::new() };
            f.small = if on { "Restore windows" } else { "Hide windows" }.into();
            f.medium = if on { "Bring windows back" } else { "Hide all windows" }.into();
            f.round = true;
            f.fill = on.then(|| c.blue());
            f.slot = ToolSlot::Key("Win + D");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + D")),
                row(G_EYE, "Peek at the desktop", Trail::Key("Win + ,")),
                note_row("Click again to bring your windows back"),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::KeepAwake => {
            let on = x.hub.keep_awake;
            let mut f = ToolFace::new(G_COFFEE, "Keep awake", WAct::KeepAwake);
            f.state = if on { "On" } else { "Off" }.into();
            f.small = if on { "PC stays awake" } else { "Off" }.into();
            f.medium = f.small.clone();
            f.round = true;
            f.fill = on.then(|| c.blue());
            f.slot = ToolSlot::Switch;
            f.slot_medium = true;
            f.rows = vec![
                row(G_SCREEN, "Screen", Trail::Text(if on { "Stays on" } else { "Can sleep" }.into())),
                row(G_POWER, "System sleep", Trail::Text(if on { "Paused" } else { "Normal" }.into())),
                row(G_SETTINGS, "Power settings", Trail::Go(open("ms-settings:powersleep"))),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Search => search(c, out, x, sh),
        ToolKind::Run => {
            let mut f = ToolFace::new(G_OPEN, "Run", open("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}"));
            f.small = "Open a command".into();
            f.medium = "Open a program, folder or file".into();
            f.slot = ToolSlot::Key("Win + R");
            f.rows = vec![
                row(G_KEYBOARD, "Shortcut", Trail::Key("Win + R")),
                row(G_RUN, "Run as admin", Trail::Key("Ctrl + Shift + Enter")),
                note_row("Type a program, folder or web address"),
            ];
            tool_face(c, out, &f, sh);
        }
        ToolKind::Trash => {
            let stats = bin_stats(x.sample);
            let mut f = ToolFace::new(G_TRASH, "Recycle Bin", open("shell:RecycleBinFolder"));
            match stats {
                Some((0, _)) => {
                    f.state = "Empty".into();
                    f.small = "Empty".into();
                    f.medium = "Empty".into();
                    f.slot = ToolSlot::Note("Nothing deleted".into());
                    f.rows = vec![row(G_TRASH, "Items", Trail::Text("None".into())), note_row("Deleted files wait here until you empty it")];
                }
                Some((n, b)) => {
                    f.state = items_str(n);
                    f.small = items_str(n);
                    f.medium = format!("{} \u{00B7} {}", items_str(n), bytes_str(b));
                    f.slot = ToolSlot::Value(bytes_str(b));
                    f.rows = vec![
                        row(G_TRASH, "Items", Trail::Text(n.to_string())),
                        row(G_DRIVE, "Size on disk", Trail::Text(bytes_str(b))),
                        note_row("Click to open; empty it from File Explorer"),
                    ];
                }
                None => {
                    f.small = "Click to open".into();
                    f.medium = "Click to open".into();
                    f.slot = ToolSlot::Note("Open".into());
                    f.rows = vec![row(G_TRASH, "Open Recycle Bin", Trail::Go(open("shell:RecycleBinFolder"))), note_row("Deleted files wait here until you empty it")];
                }
            }
            let (mx, my, ms) = tool_face(c, out, &f, sh);
            if sh == Shape::Mini && stats.map(|s| s.0 > 0).unwrap_or(false) {
                mark_dot(c, mx, my, ms, 6.0 * dock_k(c), c.text2());
            }
        }
        ToolKind::WhatsApp => {
            let (state, rows): (&str, Vec<TRow>) = match x.whatsapp {
                Some(true) => ("New messages", vec![row(G_POWER, "Status", Trail::Text("Running".into())), row(G_CHAT, "Messages", Trail::Text("New".into())), note_row("Click to open your chats")]),
                Some(false) => ("No new messages", vec![row(G_POWER, "Status", Trail::Text("Running".into())), row(G_CHAT, "Messages", Trail::Text("All read".into())), note_row("Click to open your chats")]),
                None => ("Not running", vec![row(G_POWER, "Status", Trail::Text("Not running".into())), note_row("Click to start WhatsApp")]),
            };
            let mut f = ToolFace::new(G_CHAT, "WhatsApp", WAct::WhatsApp);
            // The dock has room for a shorter state only.
            f.state = if x.whatsapp == Some(false) { "All read".into() } else { state.into() };
            f.small = state.into();
            f.medium = state.into();
            f.fill = Some(WHATSAPP);
            f.slot = match x.whatsapp {
                Some(true) => ToolSlot::Capsule("New", WHATSAPP),
                Some(false) => ToolSlot::Note("Running".into()),
                None => ToolSlot::Note("Click to open".into()),
            };
            f.rows = rows;
            let (mx, my, ms) = tool_face(c, out, &f, sh);
            if x.whatsapp == Some(true) {
                let d = if matches!(sh, Shape::Mini | Shape::Dock) { 7.0 * dock_k(c) } else { 8.0 };
                mark_dot(c, mx, my, ms, d, RED);
            }
        }
        ToolKind::Screenshot => screenshot(c, out, sh),
        ToolKind::Apps => apps_grid(c, out, x, sh),
        ToolKind::Notes => notes(c, out, x, sh),
        ToolKind::ScreenTime => screen_time(c, x, sh),
        ToolKind::Timeline => timeline(c, x, sh),
    }
}

/// Search: a plate on the dock and Small; a search field on Medium; Large adds pinned apps.
fn search(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    if !matches!(sh, Shape::Medium | Shape::Large) {
        let mut f = ToolFace::new(G_SEARCH, "Search", WAct::Search);
        f.small = "Apps and files".into();
        f.slot = ToolSlot::Key("Win + S");
        tool_face(c, out, &f, sh);
        return;
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Search));
    let m = style::M_CARD;
    let (py, ph) = (28.0, 40.0);
    c.rr(m, py, c.w - 2.0 * m, ph, ph / 2.0, c.plate());
    let cy = py + ph / 2.0;
    c.glyph(G_SEARCH, 38.0, cy, 14.0, c.text2());
    let kw = keycap(c, "Win + S", c.w - m - style::SP[4], cy, 1.0);
    let tx = 58.0;
    let tmax = c.w - m - style::SP[4] - kw - style::SP[3] - tx;
    // The longest prompt that fits whole.
    let prompts = ["Search apps, files and settings", "Search apps and files", "Search"];
    let p = prompts.iter().find(|p| c.measure(p, style::BODY.size, 400, Font::Text).0 <= tmax).unwrap_or(&"Search");
    c.text_base(p, tx, cy + c.cap_h(style::BODY.size) / 2.0, style::BODY.size, 400, c.text2(), Align::Left, tmax);
    if sh != Shape::Large {
        return;
    }
    separator(c, style::SEP_Y_LARGE);
    c.text_base("PINNED", m, 116.0, style::MICRO.size, style::MICRO.weight, c.text3(), Align::Left, c.w);
    let cw = (c.w - 2.0 * m) / 4.0;
    let s = MARK_CARD;
    let top = 124.0;
    if x.pinned.is_empty() {
        c.text_base("Pin apps in Launchpad to see them here", m, top + 26.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
        return;
    }
    for (i, id) in x.pinned.iter().take(4).enumerate() {
        let cx = m + cw * (i as f32 + 0.5);
        out.icons.push((cx - s / 2.0, top, s, id.clone()));
        out.hits.push(((cx - cw / 2.0, top - 4.0, cw, c.h - style::M_CARD - top + 8.0), WAct::App(id.clone())));
        let name = x.names.get(id).cloned().unwrap_or_default();
        c.text_base(&name, cx, 184.0, style::FOOT.size, 400, c.text2(), Align::Center, cw - 4.0);
    }
}

/// Screenshot: Region (the primary action, blue), Window and Full screen.
fn screenshot(c: &Canvas, out: &mut Painted, sh: Shape) {
    let items: [(&str, &str, &str); 4] = [
        (G_REGION, "Region", "ms-screenclip:?clippingMode=Rectangle"),
        (G_WINDOW, "Window", "ms-screenclip:?clippingMode=Window"),
        (G_FULL, "Full screen", "ms-screenclip:?clippingMode=Fullscreen"),
        (G_FOLDER, "Screenshots", "shell:Screenshots"),
    ];
    let fill = |i: usize| (i == 0).then(|| c.blue());
    match sh {
        Shape::Mini => {
            let (mx, my, ms) = mark_box(c, sh);
            mark(c, items[0].0, mx, my, ms, false, fill(0));
            out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Open(items[0].2.into())));
        }
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let s = style::BADGE_DOCK * k;
            let n = if dock_wide(c) { 4 } else { 3 };
            let gap = (c.w - 2.0 * m - n as f32 * s) / (n - 1) as f32;
            for (i, (g, _, uri)) in items.iter().take(n).enumerate() {
                let x0 = m + i as f32 * (s + gap);
                mark(c, g, x0, (c.h - s) / 2.0, s, false, fill(i));
                out.hits.push(((x0 - gap / 2.0, 0.0, s + gap, c.h), WAct::Open((*uri).into())));
            }
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let (rh, ps) = (28.0, 24.0);
            for (i, (g, l, uri)) in items.iter().take(3).enumerate() {
                let y = m + i as f32 * (rh + style::SP[1]);
                let cy = y + rh / 2.0;
                mark(c, g, m, cy - ps / 2.0, ps, false, fill(i));
                let tx = m + ps + style::SP[3];
                c.text_base(l, tx, cy + c.cap_h(style::BODY.size) / 2.0, style::BODY.size, 500, c.text1(), Align::Left, c.w - m - tx);
                out.hits.push(((0.0, y - style::SP[0], c.w, rh + style::SP[1]), WAct::Open((*uri).into())));
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let cw = (c.w - 2.0 * m) / 3.0;
            let s = MARK_CARD;
            for (i, (g, l, uri)) in items.iter().take(3).enumerate() {
                let cx = m + cw * (i as f32 + 0.5);
                mark(c, g, cx - s / 2.0, m, s, false, fill(i));
                c.text_base(l, cx, 76.0, style::FOOT.size, 500, c.text2(), Align::Center, cw);
                out.hits.push(((cx - cw / 2.0, 0.0, cw, style::SEP_Y_LARGE), WAct::Open((*uri).into())));
            }
            if sh == Shape::Large {
                tool_rows(
                    c,
                    out,
                    &[
                        row(G_FOLDER, "Screenshots folder", Trail::Go(WAct::Open("shell:Screenshots".into()))),
                        row(G_CUT, "Snipping Tool", Trail::Go(WAct::Open("Microsoft.ScreenSketch_8wekyb3d8bbwe!App".into()))),
                        row(G_KEYBOARD, "Snip from anywhere", Trail::Key("Win + Shift + S")),
                    ],
                );
            }
        }
    }
}

/// A placeholder app tile; the first one carries a "+" that opens Launchpad to pin apps.
fn app_slot(c: &Canvas, out: &mut Painted, x: f32, y: f32, s: f32, plus: bool) {
    c.rr(x, y, s, s, s * 10.0 / MARK_CARD, c.plate());
    if plus {
        c.glyph(G_ADD, x + s / 2.0, y + s / 2.0, s * 16.0 / MARK_CARD, c.text3());
        out.hits.push(((x, y, s, s), WAct::Search));
    }
}

/// Apps: the Launchpad pins on a 44 / 12 grid; empty slots are placeholders.
fn apps_grid(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let pins = x.pinned;
    let gap = 12.0;
    // (cols, rows, s, left, top) of the grid; Medium is one row of four 59-wide columns.
    let (cols, rows, s, left, top) = match sh {
        Shape::Mini => {
            let s = MARK_MINI * dock_k(c);
            (1, 1, s, (c.w - s) / 2.0, (c.h - s) / 2.0)
        }
        Shape::Dock => {
            let k = dock_k(c);
            let s = MARK_MINI * k;
            let g = style::SP[3] * k;
            let n = (((c.w - 2.0 * style::M_MINI * k + g) / (s + g)).floor() as usize).max(1);
            // The pins and one "+" at most: the dock never shows a run of empty tiles.
            let n = n.min(pins.len() + 1);
            let w = n as f32 * s + (n - 1) as f32 * g;
            let left = if pins.is_empty() { style::M_MINI * k } else { (c.w - w) / 2.0 };
            for i in 0..n {
                let ix = left + i as f32 * (s + g);
                match pins.get(i) {
                    Some(id) => {
                        out.icons.push((ix, (c.h - s) / 2.0, s, id.clone()));
                        out.hits.push(((ix - g / 2.0, 0.0, s + g, c.h), WAct::App(id.clone())));
                    }
                    None => app_slot(c, out, ix, (c.h - s) / 2.0, s, i == pins.len()),
                }
            }
            if pins.is_empty() {
                let x0 = left + s + style::SP[4] * k;
                c.text_base("Pin apps", x0, 26.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, c.w - x0 - style::M_DOCK * k);
                c.text_base("In Launchpad", x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, c.w - x0 - style::M_DOCK * k);
            }
            return;
        }
        Shape::Small => {
            if pins.is_empty() {
                (2, 1, MARK_CARD, style::M_SMALL, SMALL_MARK_Y)
            } else {
                (2, 2, MARK_CARD, style::M_SMALL, 10.0)
            }
        }
        Shape::Medium => {
            if pins.is_empty() {
                (2, 1, MARK_CARD, style::M_CARD, MED_MARK_Y)
            } else {
                // Four columns, each tile centred in its 59-wide column, with names.
                let m = style::M_CARD;
                let cw = (c.w - 2.0 * m) / 4.0;
                for i in 0..4 {
                    let cx = m + cw * (i as f32 + 0.5);
                    let ix = cx - MARK_CARD / 2.0;
                    match pins.get(i) {
                        Some(id) => {
                            out.icons.push((ix, m, MARK_CARD, id.clone()));
                            out.hits.push(((cx - cw / 2.0, 0.0, cw, c.h), WAct::App(id.clone())));
                            let name = x.names.get(id).cloned().unwrap_or_default();
                            c.text_base(&name, cx, 76.0, style::FOOT.size, 400, c.text2(), Align::Center, cw - 4.0);
                        }
                        None => app_slot(c, out, ix, m, MARK_CARD, i == pins.len()),
                    }
                }
                return;
            }
        }
        Shape::Large => {
            let rows = if pins.is_empty() { 2 } else { 3 };
            let w = 4.0 * MARK_CARD + 3.0 * gap;
            let h = rows as f32 * MARK_CARD + (rows - 1) as f32 * gap;
            let top = if pins.is_empty() { 24.0 } else { (c.h - h) / 2.0 };
            (4, rows, MARK_CARD, (c.w - w) / 2.0, top)
        }
    };
    for i in 0..cols * rows {
        let (ix, iy) = (left + (i % cols) as f32 * (s + gap), top + (i / cols) as f32 * (s + gap));
        match pins.get(i) {
            Some(id) => {
                out.icons.push((ix, iy, s, id.clone()));
                out.hits.push(((ix - gap / 2.0, iy - gap / 2.0, s + gap, s + gap), WAct::App(id.clone())));
            }
            None => app_slot(c, out, ix, iy, s, i == pins.len()),
        }
    }
    if !pins.is_empty() {
        return;
    }
    // No pins: say how to add them, under (or beside) the placeholders.
    match sh {
        Shape::Small => {
            c.text_base("Pin apps", c.w / 2.0, SMALL_NAME_BASE, style::TITLE.size, 600, c.text1(), Align::Center, c.w);
            c.text_base("In Launchpad", c.w / 2.0, SMALL_STATE_BASE, style::FOOT.size, 400, c.text2(), Align::Center, c.w);
        }
        Shape::Medium => {
            let tx = left + 2.0 * s + gap + style::SP[6];
            c.text_base("Pin apps", tx, MED_NAME_BASE, style::VALUE.size, 600, c.text1(), Align::Left, c.w - style::M_CARD - tx);
            c.text_base("In Launchpad", tx, MED_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - style::M_CARD - tx);
        }
        Shape::Large => {
            c.text_base("No apps pinned", c.w / 2.0, 154.0, style::TITLE.size, 600, c.text1(), Align::Center, c.w);
            c.text_base("Pin apps in Launchpad to see them here", c.w / 2.0, 172.0, style::CAPTION.size, 400, c.text2(), Align::Center, c.w - 2.0 * style::M_CARD);
        }
        _ => {}
    }
}

/// The Notes tile: yellow, with a dark glyph, the same in both themes.
fn note_tile(c: &Canvas, x: f32, y: f32, s: f32) {
    c.rr(x, y, s, s, s * style::BADGE_R, c.yellow());
    c.glyph(G_NOTE, x + s / 2.0, y + s / 2.0, s * 0.5, alpha(hex(0x1C1C1E), 0.85));
}

/// Notes: the first line of your notes file and what follows it.
fn notes(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    out.hits.push(((0.0, 0.0, c.w, c.h), WAct::Notes));
    let lines: Vec<&str> = x.hub.notes.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let empty = lines.is_empty();
    match sh {
        Shape::Mini => {
            let (mx, my, ms) = mark_box(c, sh);
            note_tile(c, mx, my, ms);
        }
        Shape::Dock => {
            let k = dock_k(c);
            let (mx, my, ms) = mark_box(c, sh);
            note_tile(c, mx, my, ms);
            let m = style::M_DOCK * k;
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                let slot = if empty { ToolSlot::Note("New note".into()) } else { ToolSlot::Note(if lines.len() == 1 { "1 line".into() } else { format!("{} lines", lines.len()) }) };
                if let Some(w) = tool_slot(c, &slot, c.w - m, 27.0 * k, k, false) {
                    right -= w + style::SP[5] * k;
                }
            }
            let (first, second) = if empty { ("Notes", "Click to write") } else { (lines[0], lines.get(1).copied().unwrap_or("Notes")) };
            c.text_base(first, x0, 26.0 * k, style::TITLE.size * k, 600, c.text1(), Align::Left, right - x0);
            c.text_base(second, x0, 40.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            note_tile(c, m, m, style::BADGE_SMALL);
            let inner = c.w - 2.0 * m;
            if empty {
                c.text_base("No notes", m, SMALL_NAME_BASE, style::TITLE.size, 600, c.text1(), Align::Left, inner);
                c.text_base("Click to write", m, SMALL_STATE_BASE, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
                return;
            }
            c.text_cap(lines[0], m, 52.0, style::TITLE.size, 600, c.text1(), Align::Left, inner);
            let first = 52.0 + c.cap_h(style::TITLE.size) + style::ROW_S - 2.0;
            if lines.len() == 1 {
                c.text_base("Click to add more", m, first, style::CAPTION.size, 400, c.text3(), Align::Left, inner);
            }
            for (i, l) in lines.iter().skip(1).enumerate() {
                let base = first + i as f32 * 16.0;
                if base > c.h - m {
                    break;
                }
                c.text_base(l, m, base, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            if empty && sh == Shape::Large {
                let s = 48.0;
                note_tile(c, (c.w - s) / 2.0, 36.0, s);
                c.text_base("No notes yet", c.w / 2.0, 108.0, 15.0, 600, c.text1(), Align::Center, c.w);
                c.text_base("Click to write a note", c.w / 2.0, 128.0, style::CAPTION.size, 400, c.text2(), Align::Center, c.w - 2.0 * m);
                let (bw, bh) = (132.0, 30.0);
                pill_button(c, out, (c.w - bw) / 2.0, 150.0, bw, bh, "New note", WAct::Notes);
                return;
            }
            if empty {
                note_tile(c, m, m, style::BADGE_GLANCE);
                c.text_cap("No notes", m, 50.0, 15.0, 600, c.text1(), Align::Left, c.w - 2.0 * m);
                c.text_base("Click to write a note", m, 76.0, style::CAPTION.size, 400, c.text2(), Align::Left, c.w - 2.0 * m);
                return;
            }
            let bs = style::BADGE_HEADER;
            note_tile(c, m, m, bs);
            let tx = m + bs + style::SP[3];
            c.text_base(lines[0], tx, style::HEADER_BASE, style::TITLE.size, 600, c.text1(), Align::Left, c.w - m - tx);
            let (pitch, last) = if sh == Shape::Large { (18.0, c.h - m) } else { (18.0, 80.0) };
            let fits = (((last - 56.0) / pitch).floor() as usize) + 1;
            let body: Vec<&&str> = lines.iter().skip(1).collect();
            if body.is_empty() {
                c.text_base("Click to add more", m, 56.0, style::BODY.size, 400, c.text3(), Align::Left, c.w - 2.0 * m);
            }
            for (i, l) in body.iter().take(fits).enumerate() {
                // More lines than fit: the last one fades instead of stopping short.
                let col = if i + 1 == fits && body.len() > fits { c.text3() } else { c.text2() };
                c.text_base(l, m, 56.0 + i as f32 * pitch, style::BODY.size, 400, col, Align::Left, c.w - 2.0 * m);
            }
        }
    }
}

/// Screen Time: today's total and the apps it went to (Large adds when).
fn screen_time(c: &Canvas, x: &Ctx, sh: Shape) {
    let apps = screen_apps(x);
    let total: u64 = apps.iter().map(|a| a.1).sum();
    let none = total == 0;
    let value = if none { "0m".to_string() } else { dur(total) };
    match sh {
        Shape::Mini => {
            let k = dock_k(c);
            c.glyph(G_SCREEN, c.w / 2.0, (8.0 + 9.0) * k, 18.0 * k, BLUE);
            let role = Type { size: style::MINI_VALUE.size * k, ..style::MINI_VALUE };
            let (v, fs) = fit_value(c, &dur_mini(total), role, c.w - 2.0 * style::M_MINI * k);
            c.text_base(&v, c.w / 2.0, 45.0 * k, fs, role.weight, c.text1(), Align::Center, c.w * 2.0);
        }
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let (mx, my, ms) = mark_box(c, sh);
            plate(c, G_SCREEN, mx, my, ms, c.plate(), BLUE);
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                let sw = (c.w * 0.34).round();
                let sx = c.w - m - sw;
                if let Some((name, secs)) = apps.first() {
                    let pct = format!("{:.0}%", *secs as f64 * 100.0 / total.max(1) as f64);
                    let pw = c.text_base(&pct, c.w - m, 23.0 * k, style::FOOT.size * k, 500, c.text3(), Align::Right, sw);
                    c.text_base(name, sx, 23.0 * k, style::FOOT.size * k, 500, c.text2(), Align::Left, sw - pw - style::SP[1] * k);
                }
                share_bar(c, &apps, total, sx, 29.0 * k, sw, style::METER_H_COMPACT * k);
                right = sx - style::SP[5] * k;
            }
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let (v, fs) = fit_value(c, &value, role, right - x0);
            c.text_base(&v, x0, 30.0 * k, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Today", x0, 44.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let inner = c.w - 2.0 * m;
            let (v, fs) = fit_value(c, &value, style::HERO_S, inner);
            c.text_cap(&v, m, m, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(if none { "Starting now" } else { "Today" }, m, 54.0, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
            share_bar(c, &apps, total, m, 64.0, inner, style::METER_H);
            for (i, (name, secs)) in apps.iter().take(2).enumerate() {
                let base = 88.0 + i as f32 * style::ROW_S;
                let cy = base - c.cap_h(style::FOOT.size) / 2.0;
                c.circle(m + 3.0, cy, 3.0, APP_COLS[i]);
                let tw = c.text_base(&dur(*secs), c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, inner);
                let nx = m + 10.0;
                c.text_base(name, nx, base, style::FOOT.size, 500, c.text1(), Align::Left, c.w - m - tw - style::SP[2] - nx);
            }
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let lw = 132.0 - m;
            let (v, fs) = fit_value(c, &value, style::HERO, lw);
            c.text_cap(&v, m, m, fs, style::HERO.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base(if none { "Counting from now" } else { "Screen time today" }, m, 58.0, style::CAPTION.size, 400, c.text2(), Align::Left, lw);
            share_bar(c, &apps, total, m, 70.0, lw, style::METER_H);
            let (rx, rw) = (148.0, c.w - m - 148.0);
            let first = m + c.cap_h(style::FOOT.size);
            if apps.is_empty() {
                c.text_base("No apps yet", rx, first, style::FOOT.size, 400, c.text3(), Align::Left, rw);
            }
            for (i, (name, secs)) in apps.iter().take(3).enumerate() {
                let base = first + i as f32 * style::ROW_L;
                let tw = c.text_base(&dur(*secs), c.w - m, base, style::FOOT.size, 500, c.text2(), Align::Right, rw);
                c.text_base(name, rx, base, style::FOOT.size, 500, c.text1(), Align::Left, rw - tw - style::SP[2]);
                c.bar(rx, base + 5.0, rw, 3.0, *secs as f32 / total.max(1) as f32, APP_COLS[i]);
            }
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let vals = hours_of(x);
                let w = c.w - 2.0 * m;
                let bw = (w / 24.0 * 0.6).round();
                hour_bars(c, &vals, x.now.hour as usize, 3600.0, m, style::DETAIL_TOP, w, 64.0, bw);
                hour_ticks(c, &[(0, "12a"), (6, "6a"), (12, "12p"), (18, "6p")], m, w, bw, 186.0);
            }
        }
    }
}

/// Day Timeline: when you used the PC today, hour by hour.
fn timeline(c: &Canvas, x: &Ctx, sh: Shape) {
    let vals = hours_of(x);
    let total = x.hub.screen.total();
    let now = x.now.hour as usize;
    let value = if total == 0 { "0m".to_string() } else { dur(total) };
    let ticks4: [(usize, &str); 4] = [(0, "12a"), (6, "6a"), (12, "12p"), (18, "6p")];
    match sh {
        Shape::Mini => {
            // Twelve two-hour buckets, drawn as the icon.
            let k = dock_k(c);
            let b: Vec<f64> = (0..12).map(|i| vals[2 * i] + vals[2 * i + 1]).collect();
            let (w, h) = (40.0 * k, 24.0 * k);
            if total == 0 {
                c.glyph(G_CLOCK, c.w / 2.0, c.h / 2.0, 22.0 * k, BLUE);
                return;
            }
            hour_bars(c, &b, now / 2, 7200.0, (c.w - w) / 2.0, (c.h - h) / 2.0, w, h, 2.5 * k);
        }
        Shape::Dock => {
            let k = dock_k(c);
            let m = style::M_DOCK * k;
            let (mx, my, ms) = mark_box(c, sh);
            plate(c, G_CLOCK, mx, my, ms, c.plate(), BLUE);
            let x0 = mx + ms + style::SP[4] * k;
            let mut right = c.w - m;
            if dock_wide(c) {
                let sw = (c.w * 0.34).round();
                let sx = c.w - m - sw;
                hour_bars(c, &vals, now, 3600.0, sx, 14.0 * k, sw, 26.0 * k, (sw / 24.0 * 0.7).max(1.0));
                right = sx - style::SP[5] * k;
            }
            let role = Type { size: style::VALUE.size * k, ..style::VALUE };
            let (v, fs) = fit_value(c, &value, role, right - x0);
            c.text_base(&v, x0, 30.0 * k, fs, role.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Today", x0, 44.0 * k, style::FOOT.size * k, 400, c.text2(), Align::Left, right - x0);
        }
        Shape::Small => {
            let m = style::M_SMALL;
            let inner = c.w - 2.0 * m;
            let (v, fs) = fit_value(c, &value, style::HERO_S, inner);
            c.text_cap(&v, m, m, fs, style::HERO_S.weight, c.text1(), Align::Left, c.w * 2.0);
            c.text_base("Today", m, 54.0, style::CAPTION.size, 400, c.text2(), Align::Left, inner);
            let bw = 3.0;
            hour_bars(c, &vals, now, 3600.0, m, 64.0, inner, 28.0, bw);
            hour_ticks(c, &[(0, "12a"), (12, "12p")], m, inner, bw, 106.0);
        }
        Shape::Medium | Shape::Large => {
            let m = style::M_CARD;
            let badge = super::canvas::gly(G_CLOCK, 0x0A84FF, 0xFFFFFF);
            header(c, &badge, "Today", Some((value.as_str(), c.text1())));
            let w = c.w - 2.0 * m;
            let bw = (w / 24.0 * 0.6).round();
            hour_bars(c, &vals, now, 3600.0, m, style::CONTENT_TOP, w, 22.0, bw);
            hour_ticks(c, &ticks4, m, w, bw, 79.0);
            if sh == Shape::Large {
                separator(c, style::SEP_Y_LARGE);
                let used: Vec<usize> = (0..24).filter(|h| vals[*h] > 0.0).collect();
                match (used.first(), used.is_empty()) {
                    (Some(first), false) => {
                        let busiest = (0..24).max_by(|a, b| vals[*a].partial_cmp(&vals[*b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
                        let (a, b, n) = (hour_str(*first), hour_str(busiest), format!("{}h", used.len()));
                        stat_footer(c, [(&a, "First use"), (&b, "Busiest hour"), (&n, "Active hours")], 132.0);
                    }
                    _ => {
                        c.text_base("Counting from now", c.w / 2.0, 152.0, style::CAPTION.size, 400, c.text2(), Align::Center, c.w - 2.0 * m);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Control Center

fn round_toggle(c: &Canvas, cx: f32, cy: f32, r: f32, g: &str, on: bool) {
    c.circle(cx, cy, r, if on { BLUE } else { c.ink(0.12) });
    c.glyph(g, cx, cy, r * 0.8, if on { WHITE } else { c.text1() });
}

/// Wi-Fi and Bluetooth: the round button switches the radio; the name opens its list.
fn connectivity(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let rows = [
        (G_WIFI, "Wi-Fi", ctl.wifi, WAct::Wifi, WAct::WifiMenu),
        (G_BT, "Bluetooth", ctl.bluetooth, WAct::Bluetooth, WAct::Open("ms-settings:bluetooth".into())),
    ];
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let slot = c.w / 2.0;
        for (i, (g, _, on, act, _)) in rows.into_iter().enumerate() {
            let cx = slot * (i as f32 + 0.5);
            round_toggle(c, cx, c.h / 2.0, (c.h * 0.32).min(slot * 0.42), g, on == Some(true));
            out.hits.push(((slot * i as f32, 0.0, slot, c.h), act));
        }
        return;
    }
    let rh = (c.h - 16.0) / rows.len() as f32;
    let r = if sh == Shape::Large { 22.0 } else { 18.0 };
    for (i, (g, name, on, act, more)) in rows.into_iter().enumerate() {
        let cy = 8.0 + rh * (i as f32 + 0.5);
        let cx = 14.0 + r;
        round_toggle(c, cx, cy, r, g, on == Some(true));
        let tx = cx + r + 10.0;
        let state = match on {
            Some(true) => "On",
            Some(false) => "Off",
            None => "Unavailable",
        };
        c.tw(name, tx, cy - 17.0, 12.5, 600, c.text1(), c.w - tx - 22.0);
        c.tw(state, tx, cy + 1.0, 11.0, 400, c.text2(), c.w - tx - 22.0);
        c.glyph(G_CHEV, c.w - 14.0, cy, 9.0, c.text3());
        out.hits.push(((tx - 4.0, cy - rh / 2.0, c.w - tx + 4.0, rh), more));
        if on.is_some() {
            out.hits.push(((cx - r - 4.0, cy - r - 4.0, 2.0 * r + 8.0, 2.0 * r + 8.0), act));
        }
    }
}

/// A grid of control buttons, 2 or 4 across.
fn button_grid(c: &Canvas, out: &mut Painted, items: Vec<(&'static str, &'static str, bool, WAct)>, sh: Shape) {
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if sh == Shape::Mini {
        // Just the first one.
        if let Some((g, _, on, act)) = items.into_iter().next() {
            tile(c, out, g, "", on, BLUE, act, true);
        }
        return;
    }
    // Small and Large cards: two columns (Large would otherwise stretch four tiles into
    // tall bars); Medium and the dock: one row.
    let cols = if matches!(sh, Shape::Small | Shape::Large) { 2 } else { items.len().min(4) };
    let rows = items.len().div_ceil(cols);
    let pad = if dense { 4.0 } else { 10.0 };
    let gap = if dense { 4.0 } else { 8.0 };
    let tw = (c.w - 2.0 * pad - gap * (cols - 1) as f32) / cols as f32;
    // Never taller than a little over square; the grid is centred in what's left.
    let th = ((c.h - 2.0 * pad - gap * (rows - 1) as f32) / rows as f32).min(tw * 1.15);
    let top = (c.h - th * rows as f32 - gap * (rows - 1) as f32) / 2.0;
    for (i, (g, label, on, act)) in items.into_iter().enumerate() {
        let (tx, ty) = (pad + (i % cols) as f32 * (tw + gap), top + (i / cols) as f32 * (th + gap));
        c.rr(tx, ty, tw, th, (th.min(tw) * 0.28).min(16.0), if on { BLUE } else { c.ink(0.08) });
        let ink = if on { WHITE } else { c.text1() };
        if dense || th < 50.0 {
            c.glyph(g, tx + tw / 2.0, ty + th / 2.0, (th.min(tw) * 0.42).min(18.0), ink);
        } else {
            c.glyph(g, tx + tw / 2.0, ty + th * 0.4, 18.0, ink);
            // A narrow tile gets the first word ("Dark" for "Dark mode") rather than edge-to-edge text.
            let label = if c.measure(label, 10.5, 500, Font::Text).0 > tw - 12.0 { label.split_whitespace().next().unwrap_or(label) } else { label };
            c.tc(label, tx + tw / 2.0, ty + th * 0.4 + 16.0, 10.5, 500, if on { (1.0, 1.0, 1.0, 0.9) } else { c.text2() }, tw);
        }
        out.hits.push(((tx, ty, tw, th), act));
    }
}

/// Dark mode, mute, snip and lock.
fn control_tiles(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let items = vec![
        (G_MOON, "Dark mode", ctl.dark, WAct::Dark),
        (if ctl.muted == Some(true) { G_MUTE } else { G_VOL }, "Mute", ctl.muted == Some(true), WAct::Mute),
        (G_SNIP, "Snip", false, WAct::Snip),
        (G_LOCK, "Lock", false, WAct::Lock),
    ];
    button_grid(c, out, items, sh);
}

/// Wi-Fi, Bluetooth, dark mode and mute.
fn toggles(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let ctl = x.control;
    let items = vec![
        (G_WIFI, "Wi-Fi", ctl.wifi == Some(true), WAct::Wifi),
        (G_BT, "Bluetooth", ctl.bluetooth == Some(true), WAct::Bluetooth),
        (G_MOON, "Dark mode", ctl.dark, WAct::Dark),
        (if ctl.muted == Some(true) { G_MUTE } else { G_VOL }, "Mute", ctl.muted == Some(true), WAct::Mute),
    ];
    button_grid(c, out, items, sh);
}

/// Background Apps: the tray icons in a grid (the host draws the pictures).
fn tray(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    let n = x.tray.len();
    if matches!(sh, Shape::Mini | Shape::Dock) {
        let s = (c.h * 0.42).round();
        let slot = s + 6.0;
        let fit_n = (((c.w - 8.0) / slot).floor() as usize).max(1).min(n);
        for (i, (key, _)) in x.tray.iter().take(fit_n).enumerate() {
            let ix = (c.w - fit_n as f32 * slot) / 2.0 + i as f32 * slot + 3.0;
            out.icons.push((ix, (c.h - s) / 2.0, s, format!("tray:{key}")));
            out.hits.push(((ix - 3.0, 0.0, slot, c.h), WAct::Tray(*key)));
        }
        if n == 0 {
            c.glyph(G_UP, c.w / 2.0, c.h / 2.0, c.h * 0.3, c.text2());
        }
        return;
    }
    let pad = 14.0;
    c.tw("Background Apps", pad, 12.0, 12.5, 600, c.text1(), c.w - 60.0);
    c.tr(&n.to_string(), c.w - pad, 13.0, 11.5, 400, c.text3());
    let head = 36.0;
    let foot = if sh == Shape::Small { 6.0 } else { 24.0 };
    let cols = ((c.w - 2.0 * pad) / 40.0).floor().max(1.0) as usize;
    let tile_w = (c.w - 2.0 * pad) / cols as f32;
    let rows = (((c.h - head - foot) / tile_w).floor() as usize).max(1);
    let isz = (tile_w * 0.5).min(22.0);
    for (i, (key, _)) in x.tray.iter().take(cols * rows).enumerate() {
        let (tx, ty) = (pad + (i % cols) as f32 * tile_w, head + (i / cols) as f32 * tile_w);
        if x.hover_tray == Some(*key) {
            c.rr(tx + 2.0, ty + 2.0, tile_w - 4.0, tile_w - 4.0, 10.0, c.ink(0.10));
        }
        out.icons.push((tx + (tile_w - isz) / 2.0, ty + (tile_w - isz) / 2.0, isz, format!("tray:{key}")));
        out.hits.push(((tx, ty, tile_w, tile_w), WAct::Tray(*key)));
    }
    if sh != Shape::Small {
        let hint = match x.hover_tray.and_then(|k| x.tray.iter().find(|t| t.0 == k)) {
            Some((_, tip)) if !tip.is_empty() => tip.lines().next().unwrap_or("").to_string(),
            _ if n == 0 => "No background apps".to_string(),
            _ => "Click to open · right-click for the app’s menu".to_string(),
        };
        c.tw(&hint, pad, c.h - foot + 2.0, 11.0, 400, c.text3(), c.w - 2.0 * pad);
    }
}

fn win_update(c: &Canvas, out: &mut Painted, x: &Ctx, sh: Shape) {
    use laminar_sys::update::UpdateStatus as U;
    let (sub, restart, attention) = match x.update {
        None => ("Checking…".to_string(), false, false),
        Some(U::RestartRequired) => ("Restart to finish installing updates".to_string(), true, true),
        Some(U::Pending(1)) => ("1 update waiting".to_string(), false, true),
        Some(U::Pending(n)) => (format!("{n} updates waiting"), false, true),
        Some(U::UpToDate) => ("You’re up to date".to_string(), false, false),
        Some(U::Unknown) => ("Status unavailable".to_string(), false, false),
    };
    let open = WAct::Open("ms-settings:windowsupdate".into());
    let dense = matches!(sh, Shape::Mini | Shape::Dock);
    if dense || sh == Shape::Small {
        tile_sub(c, out, G_UPDATE, if restart { "Restart" } else { "Updates" }, &sub, attention, BLUE, if restart { WAct::Power } else { open }, dense);
        if attention {
            let (cx, cy, p) = plate_center(c, dense);
            c.circle(cx + p * 0.42, cy - p * 0.42, (p * 0.12).max(4.0), RED);
        }
        return;
    }
    out.hits.push(((0.0, 0.0, c.w, c.h), open));
    let p = 48.0;
    plate(c, G_UPDATE, 18.0, (c.h - p) / 2.0, p, if attention { BLUE } else { c.ink(0.12) }, if attention { WHITE } else { c.text1() });
    let tx = 18.0 + p + 14.0;
    let bw = if restart { 78.0 } else { 0.0 };
    c.tw("Windows Update", tx, c.h / 2.0 - 19.0, 14.0, 600, c.text1(), c.w - tx - bw - 20.0);
    c.tw(&sub, tx, c.h / 2.0 + 2.0, 11.5, 400, c.text2(), c.w - tx - bw - 20.0);
    if restart {
        let (bx, bh) = (c.w - bw - 14.0, 28.0);
        let by = (c.h - bh) / 2.0;
        c.rr(bx, by, bw, bh, bh / 2.0, BLUE);
        c.tc("Restart", bx + bw / 2.0, by + 6.0, 11.5, 600, WHITE, bw);
        out.hits.push(((bx, by, bw, bh), WAct::Power));
    }
}

// ------------------------------------------------------------------ progress

fn progress(c: &Canvas, s: Span, x: &Ctx, sh: Shape) {
    let n = x.now;
    let (total, done, label, frac) = match s {
        Span::Day => {
            let m = n.hour * 60 + n.minute;
            (24u32, n.hour, "Today", m as f32 / 1440.0)
        }
        Span::Month => (n.days_in_month(), n.day - 1, MONTHS[n.month as usize - 1], (n.day - 1) as f32 / n.days_in_month() as f32 + (n.hour as f32 / 24.0) / n.days_in_month() as f32),
        Span::Year => {
            let d = n.day_of_year();
            (52, (d - 1) / 7, "This year", (d - 1) as f32 / n.days_in_year() as f32)
        }
    };
    let pct = format!("{:.0}%", frac * 100.0);
    let col = match s {
        Span::Day => BLUE,
        Span::Month => GREEN,
        Span::Year => ORANGE,
    };
    if sh == Shape::Mini {
        compact_sum(c, &Sum::new(Vis::Ring(frac, col, pct), "", ""), sh);
        return;
    }
    let dense = sh == Shape::Dock;
    let (gx, gy, gw, gh) = match sh {
        Shape::Dock => (8.0, 8.0, c.w * 0.62, c.h - 16.0),
        Shape::Small => (12.0, 12.0, c.w - 24.0, c.h - 46.0),
        _ => (16.0, 14.0, c.w - 32.0, c.h - 44.0),
    };
    let cols = match (s, sh) {
        (Span::Day, Shape::Small) => 6,
        (Span::Day, _) => 12,
        (Span::Month, Shape::Small) => 7,
        (Span::Month, _) => 11,
        (Span::Year, Shape::Small) => 8,
        (Span::Year, _) => 13,
    };
    let rows = total.div_ceil(cols);
    let cell = (gw / cols as f32).min(gh / rows as f32);
    let r = cell * 0.32;
    let ox = gx + (gw - cell * cols as f32) / 2.0;
    for i in 0..total {
        let (cx, cy) = (ox + cell * ((i % cols) as f32 + 0.5), gy + cell * ((i / cols) as f32 + 0.5));
        c.circle(cx, cy, r, if i < done { col } else if i == done { alpha(col, 0.5) } else { c.ink(0.14) });
    }
    if dense {
        c.text(&pct, c.w - 8.0, c.h / 2.0 - c.h * 0.2, c.h * 0.3, 600, c.text1(), Align::Right, c.w * 0.36, Font::Display);
        return;
    }
    c.tw(label, gx, c.h - 24.0, 11.5, 500, c.text2(), c.w * 0.6);
    c.tr(&pct, c.w - gx, c.h - 25.0, 12.5, 600, c.text1());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_figures_use_signs_not_triangles() {
        assert_eq!(fmt_delta_text(12.44), "+12.4%");
        assert_eq!(fmt_delta_text(-3.1), "\u{2212}3.1%");
        assert_eq!(fmt_delta_text(0.01), "0.0%");
    }

    #[test]
    fn compact_forms_keep_prefix_and_suffix() {
        assert_eq!(compact_str("$5,336").as_deref(), Some("$5.3K"));
        assert_eq!(compact_str("$83.46").as_deref(), Some("$83"));
        assert_eq!(compact_str("1,900,000 tokens").as_deref(), Some("1.9M tokens"));
        assert_eq!(compact_str("412,000").as_deref(), Some("412K"));
        assert_eq!(compact_str("82%"), None);
        assert_eq!(compact_str("Now"), None);
    }

    #[test]
    fn money_follows_the_spec() {
        assert_eq!(money(83.456, None), "$83.46");
        assert_eq!(money(126.0, None), "$126");
        assert_eq!(money(5336.0, None), "$5,336");
        assert_eq!(money(17491.0, None), "$17.5K");
        assert_eq!(fmt_money(5336.0, None, 5), "$5.3K");
        assert_eq!(fmt_money(126.0, None, 5), "$126");
    }
}
