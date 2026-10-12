//! From One Sine is scored by two Arrangers. These tests play the patch's
//! own Clock into its Arrangers, block by block as the engine does, and
//! check that the song's sections start on the bars the score gives them,
//! to the sample.

use soba::dsp::{DspModule, ProcessContext, SignalBuffer};
use soba::modules::arranger::{Arranger, SECTIONS};
use soba::modules::Clock;
use soba::persistence::{patch_from_json, NodeData, Patch};

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;

fn from_one_sine() -> Patch {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("patches/from-one-sine.json");
    patch_from_json(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A module's parameters as the patch sets them, in parameter order.
fn params_of(module: &dyn DspModule, node: &NodeData) -> Vec<f32> {
    module
        .parameters()
        .iter()
        .map(|p| node.parameters.iter().find(|n| n.name == p.name).map_or(p.default, |n| n.value.as_f32()))
        .collect()
}

fn rises(signal: &[f32]) -> Vec<usize> {
    (0..signal.len()).filter(|&t| signal[t] > 0.5 && (t == 0 || signal[t - 1] <= 0.5)).collect()
}

struct Played {
    clock: Vec<f32>,
    section_trig: Vec<f32>,
    end: Vec<f32>,
    /// The Cues Arranger's first lane: the drum pattern.
    drums: Vec<f32>,
    /// Every lane's CV.
    lanes: Vec<Vec<f32>>,
}

/// Plays From One Sine's Clock into the Arranger whose first lane is named
/// `first_lane`, for `seconds`.
fn play(first_lane: &str, seconds: f32) -> (Played, Vec<f32>) {
    let patch = from_one_sine();
    let clock_node = patch.nodes.iter().find(|n| n.module_id == "util.clock").unwrap();
    let arranger_node = patch
        .nodes
        .iter()
        .find(|n| n.module_id == "seq.arranger" && n.labels.get("Lane 1").map(String::as_str) == Some(first_lane))
        .unwrap_or_else(|| panic!("an Arranger whose first lane is {first_lane}"));

    let mut clock = Clock::new();
    let mut arranger = Arranger::new();
    let clock_params = params_of(&clock, clock_node);
    let arranger_params = params_of(&arranger, arranger_node);
    clock.prepare(RATE, BLOCK);
    arranger.prepare(RATE, BLOCK);

    let total = (seconds * RATE) as usize;
    let mut played = Played { clock: Vec::new(), section_trig: Vec::new(), end: Vec::new(), drums: Vec::new(), lanes: vec![Vec::new(); 8] };
    let mut clock_out: Vec<SignalBuffer> = (0..3).map(|_| SignalBuffer::gate(BLOCK)).collect();
    let mut arranger_out: Vec<SignalBuffer> = (0..Arranger::OUT_COUNT).map(|_| SignalBuffer::control(BLOCK)).collect();
    let idle = SignalBuffer::gate(BLOCK);
    let ctx = ProcessContext::new(RATE, BLOCK);
    for _ in 0..total.div_ceil(BLOCK) {
        clock.process(&[], &mut clock_out, &clock_params, &ctx);
        let ctx = ProcessContext::with_transport(RATE, BLOCK, clock.transport(&clock_params).unwrap_or_default());
        arranger.process(&[&clock_out[0], &idle, &idle, &idle], &mut arranger_out, &arranger_params, &ctx);
        played.clock.extend_from_slice(&clock_out[0].samples);
        played.section_trig.extend_from_slice(&arranger_out[Arranger::OUT_SECTION_TRIG].samples);
        played.end.extend_from_slice(&arranger_out[Arranger::OUT_END].samples);
        played.drums.extend_from_slice(&arranger_out[Arranger::OUT_LANES].samples);
        for (lane, cv) in played.lanes.iter_mut().enumerate() {
            cv.extend_from_slice(&arranger_out[Arranger::OUT_LANES + 2 * lane].samples);
        }
    }
    (played, arranger_params)
}

/// Where each section starts, in bars from 0, and the song's length.
fn section_bars(params: &[f32]) -> (Vec<usize>, usize) {
    let mut starts = Vec::new();
    let mut bar = 0;
    for section in 0..Arranger::sections(params) {
        starts.push(bar);
        bar += Arranger::length(params, section);
    }
    (starts, bar)
}

#[test]
fn from_one_sine_starts_every_section_on_its_bar() {
    // The whole song and the downbeat it loops on
    let (played, params) = play("Drums", 276.0);
    assert_eq!(Arranger::steps(&params), 16, "a bar of the Clock's sixteenths");
    let (starts, bars) = section_bars(&params);
    assert_eq!(bars, 128);
    assert!(starts.len() <= SECTIONS);

    let clocks = rises(&played.clock);
    let expected: Vec<usize> = starts.iter().chain(std::iter::once(&bars)).map(|&bar| clocks[bar * 16]).collect();
    assert_eq!(rises(&played.section_trig), expected, "each section on its bar's first clock, then the loop");
    assert_eq!(rises(&played.end), [clocks[bars * 16]]);
}

#[test]
fn the_drop_lands_on_the_sample_of_bar_81() {
    let (played, params) = play("Drums", 175.0);
    let (starts, _) = section_bars(&params);
    // Everything, the drop, follows the four-bar Build at bar 81
    let drop = starts.iter().position(|&bar| bar == 80).expect("a section starts at bar 81");
    // Swing moves only the offbeat sixteenths, so a downbeat is where the
    // tempo puts it: 80 bars of four beats at 112 BPM
    let at = (80.0 * 4.0 * 60.0 / 112.0 * RATE as f64).round() as usize;
    let trig = rises(&played.section_trig);
    assert!(trig[drop].abs_diff(at) <= 1, "the drop at sample {}, bar 81 at {at}", trig[drop]);
    // On that sample the drums leave the Build's pattern D for the groove, C
    let t = trig[drop];
    assert!(played.drums[t - 1] >= 0.75, "pattern D before the drop: {}", played.drums[t - 1]);
    assert!((0.5..0.75).contains(&played.drums[t]), "pattern C from the drop: {}", played.drums[t]);
}

#[test]
fn the_desk_and_the_cues_keep_the_same_sections() {
    let patch = from_one_sine();
    let arrangers: Vec<&NodeData> = patch.nodes.iter().filter(|n| n.module_id == "seq.arranger").collect();
    assert_eq!(arrangers.len(), 2);
    let layouts: Vec<(Vec<usize>, Vec<Option<String>>)> = arrangers
        .iter()
        .map(|node| {
            let params = params_of(&Arranger::new(), node);
            let (starts, _) = section_bars(&params);
            let names = (1..=starts.len()).map(|s| node.labels.get(&format!("Section {s}")).cloned()).collect();
            (starts, names)
        })
        .collect();
    assert_eq!(layouts[0], layouts[1], "both Arrangers play the same song");
}

#[test]
fn the_cues_follow_the_desk() {
    let patch = from_one_sine();
    let named = |name: &str| {
        patch
            .nodes
            .iter()
            .find(|n| n.module_id == "seq.arranger" && n.labels.get("Name").map(String::as_str) == Some(name))
            .unwrap_or_else(|| panic!("an Arranger named {name}"))
            .id
    };
    let (desk, cues) = (named("Desk"), named("Cues"));
    let follows = patch.connections.iter().any(|c| c.from_node == desk && c.from_port == "Section" && c.to_node == cues && c.to_port == "Follow");
    assert!(follows, "Desk's Section is patched into Cues' Follow, so the editor keeps their sections the same");
}

#[test]
fn the_pad_enters_and_swells_through_the_opening() {
    // Desk's lane 6 is the pad: in at 28% on bar 5, up to 42% by bar 17
    let (played, _) = play("Sine", 40.0);
    let pad = &played.lanes[5];
    let clocks = rises(&played.clock);
    let (bar_5, bar_17) = (clocks[4 * 16], clocks[16 * 16]);
    assert_eq!(pad[bar_5 - 1], 0.0, "silent before it enters");
    // Its second of Glide brings it up to the start, then it rises steadily
    let settled = bar_5 + RATE as usize;
    assert!((pad[settled] - 0.28).abs() < 0.01, "in at 28%: {}", pad[settled]);
    for t in settled + 1..bar_17 {
        assert!(pad[t] >= pad[t - 1], "the swell falls at sample {t}");
    }
    let half_way = clocks[10 * 16];
    assert!((pad[half_way] - 0.35).abs() < 0.002, "half way at bar 11: {}", pad[half_way]);
    // There on bar 17's downbeat, where Pulse lifts it again
    assert!((pad[bar_17 - 1] - 0.42).abs() < 1e-4, "42% by bar 17: {}", pad[bar_17 - 1]);
}

#[test]
fn the_parts_fade_away_in_the_order_they_came() {
    // Desk's lanes: the sine, arp, bells, sparkles, bass, pad, wind and lead.
    // From bar 117 each part but the sine fades, and they leave in the
    // reverse of the order the opening brought them in
    let (played, _) = play("Sine", 275.0);
    let clocks = rises(&played.clock);
    let (bar_117, bar_129) = (clocks[116 * 16], clocks[128 * 16]);
    let settled = bar_117 + RATE as usize;
    let gone = |lane: usize| {
        let cv = &played.lanes[lane];
        assert!(cv[settled] > 0.1, "lane {} still playing as the sine returns", lane + 1);
        for t in settled + 1..bar_129 {
            assert!(cv[t] <= cv[t - 1], "lane {} rises at sample {t}", lane + 1);
        }
        assert_eq!(cv[bar_129 - 1], 0.0, "lane {} silent by the loop", lane + 1);
        (settled..bar_129).find(|&t| cv[t] < 0.01).unwrap()
    };
    let (wind, pad, sparkles, arp, bass, bells) = (gone(6), gone(5), gone(3), gone(1), gone(4), gone(2));
    assert!(bells < bass && bass < arp && arp < sparkles && sparkles < pad && pad < wind, "bells, bass, arp, sparkles, pad, wind");
    // Leaving the sine alone, as it began
    assert!(played.lanes[0][bar_129 - 1] > 0.6, "the sine at the end: {}", played.lanes[0][bar_129 - 1]);
}
