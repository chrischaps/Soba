//! Learn in order (#481): one learn walked across a group. Right-click a
//! group of Pads and choose Learn in order, and the first Pad, by where it
//! sits (rows top to bottom, each left to right), waits for a pad. Hit one
//! and it takes that note and channel, and the next Pad waits. Tab (or
//! Skip) leaves one as it is, and Escape stops, keeping what was learned.
//! The whole walk is one undo step.
//!
//! Each step is an ordinary learn into a module (see
//! [`crate::app::module_learn`]), so a walk takes whatever module learns
//! from one hit or move: a Pad, or a Control (#483), whose group learns
//! as each fader is moved in turn. Another joins by answering [`walks`]
//! and [`learn_start`].
//!
//! [`LearnWalk`] is the walk itself, kept on the
//! [`SynthGraphState`] so the faces can show it: the group's miniature
//! lights each Pad's card as it's learned, and each Pad says where in the
//! walk it comes. The app's side, which starts each learn and hears it
//! end, is `crate::app::synth_app::learn_walk`.

use egui::Pos2;
use egui_node_graph2::NodeId;

use crate::modules::{control, pad};

use super::groups::{self, GroupId};
use super::{controls_face, pads_face, SynthGraph, SynthGraphState, SynthResponse};

/// How far apart two modules' tops can be, unzoomed, and still be read as
/// one row: well under a Pad's height, well over a hand's misplacement.
const ROW_SLACK: f32 = 80.0;

/// Whether a walk learns this kind of module: one that learns what it
/// hears from a single hit or move.
pub fn walks(module_id: &str) -> bool {
    [pad::ID, control::ID].contains(&module_id)
}

/// How many modules directly inside group `id` a walk would learn.
pub fn count(graph: &SynthGraph, id: GroupId) -> usize {
    groups::level_nodes(graph, Some(id)).into_iter().filter(|&n| walks(graph[n].user_data.module_id)).count()
}

/// The modules directly inside group `id` a walk learns, in the order it
/// learns them: by rows, top to bottom, each left to right. `position` is
/// where each sits in the editor, at `zoom`.
pub fn steps(graph: &SynthGraph, position: impl Fn(NodeId) -> Option<Pos2>, id: GroupId, zoom: f32) -> Vec<NodeId> {
    let walked = groups::level_nodes(graph, Some(id)).into_iter().filter(|&n| walks(graph[n].user_data.module_id));
    reading_order(walked.map(|n| (n, position(n).unwrap_or_default())).collect(), ROW_SLACK * zoom)
}

/// Things at points in reading order: in rows top to bottom, each left to
/// right. A row is everything whose top is within `slack` of its first.
pub fn reading_order<T: Copy>(mut placed: Vec<(T, Pos2)>, slack: f32) -> Vec<T> {
    placed.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
    let mut rows: Vec<Vec<(T, Pos2)>> = Vec::new();
    for item in placed {
        match rows.last_mut() {
            Some(row) if item.1.y - row[0].1.y <= slack => row.push(item),
            _ => rows.push(vec![item]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
            row.into_iter().map(|(item, _)| item)
        })
        .collect()
}

/// The response that starts the learn of one step, as the module's own
/// face would start it.
pub fn learn_start(graph: &SynthGraph, user_state: &SynthGraphState, node_id: NodeId) -> Option<SynthResponse> {
    let node = graph.nodes.get(node_id)?;
    let engine_id = user_state.get_engine_node_id(node_id)?;
    match node.user_data.module_id {
        pad::ID => Some(pads_face::learn_start(engine_id, &node.user_data.display_name, pads_face::listen_of(graph, node_id))),
        control::ID => Some(controls_face::learn_start(engine_id, &node.user_data.display_name)),
        _ => None,
    }
}

/// How far a walk has come with one of its modules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Not reached yet, or waiting now.
    Waiting,
    /// It took what was heard.
    Learned,
    /// Skipped: it keeps what it had.
    Skipped,
}

/// Where a module stands in the walk, as its face shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Waiting for a hit now.
    Now,
    /// Still to come: the walk's `n`th (from 1).
    Next(usize),
    Learned,
    Skipped,
}

/// A learn walking a group's modules, one after another.
#[derive(Clone, Debug, PartialEq)]
pub struct LearnWalk {
    /// The group walked.
    pub group: GroupId,
    /// Its name, for the status bar.
    pub name: String,
    steps: Vec<(NodeId, Stage)>,
    at: usize,
    /// What each step learned, in what it wrote: a hit the walk has already
    /// had (a pad hit twice, or bouncing) is passed by, not learned again.
    taken: Vec<Vec<(usize, f32)>>,
}

impl LearnWalk {
    /// A walk over `steps`, in order, or none if there's nothing to walk.
    pub fn new(group: GroupId, name: impl Into<String>, steps: Vec<NodeId>) -> Option<Self> {
        (!steps.is_empty()).then(|| Self {
            group,
            name: name.into(),
            steps: steps.into_iter().map(|n| (n, Stage::Waiting)).collect(),
            at: 0,
            taken: Vec::new(),
        })
    }

    /// The module waiting now, until the walk is over.
    pub fn current(&self) -> Option<NodeId> {
        self.steps.get(self.at).map(|&(n, _)| n)
    }

    /// Which step is waiting, from 0, and how many there are.
    pub fn progress(&self) -> (usize, usize) {
        (self.at.min(self.steps.len()), self.steps.len())
    }

    /// How many took what they heard.
    pub fn learned(&self) -> usize {
        self.steps.iter().filter(|(_, stage)| *stage == Stage::Learned).count()
    }

    pub fn is_over(&self) -> bool {
        self.at >= self.steps.len()
    }

    /// Where `node` stands in the walk, if it's in it.
    pub fn mark(&self, node: NodeId) -> Option<Mark> {
        let at = self.steps.iter().position(|&(n, _)| n == node)?;
        Some(match self.steps[at].1 {
            Stage::Learned => Mark::Learned,
            Stage::Skipped => Mark::Skipped,
            Stage::Waiting if at == self.at => Mark::Now,
            Stage::Waiting => Mark::Next(at + 1),
        })
    }

    /// The modules walked, in order, with how far each has come.
    pub fn steps(&self) -> impl Iterator<Item = (NodeId, Stage)> + '_ {
        self.steps.iter().copied()
    }

    /// What the walk's steps have learned so far, each as the writes it made.
    pub fn taken(&self) -> &[Vec<(usize, f32)>] {
        &self.taken
    }

    /// The module waiting took `writes`: on to the next.
    pub fn take(&mut self, writes: Vec<(usize, f32)>) {
        if let Some(step) = self.steps.get_mut(self.at) {
            step.1 = Stage::Learned;
            self.taken.push(writes);
            self.at += 1;
        }
    }

    /// Leaves the module waiting as it is: on to the next.
    pub fn skip(&mut self) {
        if let Some(step) = self.steps.get_mut(self.at) {
            step.1 = Stage::Skipped;
            self.at += 1;
        }
    }

    /// Skips the modules that have gone from the patch since the walk
    /// began, until one that's still there is waiting.
    pub fn pass_gone(&mut self, exists: impl Fn(NodeId) -> bool) {
        while self.current().is_some_and(|n| !exists(n)) {
            self.skip();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;
    use egui_node_graph2::NodeTemplateTrait;
    use crate::graph::{create_editor_state, SynthGraphEditorState, SynthNodeTemplate};

    /// A module added to `editor` at `at`, inside `parent`.
    fn add(editor: &mut SynthGraphEditorState, module_id: &str, at: Pos2, parent: Option<GroupId>) -> NodeId {
        let template = SynthNodeTemplate::from_module_id(module_id).unwrap();
        let mut user_state = SynthGraphState::new();
        let node_id = editor.graph.add_node(
            template.node_graph_label(&mut user_state),
            template.user_data(&mut user_state),
            |graph, node_id| template.build_node(graph, &mut user_state, node_id),
        );
        editor.graph[node_id].user_data.parent = parent;
        editor.node_positions.insert(node_id, at);
        node_id
    }

    fn nodes(n: usize) -> Vec<NodeId> {
        let mut editor = create_editor_state();
        (0..n).map(|_| add(&mut editor, pad::ID, Pos2::ZERO, None)).collect()
    }

    #[test]
    fn a_walk_steps_through_the_groups_own_pads_by_where_they_sit() {
        let mut editor = create_editor_state();
        let (id, other) = (GroupId(7), GroupId(8));
        // Placed second row first, at a zoom of 2
        let lower = add(&mut editor, pad::ID, pos2(0.0, 580.0), Some(id));
        let right = add(&mut editor, pad::ID, pos2(500.0, 10.0), Some(id));
        let left = add(&mut editor, pad::ID, pos2(0.0, 0.0), Some(id));
        add(&mut editor, "osc.sine", pos2(250.0, 0.0), Some(id));
        add(&mut editor, pad::ID, pos2(250.0, 0.0), Some(other));
        let positions = &editor.node_positions;
        assert_eq!(steps(&editor.graph, |n| positions.get(n).copied(), id, 2.0), [left, right, lower]);
        assert_eq!(count(&editor.graph, id), 3);
    }

    #[test]
    fn rows_read_top_to_bottom_each_left_to_right() {
        // Two rows of four, the second row's pads a little out of line, given shuffled
        let at = |row: f32, col: f32, nudge: f32| pos2(col * 250.0, row * 290.0 + nudge);
        let placed = vec![
            ("6", at(1.0, 1.0, 12.0)),
            ("1", at(0.0, 0.0, 0.0)),
            ("4", at(0.0, 3.0, -5.0)),
            ("8", at(1.0, 3.0, -20.0)),
            ("2", at(0.0, 1.0, 30.0)),
            ("5", at(1.0, 0.0, 0.0)),
            ("3", at(0.0, 2.0, 0.0)),
            ("7", at(1.0, 2.0, 40.0)),
        ];
        assert_eq!(reading_order(placed, ROW_SLACK), ["1", "2", "3", "4", "5", "6", "7", "8"]);
        // A column reads top to bottom
        assert_eq!(reading_order(vec![("b", pos2(0.0, 300.0)), ("a", pos2(4.0, 0.0))], ROW_SLACK), ["a", "b"]);
    }

    #[test]
    fn a_walk_takes_each_in_turn_and_marks_them() {
        let pads = nodes(3);
        let mut walk = LearnWalk::new(GroupId(1), "Pads", pads.clone()).unwrap();
        assert_eq!(walk.current(), Some(pads[0]));
        assert_eq!(walk.mark(pads[0]), Some(Mark::Now));
        assert_eq!(walk.mark(pads[2]), Some(Mark::Next(3)));

        walk.take(vec![(1, 40.0), (2, 10.0)]);
        walk.skip();
        assert_eq!(walk.mark(pads[0]), Some(Mark::Learned));
        assert_eq!(walk.mark(pads[1]), Some(Mark::Skipped));
        assert_eq!(walk.mark(pads[2]), Some(Mark::Now));
        assert_eq!(walk.progress(), (2, 3));
        assert_eq!(walk.taken(), [vec![(1, 40.0), (2, 10.0)]]);

        walk.take(vec![(1, 41.0), (2, 10.0)]);
        assert!(walk.is_over());
        assert_eq!(walk.current(), None);
        assert_eq!(walk.learned(), 2);
        assert_eq!(walk.progress(), (3, 3));
        // Nothing more to take
        walk.take(vec![(1, 42.0)]);
        assert_eq!(walk.learned(), 2);
    }

    #[test]
    fn modules_gone_are_passed_and_nothing_makes_no_walk() {
        let pads = nodes(3);
        let mut walk = LearnWalk::new(GroupId(1), "Pads", pads.clone()).unwrap();
        walk.pass_gone(|n| n == pads[2]);
        assert_eq!(walk.current(), Some(pads[2]));
        assert_eq!(walk.mark(pads[1]), Some(Mark::Skipped));
        assert!(LearnWalk::new(GroupId(1), "Empty", Vec::new()).is_none());
    }
}
