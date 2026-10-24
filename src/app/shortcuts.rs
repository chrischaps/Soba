//! Help ▸ Keyboard Shortcuts (or F1): every shortcut on one sheet, grouped
//! as the manual groups them.
//!
//! The sheet is one table, written in the manual's own markdown, that the
//! window draws and a test holds against the manual's Keyboard shortcuts
//! section, so the app and the manual can't drift apart. Backticked keys
//! are drawn as keycaps. The Playing group adds a little piano of the
//! letter keys, drawn from `key_to_note`, that lights up as they're held.
//!
//! The sheet, the manual and the tooltips write Command as Ctrl. On a Mac
//! it's ⌘, and `keys` writes a shortcut that way there.

use std::borrow::Cow;

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke};

use super::theme;
use crate::app::qwerty_keys::key_to_note;

/// One group of shortcuts, under its heading.
pub struct Group {
    pub name: &'static str,
    /// The heading's mark
    pub color: Color32,
    /// Shortcut and action, as the manual's table writes them
    pub rows: &'static [(&'static str, &'static str)],
}

/// Every shortcut, in the order of the manual's Keyboard shortcuts section.
pub const SHEET: &[Group] = &[
    Group {
        name: "Canvas",
        color: theme::accent::PRIMARY,
        rows: &[
            ("`Space` or `Tab`", "Quick add a module at the cursor"),
            ("`F`", "Zoom to the selection, or with nothing selected, to fit the whole patch"),
            ("`Shift + F`", "Zoom to fit the whole patch"),
            ("`F1`", "Show or hide the Keyboard Shortcuts window"),
            ("`Escape`", "Close the add menu, quick-add box or Keyboard Shortcuts window, cancel MIDI Learn or leave Map Mode, finish typing a frame's name or a note, or go back out of a group"),
        ],
    },
    Group {
        name: "Editing",
        color: theme::accent::SUCCESS,
        rows: &[
            ("`Delete` or `Backspace`", "Delete the selected modules, frames and notes"),
            ("`Ctrl + D`", "Duplicate the selected modules, frames and notes"),
            ("`Ctrl + C` / `Ctrl + X`", "Copy / cut the selected modules, frames and notes"),
            ("`Ctrl + V`", "Paste at the cursor"),
            ("`Ctrl + Shift + F`", "Frame the selected modules"),
            ("`Ctrl + B`", "Bypass or switch on the selected filters and effects"),
            ("`Ctrl + Z`", "Undo"),
            ("`Ctrl + Shift + Z` or `Ctrl + Y`", "Redo"),
        ],
    },
    Group {
        name: "Knobs",
        color: theme::signal::CONTROL,
        rows: &[
            ("`Shift` + drag", "Fine knob adjustment"),
            ("Double-click a knob", "Reset it to its default"),
            ("`Alt` + scroll over a knob", "Turn it, finely with `Shift` held too"),
            ("Click a knob, then the arrow keys", "Nudge it: `Shift` finer, `Page Up` and `Page Down` further, `Home` to its default"),
            ("`Ctrl + M`", "Mutate the selected modules' knobs: hold it to go further"),
            ("`Ctrl + Alt + M`", "Mutate their switches and dropdowns too"),
            ("`Ctrl + Shift + M`", "Keep the knobs as the next free scene of a Scenes module"),
        ],
    },
    Group {
        name: "Groups",
        color: theme::module::GROUP,
        rows: &[
            ("`Ctrl + G`", "Group the selected modules"),
            ("`Ctrl + Alt + G`", "Ungroup the selected group"),
            ("`Tab`", "Open the selected group"),
            ("`F2`", "Rename the selected group"),
        ],
    },
    Group {
        name: "File",
        color: theme::text::SECONDARY,
        rows: &[
            ("`Ctrl + N`", "New patch"),
            ("`Ctrl + O`", "Open a patch"),
            ("`Ctrl + S`", "Save"),
            ("`Ctrl + Shift + S`", "Save as"),
        ],
    },
    Group {
        name: "Playing",
        color: theme::signal::MIDI,
        rows: &[
            ("`Shift + Space`", "Play or stop the patch"),
            ("`Ctrl + R`", "Record, or end the take"),
            ("`Ctrl + L`", "Map Mode: click controls one after another to map them to MIDI"),
            ("`Ctrl + Shift + L`", "The controller's encoders and faders follow the selection, or let go"),
            ("`Ctrl + H`", "Show or hide the sketch of the controller, saying what each control does"),
            ("`Ctrl + Shift + H`", "The Rig: every MIDI input and MIDI Out port, the device each plays, and the thru routes between them"),
            ("`Alt` over a cable", "Hear what the cable carries"),
        ],
    },
];

/// Did Command (Ctrl, or ⌘ on a Mac) and `key` go down this frame, with
/// exactly `extra` held besides? Takes the press if so. egui's own matching
/// lets an extra Alt through, and Windows reports AltGr as Ctrl+Alt: AltGr+S,
/// typing a letter into a note, would otherwise save.
pub fn consume_command(i: &mut egui::InputState, extra: egui::Modifiers, key: egui::Key) -> bool {
    let modifiers = egui::Modifiers::COMMAND | extra;
    i.modifiers.matches_exact(modifiers) && i.consume_shortcut(&egui::KeyboardShortcut::new(modifiers, key))
}

/// Did Shift+Space, Play or Stop, go down this frame, with nothing else
/// held? Takes the press, and the space it would type, if so. Not Command:
/// ⌘Space is Spotlight on a Mac, and Ctrl+Space its input-source switch.
pub fn consume_play(i: &mut egui::InputState) -> bool {
    let pressed = i.modifiers.matches_exact(egui::Modifiers::SHIFT) && i.consume_key(egui::Modifiers::SHIFT, egui::Key::Space);
    if pressed {
        i.events.retain(|event| !matches!(event, egui::Event::Text(text) if text == " "));
    }
    pressed
}

/// Is this a Mac, whose Command key is ⌘? Asked of egui, so a browser on a
/// Mac counts too.
fn is_mac(ctx: &egui::Context) -> bool {
    ctx.os() == egui::os::OperatingSystem::Mac
}

/// A shortcut, written as this keyboard labels it: `text` as it is, or on a
/// Mac each `Ctrl+` chord in it as a Mac menu writes it, ⇧⌘S for Ctrl+Shift+S.
pub fn keys<'a>(ctx: &egui::Context, text: &'a str) -> Cow<'a, str> {
    if is_mac(ctx) {
        Cow::Owned(mac_keys(text))
    } else {
        Cow::Borrowed(text)
    }
}

/// `text` with each `Ctrl+` chord's modifiers as Mac symbols before its key,
/// in the Mac's order: Option, Shift, Command. None of the fonts has Option's
/// ⌥, so it's written out.
fn mac_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("Ctrl+") {
        out.push_str(&rest[..at]);
        let (mut chord, mut alt, mut shift) = (&rest[at..], false, false);
        loop {
            if let Some(after) = chord.strip_prefix("Ctrl+") {
                chord = after;
            } else if let Some(after) = chord.strip_prefix("Shift+") {
                (chord, shift) = (after, true);
            } else if let Some(after) = chord.strip_prefix("Alt+") {
                (chord, alt) = (after, true);
            } else {
                break;
            }
        }
        if alt {
            out.push_str("Option+");
        }
        if shift {
            out.push('⇧');
        }
        out.push('⌘');
        rest = chord;
    }
    out.push_str(rest);
    out
}

/// A modifier's keycap on a Mac, as its keys are marked (Option's ⌥ isn't
/// in the fonts).
fn mac_cap(cap: &str) -> &str {
    match cap {
        "Ctrl" => "⌘",
        "Alt" => "Option",
        "Shift" => "⇧",
        _ => cap,
    }
}

/// The keys that play notes, as the manual names them: the bottom row for
/// white keys from C4, the row above for black; then the top row for white
/// keys from C5 and the numbers for black, as a tracker lays them out.
pub const PIANO_KEYS: &[(egui::Key, &str)] = &[
    (egui::Key::Z, "Z"), (egui::Key::S, "S"), (egui::Key::X, "X"), (egui::Key::D, "D"),
    (egui::Key::C, "C"), (egui::Key::V, "V"), (egui::Key::G, "G"), (egui::Key::B, "B"),
    (egui::Key::H, "H"), (egui::Key::N, "N"), (egui::Key::J, "J"), (egui::Key::M, "M"),
    (egui::Key::Comma, ","), (egui::Key::L, "L"), (egui::Key::Period, "."),
    (egui::Key::Semicolon, ";"), (egui::Key::Slash, "/"),
    (egui::Key::Q, "Q"), (egui::Key::Num2, "2"), (egui::Key::W, "W"), (egui::Key::Num3, "3"),
    (egui::Key::E, "E"), (egui::Key::R, "R"), (egui::Key::Num5, "5"), (egui::Key::T, "T"),
    (egui::Key::Num6, "6"), (egui::Key::Y, "Y"), (egui::Key::Num7, "7"), (egui::Key::U, "U"),
    (egui::Key::I, "I"), (egui::Key::Num9, "9"), (egui::Key::O, "O"), (egui::Key::Num0, "0"),
    (egui::Key::P, "P"),
];

/// The actions' column, and the gap between the sheet's two columns
const ACTION_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 36.0;
/// Past the widest run of keycaps in a column, before its actions
const KEYS_GAP: f32 = 16.0;
const KEYCAP_HEIGHT: f32 = 20.0;

/// The sheet's window: whether it's open, and the keys that open and close it.
#[derive(Default)]
pub struct ShortcutsSheet {
    pub open: bool,
}

impl ShortcutsSheet {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// F1 shows or hides the sheet. While it's up, Escape puts it away, when
    /// nothing else that Escape closes (a text field, the palette) has the
    /// keys: `free` says so.
    pub fn handle_keys(&mut self, ctx: &egui::Context, free: bool) {
        use egui::{Key, Modifiers};
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::F1) {
                self.toggle();
            } else if self.open && free && i.consume_key(Modifiers::NONE, Key::Escape) {
                self.open = false;
            }
        });
    }

    /// Draws the sheet within `screen`, between the toolbar and the status
    /// bar. `held` are the notes the letter keys (or a pointer on a piano)
    /// are holding, relative to C4.
    pub fn show(&mut self, ctx: &egui::Context, screen: egui::Rect, held: &[i32]) {
        // Canvas, Editing and Knobs on the left; the rest, and the piano,
        // on the right. Each column's actions start past its widest keys
        let (left, right) = SHEET.split_at(3);
        let (left_keys, right_keys) = (keys_width(ctx, left), keys_width(ctx, right));
        let two_columns = screen.width() >= left_keys + right_keys + 2.0 * ACTION_WIDTH + COLUMN_GAP + 64.0;
        let mut open = true;
        egui::Window::new("Keyboard Shortcuts")
            .id(egui::Id::unique("keyboard_shortcuts"))
            .open(&mut open)
            .collapsible(false)
            .auto_sized()
            .pivot(Align2::CENTER_CENTER)
            .default_pos(screen.center())
            .show(ctx, |ui| {
                // Scrolling only in a window too short for it
                let room = screen.height() - 56.0;
                egui::ScrollArea::vertical().max_height(room).show(ui, |ui| {
                    if two_columns {
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| column(ui, left, left_keys, held));
                            ui.add_space(COLUMN_GAP);
                            ui.vertical(|ui| column(ui, right, right_keys, held));
                        });
                    } else {
                        column(ui, SHEET, left_keys.max(right_keys), held);
                    }
                });
            });
        self.open &= open;
    }
}

/// Some groups, one under another, their actions `keys` in from the left.
fn column(ui: &mut egui::Ui, groups: &[Group], keys: f32, held: &[i32]) {
    ui.set_width(keys + ACTION_WIDTH);
    ui.spacing_mut().item_spacing.y = 4.0;
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            ui.add_space(12.0);
        }
        heading(ui, group);
        for (shortcut, action) in group.rows {
            ui.horizontal_top(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(keys, KEYCAP_HEIGHT), Sense::hover());
                draw_keys(ui, rect, shortcut);
                // Level with the keycaps' text, wrapping under itself
                ui.vertical(|ui| {
                    ui.add_space(1.0);
                    ui.add(egui::Label::new(RichText::new(*action).color(theme::text::SECONDARY)).wrap());
                });
            });
        }
        if group.name == "Playing" {
            ui.add_space(8.0);
            piano(ui, held);
        }
    }
}

/// A group's name in the title face, after a short bar of its color.
fn heading(ui: &mut egui::Ui, group: &Group) {
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(vec2(3.0, 15.0), Sense::hover());
        ui.painter().rect_filled(bar, 1.5, group.color);
        let family = egui::FontFamily::Name(theme::TITLE_FAMILY.into());
        ui.label(RichText::new(group.name).font(FontId::new(16.0, family)).color(theme::text::PRIMARY));
    });
    ui.add_space(4.0);
}

/// A piece of a shortcut, as the manual writes it.
enum Piece<'a> {
    /// A key, from a backticked chord: `Ctrl + Shift + Z` is three
    Cap(&'a str),
    /// `or` or `/`, between the choices
    Joiner(&'a str),
    /// A gesture: `+ drag`, `Double-click a knob`
    Words(&'a str),
}

impl Piece<'_> {
    fn text(&self) -> &str {
        let (Piece::Cap(text) | Piece::Joiner(text) | Piece::Words(text)) = self;
        text
    }

    fn color(&self) -> Color32 {
        match self {
            Piece::Joiner(_) => theme::text::DISABLED,
            _ => theme::text::PRIMARY,
        }
    }

    fn galley(&self, fonts: &mut egui::epaint::text::FontsView<'_>) -> std::sync::Arc<egui::Galley> {
        let size = match self {
            Piece::Cap(_) => 12.0,
            Piece::Joiner(_) => 11.0,
            Piece::Words(_) => 13.0,
        };
        fonts.layout_no_wrap(self.text().to_owned(), FontId::proportional(size), self.color())
    }

    /// How much room it takes, its text being `text` wide: a cap is padded,
    /// and a joiner has room either side
    fn width(&self, text: f32) -> f32 {
        match self {
            Piece::Cap(_) => (text + 12.0).max(KEYCAP_HEIGHT),
            Piece::Joiner(_) => text + 2.0 * JOINER_GAP,
            Piece::Words(_) => text,
        }
    }
}

/// Between one piece of a shortcut and the next
const PIECE_GAP: f32 = 3.0;
/// Either side of an `or` or `/`
const JOINER_GAP: f32 = 2.0;

/// The pieces of a shortcut: each backticked chord's keys, and the words
/// between and after them. On a `mac` the modifiers' caps carry its symbols.
fn pieces(shortcut: &str, mac: bool) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    for (i, part) in shortcut.split('`').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if i % 2 == 1 {
            pieces.extend(part.split(" + ").map(|cap| Piece::Cap(if mac { mac_cap(cap) } else { cap })));
        } else if part == "or" || part == "/" {
            pieces.push(Piece::Joiner(part));
        } else {
            pieces.push(Piece::Words(part));
        }
    }
    pieces
}

/// How far in a column's actions start: past its widest shortcut.
fn keys_width(ctx: &egui::Context, groups: &[Group]) -> f32 {
    let mac = is_mac(ctx);
    let widest = ctx.fonts_mut(|fonts| {
        groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .map(|(shortcut, _)| {
                pieces(shortcut, mac).iter().map(|piece| piece.width(piece.galley(fonts).size().x) + PIECE_GAP).sum::<f32>()
            })
            .fold(0.0, f32::max)
    });
    widest + KEYS_GAP
}

/// A shortcut, left to right in `rect`: keys as small raised caps, the
/// words as they are.
fn draw_keys(ui: &egui::Ui, rect: Rect, shortcut: &str) {
    let painter = ui.painter();
    let mut x = rect.left();
    for piece in pieces(shortcut, is_mac(ui.ctx())) {
        let galley = ui.fonts_mut(|fonts| piece.galley(fonts));
        let text = galley.size();
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(piece.width(text.x), KEYCAP_HEIGHT));
        if let Piece::Cap(_) = piece {
            // The cap's lip below, and its face above
            painter.rect_filled(cell, 4.0, theme::background::MAIN);
            let face = Rect::from_min_max(cell.min, cell.max - vec2(0.0, 2.0));
            painter.rect(face, 4.0, theme::background::WIDGET_HOVERED, Stroke::new(1.0_f32, theme::background::WIDGET_ACTIVE), egui::StrokeKind::Middle);
            painter.galley(face.center() - text / 2.0, galley, piece.color());
        } else {
            // On the caps' text line, which sits a point above the middle
            painter.galley(pos2(cell.center().x - text.x / 2.0, cell.center().y - 1.0 - text.y / 2.0), galley, piece.color());
        }
        x = cell.right() + PIECE_GAP;
    }
}

/// The keys as the piano they play, from C4 (middle C) up to E6. Each key is
/// labeled with the keys that play it, by `key_to_note`, and lights up while
/// it's held. Where the two pianos overlap (C5 to E5), the upper row's key
/// sits above the lower's, as it does on the keyboard.
fn piano(ui: &mut egui::Ui, held: &[i32]) {
    const WHITE: [i32; 17] = [0, 2, 4, 5, 7, 9, 11, 12, 14, 16, 17, 19, 21, 23, 24, 26, 28];
    const WHITE_H: f32 = 80.0;
    const BLACK_H: f32 = 48.0;
    /// Between the two letters on a key the rows share
    const LINE: f32 = 13.0;
    // As wide as the column allows, up to a comfortable key
    let white_w = (ui.available_width() / WHITE.len() as f32).clamp(20.0, 28.0);
    let black_w = white_w * 0.62;
    let lit = theme::signal::MIDI;
    let letters = |note: i32| PIANO_KEYS.iter().filter(move |(key, _)| key_to_note(*key) == Some(note)).map(|(_, name)| *name);

    let (rect, _) = ui.allocate_exact_size(vec2(WHITE.len() as f32 * white_w, WHITE_H + 16.0), Sense::hover());
    let painter = ui.painter();
    let font = FontId::proportional(11.0);
    for (i, &note) in WHITE.iter().enumerate() {
        let key = Rect::from_min_size(pos2(rect.left() + i as f32 * white_w, rect.top()), vec2(white_w, WHITE_H)).shrink2(vec2(1.0, 0.0));
        let fill = if held.contains(&note) { lit } else { Color32::from_rgb(226, 226, 234) };
        painter.rect_filled(key, egui::CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 }, fill);
        for (row, name) in letters(note).enumerate() {
            let at = key.center_bottom() - vec2(0.0, 6.0 + row as f32 * LINE);
            painter.text(at, Align2::CENTER_BOTTOM, name, font.clone(), theme::background::MAIN);
        }
    }
    for (i, pair) in WHITE.windows(2).enumerate() {
        // A black key wherever two white keys are a whole tone apart
        if pair[1] - pair[0] != 2 {
            continue;
        }
        let note = pair[0] + 1;
        let x = rect.left() + (i + 1) as f32 * white_w;
        let key = Rect::from_min_size(pos2(x - black_w / 2.0, rect.top()), vec2(black_w, BLACK_H));
        let fill = if held.contains(&note) { lit } else { theme::background::MAIN };
        painter.rect(key, egui::CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, fill, Stroke::new(1.0_f32, theme::background::WIDGET_ACTIVE), egui::StrokeKind::Middle);
        for (row, name) in letters(note).enumerate() {
            let at = key.center_bottom() - vec2(0.0, 5.0 + row as f32 * LINE);
            painter.text(at, Align2::CENTER_BOTTOM, name, font.clone(), theme::text::PRIMARY);
        }
    }
    // Each C, named under its key: middle C, and the octaves above it
    for (i, name) in [(0, "C4"), (7, "C5"), (14, "C6")] {
        painter.text(
            pos2(rect.left() + (i as f32 + 0.5) * white_w, rect.bottom()),
            Align2::CENTER_BOTTOM,
            name,
            FontId::proportional(10.0),
            theme::text::QUIET,
        );
    }
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            RichText::new("The letter rows and the numbers above them play every Note In")
                .small()
                .color(theme::text::QUIET),
        )
        .wrap(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANUAL: &str = include_str!("../../docs/src/getting-started/interface-overview.md");

    /// The lines of a `## ` section of the manual, up to the next one.
    fn section(title: &str) -> Vec<&'static str> {
        MANUAL
            .lines()
            .skip_while(|line| line.trim() != title)
            .skip(1)
            .take_while(|line| !line.starts_with("## "))
            .collect()
    }

    /// A table row's cells, or None for anything that isn't a row (or is
    /// the header's rule).
    fn cells(line: &str) -> Option<Vec<&str>> {
        let row = line.trim().strip_prefix('|')?.strip_suffix('|')?;
        let cells: Vec<&str> = row.split('|').map(str::trim).collect();
        (!cells.iter().all(|cell| cell.chars().all(|c| c == '-'))).then_some(cells)
    }

    /// The manual's Keyboard shortcuts section, as (group, rows), without
    /// each table's header row.
    fn manual_sheet() -> Vec<(String, Vec<(String, String)>)> {
        let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut header = false;
        for line in section("## Keyboard shortcuts") {
            if let Some(name) = line.strip_prefix("### ") {
                groups.push((name.trim().to_owned(), Vec::new()));
                header = true;
            } else if let Some(cells) = cells(line) {
                if std::mem::take(&mut header) {
                    continue;
                }
                let (_, rows) = groups.last_mut().expect("a table before the first ### group");
                assert_eq!(cells.len(), 2, "a shortcut row with other than two cells: {line}");
                rows.push((cells[0].to_owned(), cells[1].to_owned()));
            }
        }
        groups
    }

    #[test]
    fn mac_keys_writes_chords_as_mac_menus_do() {
        assert_eq!(mac_keys("Save to a new file (Ctrl+Shift+S)"), "Save to a new file (⇧⌘S)");
        assert_eq!(mac_keys("Ctrl+N"), "⌘N");
        assert_eq!(mac_keys("Ungroup (Ctrl+Alt+G)"), "Ungroup (Option+⌘G)");
        assert_eq!(mac_keys("Unsaved changes (Ctrl+S to save)"), "Unsaved changes (⌘S to save)");
        assert_eq!(mac_keys("Shift+click its tie"), "Shift+click its tie");
        assert_eq!(pieces("`Ctrl + Shift + Z` or `Ctrl + Y`", true).iter().map(Piece::text).collect::<Vec<_>>(), ["⌘", "⇧", "Z", "or", "⌘", "Y"]);
    }

    /// The Mac's symbols have to be in the fonts, or they'd draw as boxes
    #[test]
    fn mac_symbols_are_in_the_fonts() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        assert!(ctx.fonts_mut(|fonts| fonts.has_glyphs(&FontId::proportional(12.0), "⌘⇧")));
    }

    /// One frame with Space pressed under `modifiers` (and the space it
    /// types): was it Play, and was the space left for the palette?
    fn press_space(modifiers: egui::Modifiers) -> (bool, bool) {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key { key: egui::Key::Space, physical_key: None, pressed: true, repeat: false, modifiers },
                egui::Event::Text(" ".into()),
            ],
            ..Default::default()
        };
        let mut result = (false, false);
        ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx();
            result = ctx.input_mut(|i| {
                let play = consume_play(i);
                (play, i.key_pressed(egui::Key::Space))
            });
        }).textures_delta.clear();
        result
    }

    #[test]
    fn shift_space_plays_and_nothing_else_does() {
        assert_eq!(press_space(egui::Modifiers::SHIFT), (true, false));
        // Plain Space is the palette's, and Ctrl+Shift+Space no one's
        assert_eq!(press_space(egui::Modifiers::NONE), (false, true));
        assert!(!press_space(egui::Modifiers::SHIFT | egui::Modifiers::COMMAND).0);
        assert!(!press_space(egui::Modifiers::SHIFT | egui::Modifiers::ALT).0);
    }

    #[test]
    fn sheet_matches_the_manual() {
        let app: Vec<(String, Vec<(String, String)>)> = SHEET
            .iter()
            .map(|group| {
                let rows = group.rows.iter().map(|(keys, action)| (keys.to_string(), action.to_string())).collect();
                (group.name.to_owned(), rows)
            })
            .collect();
        let manual = manual_sheet();
        assert!(!manual.is_empty(), "no ### groups under the manual's ## Keyboard shortcuts");
        // Group by group, so a difference is easy to find
        for (ours, theirs) in app.iter().zip(&manual) {
            assert_eq!(ours, theirs, "the app's Keyboard Shortcuts sheet and the manual's table disagree");
        }
        assert_eq!(
            app.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            manual.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            "the app's sheet and the manual have different groups",
        );
    }

    /// A note name like `C` or `F♯`, as semitones above C.
    fn semitone(name: &str) -> i32 {
        let mut chars = name.chars();
        let base = match chars.next() {
            Some('C') => 0,
            Some('D') => 2,
            Some('E') => 4,
            Some('F') => 5,
            Some('G') => 7,
            Some('A') => 9,
            Some('B') => 11,
            _ => panic!("not a note: {name}"),
        };
        match chars.as_str() {
            "" => base,
            "♯" => base + 1,
            rest => panic!("not a note: {name} ({rest})"),
        }
    }

    #[test]
    fn piano_matches_the_manual_and_the_keys() {
        // The manual's table under "The computer keyboard"
        let table: Vec<&str> = MANUAL
            .lines()
            .skip_while(|line| line.trim() != "### The computer keyboard")
            .take_while(|line| !line.starts_with("### MIDI"))
            .collect();
        let mut named = Vec::new();
        for cells in table.iter().filter_map(|line| cells(line)).skip(1) {
            let keys: Vec<&str> = cells[0].split('`').map(str::trim).filter(|k| !k.is_empty()).collect();
            let (notes, octave) = if let Some((notes, _)) = cells[1].split_once(", two octaves up") {
                (notes, 24)
            } else if let Some((notes, _)) = cells[1].split_once(", an octave up") {
                (notes, 12)
            } else {
                (cells[1], 0)
            };
            let notes: Vec<&str> = notes.split_whitespace().collect();
            assert_eq!(keys.len(), notes.len(), "keys and notes don't pair up: {cells:?}");
            for (name, note) in keys.into_iter().zip(notes) {
                let &(key, _) = PIANO_KEYS
                    .iter()
                    .find(|(_, n)| *n == name)
                    .unwrap_or_else(|| panic!("the manual's `{name}` isn't on the sheet's piano"));
                assert_eq!(key_to_note(key), Some(semitone(note) + octave), "`{name}` should play {note}");
                named.push(name);
            }
        }
        // Every key on the piano is in the manual, once
        named.sort_unstable();
        let mut ours: Vec<&str> = PIANO_KEYS.iter().map(|(_, name)| *name).collect();
        ours.sort_unstable();
        assert_eq!(named, ours, "the sheet's piano and the manual's keys differ");

        // And they're two pianos an octave apart: C4 up to E5 on the lower
        // rows, C5 up to E6 on the upper ones
        let notes = |keys: &[(egui::Key, &str)]| {
            let mut notes: Vec<i32> = keys.iter().filter_map(|(key, _)| key_to_note(*key)).collect();
            notes.sort_unstable();
            notes
        };
        let (lower, upper) = PIANO_KEYS.split_at(17);
        assert_eq!(notes(lower), (0..=16).collect::<Vec<_>>());
        assert_eq!(notes(upper), (12..=28).collect::<Vec<_>>());

        // No other key plays a note, so nothing sounds that isn't on the sheet
        for key in egui::Key::ALL {
            if key_to_note(*key).is_some() {
                assert!(PIANO_KEYS.iter().any(|(k, _)| k == key), "{key:?} plays a note the sheet doesn't show");
            }
        }
    }
}
