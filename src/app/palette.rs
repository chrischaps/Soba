//! The quick-add palette: press Space or Tab over the graph, type a few
//! letters of a module's name or category, press Enter, and the module
//! appears where the cursor was. Groups saved to My Modules are there too,
//! first, in their rose, and the Library's ready-made groups, last.
//!
//! Matching is fuzzy: the letters typed have to appear in order, not
//! together, so "lfo", "svf" and "dly" all find what they should. Letters at
//! the start of words and runs of letters count for more.

use eframe::egui::{self, text::LayoutJob, Align2, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, TextFormat, Vec2};

use crate::graph::{AllNodeTemplates, SynthNodeTemplate};
use super::library::SavedModule;
use super::theme;

const WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 26.0;
const LIST_HEIGHT: f32 = 330.0;

/// Points for each matched letter, and extra for where it falls.
const MATCHED: i32 = 1;
const WORD_START: i32 = 8;
const FIRST_LETTER: i32 = 4;
const CONSECUTIVE: i32 = 6;
/// Letters skipped between two matches cost this much each, up to a cap.
const GAP: i32 = 1;
const MAX_GAP_COST: i32 = 6;

/// Where the query's letters fall in `text`, and how good a match that is.
/// `None` if they don't all appear in order. Case doesn't matter.
fn subsequence(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let original: Vec<char> = text.chars().collect();
    let text: Vec<char> = original.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    if query.is_empty() || query.len() > text.len() {
        return None;
    }

    let word_start = |i: usize| {
        i == 0 || !original[i - 1].is_alphanumeric() || (original[i].is_uppercase() && original[i - 1].is_lowercase())
    };
    let bonus = |i: usize| MATCHED + if word_start(i) { WORD_START } else { 0 } + if i == 0 { FIRST_LETTER } else { 0 };

    // best[q][t]: the best score with query letter q matched at text letter t,
    // and where the letter before it was matched
    let mut best: Vec<Vec<Option<(i32, usize)>>> = vec![vec![None; text.len()]; query.len()];
    for t in 0..text.len() {
        if text[t] == query[0] {
            best[0][t] = Some((bonus(t) - (t as i32).min(MAX_GAP_COST), 0));
        }
    }
    for q in 1..query.len() {
        for t in q..text.len() {
            if text[t] != query[q] {
                continue;
            }
            best[q][t] = (q - 1..t)
                .filter_map(|prev| {
                    let (score, _) = best[q - 1][prev]?;
                    let step = if prev + 1 == t { CONSECUTIVE } else { -(((t - prev - 1) as i32) * GAP).min(MAX_GAP_COST) };
                    Some((score + bonus(t) + step, prev))
                })
                .max_by_key(|(score, _)| *score);
        }
    }

    let last = query.len() - 1;
    let (end, (score, _)) = best[last].iter().enumerate().filter_map(|(t, b)| Some((t, (*b)?))).max_by_key(|(_, (s, _))| *s)?;
    let mut positions = vec![end];
    let mut t = end;
    for q in (1..=last).rev() {
        t = best[q][t]?.1;
        positions.push(t);
    }
    positions.reverse();
    Some((score, positions))
}

/// The heading saved groups go under.
const MY_MODULES: &str = "My Modules";
/// The heading the Library's groups go under.
const LIBRARY: &str = "Library";

/// Something the palette can add: a module, or a group from My Modules.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Module(SynthNodeTemplate),
    Saved(SavedModule),
}

impl Entry {
    fn name(&self) -> &str {
        match self {
            Self::Module(template) => template.name(),
            Self::Saved(saved) => &saved.name,
        }
    }

    fn category(&self) -> &str {
        match self {
            Self::Module(template) => template.category().name(),
            Self::Saved(saved) if saved.in_library() => LIBRARY,
            Self::Saved(_) => MY_MODULES,
        }
    }

    fn color(&self) -> Color32 {
        match self {
            Self::Module(template) => template.category().color(),
            Self::Saved(_) => theme::module::GROUP,
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::Module(template) => template.description(),
            Self::Saved(saved) => &saved.summary,
        }
    }

    /// Text matched only as typed: a module's ID.
    fn id(&self) -> &str {
        match self {
            Self::Module(template) => template.module_id(),
            Self::Saved(_) => "",
        }
    }
}

/// Something the query found, with the letters of its name that matched.
struct Found {
    entry: Entry,
    score: i32,
    /// Character positions in the name, for highlighting.
    matched: Vec<usize>,
}

/// How well one word of the query matches an entry. Names match fuzzily.
/// Categories ("eff", "my"), module IDs ("fx.") and descriptions only match
/// the word as typed: spread over a long ID, three letters match almost anything.
fn match_word(word: &str, entry: &Entry) -> Option<(i32, Vec<usize>)> {
    let word_lower = word.to_lowercase();
    let contains = |text: &str| text.to_lowercase().contains(&word_lower);
    // Letters scattered far apart score at or below zero: not a match
    let by_name = subsequence(word, entry.name()).filter(|(s, _)| *s > 0).map(|(s, m)| (s * 3, m));
    let by_category = entry.category().to_lowercase().starts_with(&word_lower).then_some((12, Vec::new()));
    let by_id = (!entry.id().is_empty() && contains(entry.id())).then_some((6, Vec::new()));
    let by_description = (word.len() >= 3 && contains(entry.description())).then_some((2, Vec::new()));
    [by_name, by_category, by_id, by_description].into_iter().flatten().max_by_key(|(s, _)| *s)
}

/// Everything matching all the words of `query`, best first. An empty
/// query finds everything in menu order: My Modules, every module, then
/// the Library.
fn search(query: &str, saved: &[SavedModule]) -> Vec<Found> {
    let (library, mine): (Vec<_>, Vec<_>) = saved.iter().cloned().partition(SavedModule::in_library);
    let menu_order: Vec<Entry> = mine
        .into_iter()
        .map(Entry::Saved)
        .chain(AllNodeTemplates::by_category().into_iter().flat_map(|(_, templates)| templates).map(Entry::Module))
        .chain(library.into_iter().map(Entry::Saved))
        .collect();
    let words: Vec<&str> = query.split_whitespace().collect();

    let mut found: Vec<(usize, Found)> = menu_order
        .into_iter()
        .enumerate()
        .filter_map(|(order, entry)| {
            let mut score = 0;
            let mut matched = Vec::new();
            for word in &words {
                let (s, m) = match_word(word, &entry)?;
                score += s;
                matched.extend(m);
            }
            matched.sort_unstable();
            matched.dedup();
            Some((order, Found { entry, score, matched }))
        })
        .collect();
    found.sort_by_key(|(order, f)| (-f.score, *order));
    found.into_iter().map(|(_, f)| f).collect()
}

/// What the user did with the palette this frame.
pub enum PaletteAction {
    /// Still choosing.
    None,
    /// Closed without adding anything.
    Close,
    /// Chose a module to add.
    Add(SynthNodeTemplate),
    /// Chose a group from My Modules or the Library.
    AddSaved(SavedModule),
}

impl PaletteAction {
    fn add(entry: &Entry) -> Self {
        match entry {
            Entry::Module(template) => Self::Add(*template),
            Entry::Saved(saved) => Self::AddSaved(saved.clone()),
        }
    }
}

/// The open palette.
pub struct QuickAdd {
    /// Where it was opened, in screen points. The module goes here.
    anchor: Pos2,
    query: String,
    /// Which result Enter would add.
    highlighted: usize,
    /// The highlight moved by keyboard this frame, so the list should
    /// scroll to it.
    scroll_to_highlight: bool,
    /// The groups in My Modules and the Library.
    saved: Vec<SavedModule>,
}

impl QuickAdd {
    pub fn new(anchor: Pos2, saved: Vec<SavedModule>) -> Self {
        Self { anchor, query: String::new(), highlighted: 0, scroll_to_highlight: false, saved }
    }

    /// Where the palette was opened, in screen points.
    pub fn anchor(&self) -> Pos2 {
        self.anchor
    }

    /// Draws the palette and handles its keys.
    pub fn show(&mut self, ctx: &egui::Context) -> PaletteAction {
        // The list's keys, taken before the text field sees them
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            let up = i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::SHIFT, Key::Tab);
            let down = i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::Tab);
            let enter = i.consume_key(Modifiers::NONE, Key::Enter);
            let escape = i.consume_key(Modifiers::NONE, Key::Escape);
            (up, down, enter, escape)
        });
        if escape {
            return PaletteAction::Close;
        }

        let results = search(&self.query, &self.saved);
        if results.is_empty() {
            self.highlighted = 0;
        } else {
            let last = results.len() - 1;
            if up {
                self.highlighted = if self.highlighted == 0 { last } else { self.highlighted - 1 };
            }
            if down {
                self.highlighted = if self.highlighted >= last { 0 } else { self.highlighted + 1 };
            }
            self.highlighted = self.highlighted.min(last);
            self.scroll_to_highlight |= up || down;
        }
        if enter {
            return results.get(self.highlighted).map_or(PaletteAction::None, |f| PaletteAction::add(&f.entry));
        }

        let mut action = PaletteAction::None;
        let area = egui::Area::new(egui::Id::unique("quick_add_palette"))
            .fixed_pos(self.anchor)
            .order(egui::Order::Foreground)
            .constrain(true)
            .show(ctx, |ui| {
                palette_frame().show(ui, |ui| {
                    ui.set_width(WIDTH);
                    self.search_field(ui, results.len());
                    ui.add_space(6.0);
                    if let Some(entry) = self.result_list(ui, &results) {
                        action = PaletteAction::add(&entry);
                    }
                    if let Some(found) = results.get(self.highlighted) {
                        footer(ui, &found.entry);
                    }
                });
            });

        // A click anywhere else closes it
        let clicked_outside = ctx.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pos| !area.response.rect.contains(pos))
        });
        if clicked_outside && matches!(action, PaletteAction::None) {
            action = PaletteAction::Close;
        }
        action
    }

    fn search_field(&mut self, ui: &mut egui::Ui, count: usize) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("+").size(18.0).color(theme::text::ACCENT));
            let edit = egui::TextEdit::singleline(&mut self.query)
                .hint_text("Add a module…")
                .font(FontId::proportional(15.0))
                .text_color(theme::text::PRIMARY)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 2)))
                .desired_width(WIDTH - 70.0);
            let response = ui.add(edit);
            response.request_focus();
            if response.changed() {
                self.highlighted = 0;
                self.scroll_to_highlight = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(count.to_string()).small().color(theme::text::QUIET));
            });
        });
        let rect = ui.min_rect();
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() + 3.0,
            egui::Stroke::new(1.0_f32, theme::background::WIDGET_HOVERED),
        );
    }

    /// The matching modules. With no query they're grouped under their
    /// categories, like the right-click menu.
    fn result_list(&mut self, ui: &mut egui::Ui, results: &[Found]) -> Option<Entry> {
        if results.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("No module matches").color(theme::text::DISABLED).italics());
            ui.add_space(4.0);
            return None;
        }
        let grouped = self.query.trim().is_empty();
        let pointer_moved = ui.input(|i| i.pointer.delta() != Vec2::ZERO);
        let mut chosen = None;

        egui::ScrollArea::vertical().max_height(LIST_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
            let mut previous_category = None;
            for (index, found) in results.iter().enumerate() {
                let category = found.entry.category();
                if grouped && previous_category != Some(category) {
                    if previous_category.is_some() {
                        ui.add_space(4.0);
                    }
                    ui.label(egui::RichText::new(category.to_uppercase()).small().color(found.entry.color().gamma_multiply(0.8)));
                    previous_category = Some(category);
                }

                let highlighted = index == self.highlighted;
                let response = result_row(ui, found, highlighted, !grouped);
                if response.hovered() && pointer_moved {
                    self.highlighted = index;
                }
                if response.clicked() {
                    chosen = Some(found.entry.clone());
                }
                if highlighted && self.scroll_to_highlight {
                    response.scroll_to_me(None);
                }
            }
        });
        self.scroll_to_highlight = false;
        chosen
    }
}

/// The palette's panel: the toolbar's colour, lifted off the graph.
fn palette_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::background::PANEL)
        .stroke(egui::Stroke::new(1.0_f32, theme::background::WIDGET_ACTIVE))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(140),
        })
}

/// One module: a dot in its category's colour, its name with the matched
/// letters lit in that colour, and its category on the right unless the
/// list is already grouped by category.
fn result_row(ui: &mut egui::Ui, found: &Found, highlighted: bool, show_category: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
    let color = found.entry.color();
    let painter = ui.painter_at(rect.expand(1.0));

    if highlighted {
        painter.rect_filled(rect, 6.0, color.gamma_multiply(0.16));
        let bar = Rect::from_min_size(rect.min + Vec2::new(0.0, 5.0), Vec2::new(3.0, rect.height() - 10.0));
        painter.rect_filled(bar, 1.5, color);
    }
    painter.circle_filled(Pos2::new(rect.left() + 14.0, rect.center().y), 4.0, color);

    let text_color = if highlighted { theme::text::PRIMARY } else { theme::text::PRIMARY.gamma_multiply(0.85) };
    let mut job = LayoutJob::default();
    for (i, ch) in found.entry.name().chars().enumerate() {
        let lit = found.matched.contains(&i);
        let format = TextFormat {
            font_id: FontId::proportional(14.0),
            color: if lit { color } else { text_color },
            ..Default::default()
        };
        job.append(&ch.to_string(), 0.0, format);
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    painter.galley(Pos2::new(rect.left() + 26.0, rect.center().y - galley.size().y / 2.0), galley, text_color);

    if show_category {
        painter.text(
            Pos2::new(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            found.entry.category(),
            FontId::proportional(11.0),
            if highlighted { color } else { theme::text::QUIET },
        );
    }
    response
}

/// What the highlighted module does, and the keys.
fn footer(ui: &mut egui::Ui, entry: &Entry) {
    ui.add_space(4.0);
    let rect = ui.min_rect();
    ui.painter().hline(rect.x_range(), ui.cursor().top(), egui::Stroke::new(1.0_f32, theme::background::WIDGET_HOVERED));
    ui.add_space(6.0);
    ui.label(egui::RichText::new(entry.description()).small().color(theme::text::SECONDARY));
    ui.add_space(2.0);
    ui.label(egui::RichText::new("Arrows or Tab to choose  ·  Enter to add  ·  Esc to close").small().color(theme::text::QUIET));
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::library::{self, Source};
    use egui_node_graph2::NodeTemplateIter;

    fn top(query: &str) -> &'static str {
        match search(query, &[]).first().map(|f| &f.entry) {
            Some(Entry::Module(template)) => template.module_id(),
            _ => "none",
        }
    }

    fn template(entry: &Entry) -> SynthNodeTemplate {
        match entry {
            Entry::Module(template) => *template,
            Entry::Saved(saved) => panic!("expected a module, found {}", saved.name),
        }
    }

    #[test]
    fn letters_must_appear_in_order() {
        assert!(subsequence("svf", "SVF Filter").is_some());
        assert!(subsequence("fvs", "SVF Filter").is_none());
        assert!(subsequence("", "Anything").is_none());
    }

    #[test]
    fn word_starts_and_runs_win() {
        // "re" starting "Reverb" beats "re" inside "Compressor"
        let (start, _) = subsequence("re", "Reverb").unwrap();
        let (inside, _) = subsequence("re", "Compressor").unwrap();
        assert!(start > inside);
        // The matched letters are the ones at word starts
        assert_eq!(subsequence("sh", "Sample & Hold").unwrap().1, vec![0, 9]);
    }

    #[test]
    fn finds_modules_by_abbreviation_name_and_category() {
        assert_eq!(top("lfo"), "mod.lfo");
        assert_eq!(top("ladder"), "filter.ladder");
        assert_eq!(top("verb"), "fx.reverb");
        assert_eq!(top("osc"), "osc.sine");
        assert_eq!(top("adsr"), "mod.adsr");
        // The examples the docs give
        assert_eq!(top("svf"), "filter.svf");
        assert_eq!(top("dly"), "fx.delay");
        assert_eq!(top("s&h"), "util.sample_hold");
        // Note In, by its name or the modules it took the place of
        assert_eq!(top("note"), "input.poly_midi");
        assert!(search("keyboard", &[]).iter().any(|f| template(&f.entry).module_id() == "input.poly_midi"));
        // A category's modules come before stray matches in names and descriptions
        let modulation = AllNodeTemplates.all_kinds().into_iter().filter(|t| t.category().name() == "Modulation").count();
        assert!(search("mod", &[]).iter().take(modulation).all(|f| template(&f.entry).category().name() == "Modulation"));
        // Letters scattered across a name don't count ("Sample & Hold")
        assert!(search("mod", &[]).iter().all(|f| f.entry.name() != "Sample & Hold"));
        // Every word has to match: a category narrows the list
        assert!(search("effect", &[]).iter().all(|f| template(&f.entry).category().name() == "Effect"));
        assert!(search("zzzz", &[]).is_empty());
        // An ID's letters spread far apart aren't a match ("util.sample_hold")
        assert!(search("lad", &[]).iter().all(|f| template(&f.entry).module_id() == "filter.ladder"));
        assert_eq!(top("fx.del"), "fx.delay");
    }

    #[test]
    fn empty_query_lists_everything_in_menu_order() {
        let all: Vec<_> = search("", &[]).into_iter().map(|f| template(&f.entry).module_id()).collect();
        let menu: Vec<_> = AllNodeTemplates::by_category()
            .into_iter()
            .flat_map(|(_, t)| t)
            .map(|t| t.module_id())
            .collect();
        assert_eq!(all, menu);
        assert_eq!(all.len(), AllNodeTemplates.all_kinds().len());
    }

    #[test]
    fn saved_groups_come_first_and_match_by_name_or_heading() {
        let saved = vec![SavedModule {
            name: "Acid Voice".into(),
            source: Source::File("Acid Voice.json".into()),
            summary: "Gate, Pitch → Out · 4 modules".into(),
        }];
        let all = search("", &saved);
        assert_eq!(all[0].entry, Entry::Saved(saved[0].clone()));
        assert_eq!(all.len(), AllNodeTemplates.all_kinds().len() + 1);
        assert!(matches!(&search("acid", &saved)[0].entry, Entry::Saved(s) if s.name == "Acid Voice"));
        assert!(search("my", &saved).iter().any(|f| matches!(f.entry, Entry::Saved(_))));
        assert!(matches!(PaletteAction::add(&all[0].entry), PaletteAction::AddSaved(_)));
    }

    #[test]
    fn the_library_comes_last_and_matches_by_name_or_heading() {
        let mut saved = library::library();
        saved.insert(0, SavedModule { name: "Mine".into(), source: Source::File("Mine.json".into()), summary: String::new() });
        let all = search("", &saved);
        assert_eq!(all.len(), AllNodeTemplates.all_kinds().len() + saved.len());
        assert!(matches!(&all[0].entry, Entry::Saved(s) if s.name == "Mine"));
        assert!(all.iter().rev().take(saved.len() - 1).all(|f| f.entry.category() == "Library"));
        assert!(matches!(&search("supersaw", &saved)[0].entry, Entry::Saved(s) if s.name == "Supersaw Pad"));
        assert!(matches!(&search("fm bell", &saved)[0].entry, Entry::Saved(s) if s.name == "FM Bell"));
        let headed = search("lib", &saved);
        assert!(headed.iter().take(saved.len() - 1).all(|f| f.entry.category() == "Library"));
        // A module still wins its own name
        assert_eq!(top("ladder"), "filter.ladder");
    }
}
