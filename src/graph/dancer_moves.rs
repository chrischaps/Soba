//! The Dancer's choreography: where a stick figure's head, hands and feet
//! are at a moment in the music.
//!
//! A [`Pose`] says where the hips, hands and feet want to be; [`joints`]
//! turns it into a figure, solving each arm and leg so its two bones keep
//! their lengths whatever the pose asks. Each style has four dance steps,
//! each a pose that loops every two beats, and the figure changes step every
//! four bars, picked from the phrase so the same patch always dances the
//! same routine. A hit in the music dips the hips; an accent adds a big
//! move: a jump, a spin, or arms flung up.
//!
//! The figure lives in its own units, y up: the floor is y = 0 and a figure
//! standing is about 1 tall. Everything it does stays inside [`BOUNDS`].

use std::f32::consts::{PI, TAU};

use eframe::egui::{vec2, Pos2, Rect, Vec2};

/// The box every pose stays inside, in figure units.
pub const BOUNDS: Rect = Rect { min: Pos2 { x: -0.62, y: -0.01 }, max: Pos2 { x: 0.62, y: 1.4 } };

const SHIN: f32 = 0.24;
const THIGH: f32 = 0.24;
const SPINE: f32 = 0.30;
/// The shoulders sit this far up the spine.
const SHOULDERS: f32 = 0.9;
const NECK: f32 = 0.10;
/// The head's radius.
pub const HEAD: f32 = 0.075;
const UPPER_ARM: f32 = 0.17;
const FOREARM: f32 = 0.16;

/// The share of a limb's length that [`reach`] eases into.
const SOFT: f32 = 0.08;

/// How high the hips stand, knees soft.
const STAND: f32 = 0.46;

/// Beats in a phrase: the figure changes step every four bars.
pub const PHRASE: u32 = 16;

/// An arm, as angles. Arms are posed by where the hand goes but kept as
/// angles, so an arm moving from one pose to the next swings round its
/// shoulder and elbow rather than pulling its hand in a straight line or
/// flipping its elbow over.
///
/// Angles are measured from pointing straight out to the arm's own side,
/// up positive, so both arms read alike. The upper arm's angle never wraps
/// where an arm can't point (up and in, across the face), and the elbow is
/// kept as a bend from the upper arm, so a blend of two arms never goes the
/// long way round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arm {
    /// Where the upper arm points.
    pub upper: f32,
    /// How far the forearm turns from the upper arm.
    pub bend: f32,
}

/// Upper arm angles run from here (up and in, the way an arm can't point)
/// round to just short of a full turn later.
const ARM_SEAM: f32 = 0.75 * PI;

impl Arm {
    /// The arm whose hand is at `hand`, from the shoulder and on the arm's
    /// own side (x out), its elbow bending toward `bend`.
    fn reaching(hand: Vec2, bend: Vec2) -> Arm {
        let (elbow, end) = reach(Vec2::ZERO, hand, UPPER_ARM, FOREARM, bend);
        let fore = end - elbow;
        Arm::new(elbow.y.atan2(elbow.x), fore.y.atan2(fore.x))
    }

    /// The arm with its upper arm and forearm pointing at these angles.
    fn new(upper: f32, fore: f32) -> Arm {
        let upper = ARM_SEAM - (ARM_SEAM - upper).rem_euclid(TAU);
        let bend = (fore - upper + PI).rem_euclid(TAU) - PI;
        Arm { upper, bend }
    }

    fn lerp(&self, other: &Arm, t: f32) -> Arm {
        Arm { upper: self.upper + (other.upper - self.upper) * t, bend: self.bend + (other.bend - self.bend) * t }
    }
}

/// Where a figure wants to be. Hands are posed relative to the shoulders;
/// the hips and feet are on the floor's axes. The legs bend to reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub hip: Vec2,
    /// How far the body leans, in radians: positive to the right.
    pub lean: f32,
    /// How far the head tilts from the body, in radians.
    pub head: f32,
    /// Screen-left arm, then screen-right.
    pub arms: [Arm; 2],
    /// Screen-left foot, then screen-right.
    pub feet: [Vec2; 2],
    /// How far round the figure has turned, in radians (a spin).
    pub turn: f32,
}

impl Pose {
    /// Standing easy.
    pub fn stand() -> Self {
        Self {
            hip: vec2(0.0, STAND),
            lean: 0.0,
            head: 0.0,
            arms: hands([vec2(-0.07, -0.30), vec2(0.07, -0.30)]),
            feet: [vec2(-0.09, 0.0), vec2(0.09, 0.0)],
            turn: 0.0,
        }
    }

    /// Part way from `self` to `other`.
    pub fn lerp(&self, other: &Pose, t: f32) -> Pose {
        let v = |a: Vec2, b: Vec2| a + (b - a) * t;
        let f = |a: f32, b: f32| a + (b - a) * t;
        Pose {
            hip: v(self.hip, other.hip),
            lean: f(self.lean, other.lean),
            head: f(self.head, other.head),
            arms: [self.arms[0].lerp(&other.arms[0], t), self.arms[1].lerp(&other.arms[1], t)],
            feet: [v(self.feet[0], other.feet[0]), v(self.feet[1], other.feet[1])],
            turn: f(self.turn, other.turn),
        }
    }
}

/// A figure: where every joint is, in figure units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Joints {
    pub head: Vec2,
    pub neck: Vec2,
    pub shoulders: Vec2,
    pub hip: Vec2,
    pub elbows: [Vec2; 2],
    pub hands: [Vec2; 2],
    pub knees: [Vec2; 2],
    pub feet: [Vec2; 2],
}

impl Joints {
    /// Every joint, for drawing a trail or checking bounds.
    pub fn all(&self) -> [Vec2; 12] {
        [
            self.head, self.neck, self.shoulders, self.hip, self.elbows[0], self.elbows[1], self.hands[0], self.hands[1], self.knees[0],
            self.knees[1], self.feet[0], self.feet[1],
        ]
    }

    /// The figure turned `turn` radians about its hips, seen from the front:
    /// side-on it's a line, and half way round it's facing away.
    pub fn turned(&self, turn: f32) -> Joints {
        let squash = turn.cos();
        let axis = self.hip.x;
        let t = |p: Vec2| vec2(axis + (p.x - axis) * squash, p.y);
        Joints {
            head: t(self.head),
            neck: t(self.neck),
            shoulders: t(self.shoulders),
            hip: t(self.hip),
            elbows: self.elbows.map(t),
            hands: self.hands.map(t),
            knees: self.knees.map(t),
            feet: self.feet.map(t),
        }
    }
}

/// Up the body, leaning `angle` radians to the right.
fn up(angle: f32) -> Vec2 {
    vec2(angle.sin(), angle.cos())
}

/// A two-bone limb from `root` reaching for `target`, its middle joint
/// bending toward `bend`: the joint and where the end lands.
///
/// Near full stretch the limb eases off rather than locking straight (the
/// last [`SOFT`] of its length is approached but never reached), because
/// there the joint swings a long way for a small move of the target, and a
/// knee would snap. Out of reach, the limb points at the target.
fn reach(root: Vec2, target: Vec2, first: f32, second: f32, bend: Vec2) -> (Vec2, Vec2) {
    let to = target - root;
    let length = to.length();
    let along = if length > 1e-5 { to / length } else { vec2(0.0, -1.0) };
    let full = first + second;
    let soft = SOFT * full;
    let eased = if length > full - soft { full - soft * (-(length - (full - soft)) / soft).exp() } else { length };
    let d = eased.clamp((first - second).abs() + 1e-4, full - 1e-4);
    let a = (first * first - second * second + d * d) / (2.0 * d);
    let h = (first * first - a * a).max(0.0).sqrt();
    let across = vec2(-along.y, along.x);
    let side = if across.dot(bend) >= 0.0 { 1.0 } else { -1.0 };
    let joint = root + along * a + across * (h * side);
    (joint, root + along * d)
}

/// Where a pose puts every joint.
pub fn joints(pose: &Pose) -> Joints {
    let hip = pose.hip;
    let neck = hip + up(pose.lean) * SPINE;
    let shoulders = hip + up(pose.lean) * (SPINE * SHOULDERS);
    let head = neck + up(pose.lean + pose.head) * NECK;
    let mut joints = Joints { head, neck, shoulders, hip, elbows: [shoulders; 2], hands: [shoulders; 2], knees: [hip; 2], feet: [hip; 2] };
    for side in 0..2 {
        let out = if side == 0 { -1.0 } else { 1.0 };
        let arm = pose.arms[side];
        let fore = arm.upper + arm.bend;
        let elbow = shoulders + vec2(out * arm.upper.cos(), arm.upper.sin()) * UPPER_ARM;
        let hand = elbow + vec2(out * fore.cos(), fore.sin()) * FOREARM;
        // Knees bend out and a little up
        let (knee, foot) = reach(hip, pose.feet[side], THIGH, SHIN, vec2(out, 0.3));
        joints.elbows[side] = elbow;
        joints.hands[side] = hand;
        joints.knees[side] = knee;
        joints.feet[side] = foot;
    }
    joints
}

/// An arm posed by its angles in degrees, measured from pointing straight
/// out to its side, up positive: (0, 90) is a goal post, (-90, 0) an L at
/// the waist.
fn arm(upper: f32, fore: f32) -> Arm {
    Arm::new(upper.to_radians(), fore.to_radians())
}

/// Arms with their hands at `targets` (screen-left, then screen-right),
/// from the shoulders, elbows bending out and down.
fn hands(targets: [Vec2; 2]) -> [Arm; 2] {
    let out = |target: Vec2, x: f32| vec2(target.x * x, target.y);
    [Arm::reaching(out(targets[0], -1.0), vec2(1.0, -0.5)), Arm::reaching(targets[1], vec2(1.0, -0.5))]
}

/// 0 to 1, eased at both ends.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// How far down the beat has pushed: 1 on the beat, 0 half way to the next.
fn dip(p: f32) -> f32 {
    ((1.0 + (TAU * p).cos()) * 0.5).powf(1.5)
}

/// A hit on the beat: there in `attack` of a beat, and gone again by the
/// next.
fn hit(p: f32, attack: f32) -> f32 {
    smooth(p / attack) * (1.0 - smooth((p - attack) / (1.0 - attack)))
}

/// Moving to a new place quickly at the start of a beat, then holding:
/// `fraction` is how much of the beat the move takes.
fn snap(p: f32, fraction: f32) -> f32 {
    smooth(p / fraction)
}

/// A step that hits a new pose on each beat: `frames[b]` is the one for
/// beat `b`, reached `fraction` of the way into it.
fn on_beats(t: f32, frames: &[Pose], fraction: f32) -> Pose {
    let n = frames.len();
    let beat = (t.floor() as usize) % n;
    let p = t.fract();
    frames[(beat + n - 1) % n].lerp(&frames[beat], snap(p, fraction))
}

/// A Robot step: a new pose every half beat, snapped to.
fn on_half_beats(t: f32, frames: &[Pose; 4]) -> Pose {
    on_beats(t * 2.0, frames, 0.16)
}

/// A dance step: a pose for `t`, 0 to 2 beats into its loop, as big as `amp`.
type Step = fn(t: f32, amp: f32) -> Pose;

/// The styles' steps, in the order of the Style dropdown. Each style's
/// first is the step it starts a dance with.
const STEPS: [[Step; 4]; 4] = [
    [bob, side_step, running_man, jumping_jack],
    [fever, hip_sway, roll, bump],
    [tut, head_tick, wave, slide],
    [whip, air_guitar, fist_pump, stomp],
];

// Bounce

/// Knees bob on the beat and the fists pump on the off-beat.
fn bob(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let down = dip(p);
    let pump = 0.1 * amp * (1.0 - down);
    let mut pose = Pose::stand();
    pose.hip.y -= 0.06 * amp * down;
    pose.head = 0.08 * amp * (PI * t).sin();
    pose.arms = hands([vec2(-0.13, -0.17 + pump), vec2(0.13, -0.17 + pump)]);
    pose.feet = [vec2(-0.1, 0.0), vec2(0.1, 0.0)];
    pose
}

/// Step right, step left, clapping on each step.
fn side_step(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let x = 0.07 * amp * (PI * t).cos();
    let mut pose = Pose::stand();
    pose.hip = vec2(x, STAND - 0.04 * amp * dip(p));
    pose.lean = -0.6 * x;
    // The foot that follows lifts as it comes
    let trailing = if (t as usize) % 2 == 0 { 0 } else { 1 };
    for side in 0..2 {
        let out = if side == 0 { -1.0 } else { 1.0 };
        let lift = if side == trailing { 0.05 * amp * (PI * p).sin() } else { 0.0 };
        pose.feet[side] = vec2(x * 1.4 + out * 0.11, lift);
    }
    let apart = 0.02 + 0.13 * (1.0 - dip(p));
    pose.arms = hands([vec2(-apart, -0.08), vec2(apart, -0.08)]);
    pose
}

/// Knees up in turn, arms swinging against them.
fn running_man(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let lifting = (t as usize) % 2;
    let mut pose = Pose::stand();
    pose.hip.y -= 0.035 * amp * dip(p);
    let mut targets = [Vec2::ZERO; 2];
    for side in 0..2 {
        let out = if side == 0 { -1.0 } else { 1.0 };
        let lift = if side == lifting { 0.14 * amp * (PI * p).sin() } else { 0.0 };
        pose.feet[side] = vec2(out * (0.09 - 0.5 * lift), lift);
        let swing = (PI * t + side as f32 * PI).cos();
        targets[side] = vec2(out * 0.12, -0.2 + 0.1 * amp * swing);
    }
    pose.arms = hands(targets);
    pose.lean = 0.05 * amp * (PI * t).sin();
    pose
}

/// Out with arms up, in with arms down, a hop each beat.
fn jumping_jack(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let out = (t as usize) % 2 == 0;
    // How open the jack is, moving through each hop
    let open = if out { smooth(p / 0.6) } else { 1.0 - smooth(p / 0.6) };
    let hop = 0.07 * amp * (PI * p).sin().max(0.0);
    let mut pose = Pose::stand();
    pose.hip.y += hop - 0.03 * amp * dip(p);
    let wide = 0.08 + 0.12 * amp * open;
    pose.feet = [vec2(-wide, hop), vec2(wide, hop)];
    let down = hands([vec2(-0.09, -0.29), vec2(0.09, -0.29)]);
    let raised = hands([vec2(-0.2, 0.24), vec2(0.2, 0.24)]);
    let open = open * (0.4 + 0.6 * amp);
    pose.arms = [down[0].lerp(&raised[0], open), down[1].lerp(&raised[1], open)];
    pose
}

// Disco

/// Point up and out, then down and across, the other hand on the hip.
fn fever(t: f32, amp: f32) -> Pose {
    let mut up_right = Pose::stand();
    up_right.hip = vec2(-0.03 * amp, STAND - 0.02);
    up_right.lean = -0.1 * amp;
    up_right.head = 0.2 * amp;
    let on_hip = vec2(-0.08, -0.25);
    up_right.arms = hands([on_hip, vec2(0.17 + 0.08 * amp, 0.2 + 0.08 * amp)]);
    up_right.feet = [vec2(-0.1, 0.0), vec2(0.12, 0.0)];
    let mut down_left = up_right;
    down_left.hip = vec2(0.02 * amp, STAND - 0.05 * amp);
    down_left.lean = 0.08 * amp;
    down_left.head = -0.15 * amp;
    down_left.arms = hands([on_hip, vec2(-0.16 - 0.06 * amp, -0.2 - 0.06 * amp)]);
    on_beats(t, &[up_right, down_left], 0.35)
}

/// Hands on hips, swinging them side to side.
fn hip_sway(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let sway = (PI * t).sin();
    let mut pose = Pose::stand();
    pose.hip = vec2(0.06 * amp * sway, STAND - 0.03 * amp * dip(p));
    pose.lean = -0.14 * amp * sway;
    pose.head = 0.2 * amp * sway;
    pose.arms = hands([vec2(-0.1, -0.24), vec2(0.1, -0.24)]);
    pose.feet = [vec2(-0.1, 0.0), vec2(0.1, 0.0)];
    pose
}

/// Forearms rolling round each other, the roll drifting side to side.
fn roll(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let drift = 0.06 * amp * (PI * t).cos();
    let r = 0.035 + 0.03 * amp;
    let angle = TAU * t;
    let mut pose = Pose::stand();
    pose.hip = vec2(drift * 0.6, STAND - 0.03 * amp * dip(p));
    pose.lean = drift;
    let centre = vec2(drift, -0.17 + 0.03 * amp);
    pose.arms = hands([centre + vec2(angle.cos(), angle.sin()) * r - vec2(0.02, 0.0), centre - vec2(angle.cos(), angle.sin()) * r + vec2(0.02, 0.0)]);
    pose
}

/// A bump of the hips to one side on each beat, arms swung the other way.
fn bump(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let side = if (t as usize) % 2 == 0 { 1.0 } else { -1.0 };
    let push = side * (0.02 + 0.05 * amp) * hit(p, 0.12);
    let mut pose = Pose::stand();
    pose.hip = vec2(push, STAND - 0.02 * amp);
    pose.lean = -1.5 * push;
    pose.head = push;
    // The arms swing away from the bump
    let level = hands([vec2(-0.16, -0.12), vec2(0.16, -0.12)]);
    let swung = hands([vec2(-0.16 - 0.1 * side, -0.12), vec2(0.16 - 0.1 * side, -0.12)]);
    let swing = push.abs() / 0.07;
    pose.arms = [level[0].lerp(&swung[0], swing), level[1].lerp(&swung[1], swing)];
    pose.feet = [vec2(-0.11, 0.0), vec2(0.11, 0.0)];
    pose
}

// Robot

/// Boxy arms, a new right angle every half beat.
fn tut(t: f32, amp: f32) -> Pose {
    let rest = Pose::stand();
    // Quieter, the boxes stay closer to the sides
    let base = |left: (f32, f32), right: (f32, f32)| {
        let boxed = [arm(left.0, left.1), arm(right.0, right.1)];
        Pose {
            arms: [rest.arms[0].lerp(&boxed[0], 0.6 + 0.4 * amp), rest.arms[1].lerp(&boxed[1], 0.6 + 0.4 * amp)],
            hip: vec2(0.0, STAND - 0.015 * amp),
            ..rest
        }
    };
    let frames = [
        base((-90.0, 0.0), (0.0, 90.0)),
        base((0.0, 90.0), (-90.0, 180.0)),
        base((-90.0, 180.0), (0.0, -90.0)),
        base((0.0, 90.0), (0.0, 90.0)),
    ];
    on_half_beats(t, &frames)
}

/// The head clicks from side to side, elbows at right angles.
fn head_tick(t: f32, amp: f32) -> Pose {
    let base = |head: f32, lean: f32| Pose {
        head: head * (0.5 + 0.5 * amp),
        lean,
        arms: [arm(-75.0, 10.0), arm(-75.0, 10.0)],
        ..Pose::stand()
    };
    let frames = [base(0.3, 0.0), base(0.0, 0.04 * amp), base(-0.3, 0.0), base(0.0, -0.04 * amp)];
    on_half_beats(t, &frames)
}

/// A wave passed along the arms, fingertips to fingertips, a half beat at
/// a time: arms out, each joint lifting in turn.
fn wave(t: f32, amp: f32) -> Pose {
    let a = 0.5 + 0.5 * amp;
    let base = |left: (f32, f32), right: (f32, f32), lean: f32| Pose {
        arms: [arm(left.0 * a, left.1 * a), arm(right.0 * a, right.1 * a)],
        lean: lean * amp,
        ..Pose::stand()
    };
    let frames = [
        base((0.0, 40.0), (-5.0, -5.0), 0.0),
        base((25.0, -10.0), (-5.0, -5.0), -0.04),
        base((-5.0, -5.0), (25.0, -10.0), 0.04),
        base((-5.0, -5.0), (0.0, 40.0), 0.0),
    ];
    on_half_beats(t, &frames)
}

/// Sliding sideways in jerks, one foot catching up each half beat.
fn slide(t: f32, amp: f32) -> Pose {
    let base = |x: f32, lifted: Option<usize>| {
        let mut pose = Pose::stand();
        pose.hip = vec2(x * amp, STAND - 0.02);
        pose.arms = hands([vec2(-0.08, -0.16), vec2(0.15, -0.0)]);
        for side in 0..2 {
            let out = if side == 0 { -1.0 } else { 1.0 };
            let lift = if lifted == Some(side) { 0.04 * amp } else { 0.0 };
            pose.feet[side] = vec2(x * amp + out * 0.1, lift);
        }
        pose
    };
    let frames = [base(0.04, Some(1)), base(0.08, None), base(0.04, Some(0)), base(0.0, None)];
    on_half_beats(t, &frames)
}

// Headbang

/// The head whips down on each beat, knees with it.
fn whip(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let side = if (t as usize) % 2 == 0 { 1.0 } else { -1.0 };
    let hit = hit(p, 0.08);
    let mut pose = Pose::stand();
    pose.hip.y -= 0.07 * amp * hit;
    pose.lean = side * 0.08 * amp * hit;
    pose.head = side * (0.15 + 0.4 * amp) * hit;
    pose.arms = hands([vec2(-0.12, -0.26), vec2(0.12, -0.26)]);
    pose.feet = [vec2(-0.15, 0.0), vec2(0.15, 0.0)];
    pose
}

/// One hand up the neck of the guitar, the other strumming twice a beat.
fn air_guitar(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let strum = dip((2.0 * p).fract());
    let mut pose = Pose::stand();
    pose.hip.y -= 0.05 * amp * dip(p);
    pose.lean = -0.12 * amp;
    pose.head = 0.3 * amp * dip(p) - 0.1;
    pose.arms = hands([vec2(-0.25, -0.06), vec2(0.05, -0.22 + 0.07 * amp * strum)]);
    pose.feet = [vec2(-0.16, 0.0), vec2(0.14, 0.0)];
    pose
}

/// A fist punched up on every beat.
fn fist_pump(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let punch = 1.0 - smooth((p - 0.15) / 0.6);
    let rise = snap(p, 0.12) * punch;
    let mut pose = Pose::stand();
    pose.hip.y -= 0.05 * amp * dip(p);
    pose.head = -0.2 * amp * rise;
    let cocked = hands([vec2(-0.09, -0.25), vec2(0.1, -0.1)]);
    let punched = hands([vec2(-0.09, -0.25), vec2(0.14, 0.3)]);
    let rise = rise * (0.5 + 0.5 * amp);
    pose.arms = [cocked[0], cocked[1].lerp(&punched[1], rise)];
    pose.feet = [vec2(-0.13, 0.0), vec2(0.13, 0.0)];
    pose
}

/// A wide stance, stamping one foot down on each beat, arms hanging heavy.
fn stomp(t: f32, amp: f32) -> Pose {
    let p = t.fract();
    let stamping = (t as usize) % 2;
    // Up on the off-beat, slammed down on the beat
    let lift = 0.09 * amp * (TAU * (p - 0.5)).cos().mul_add(0.5, 0.5).powi(2) * smooth(p / 0.2);
    let mut pose = Pose::stand();
    pose.hip.y -= 0.03 + 0.06 * amp * dip(p);
    pose.lean = if stamping == 1 { -0.06 } else { 0.06 } * amp * lift / 0.09;
    pose.arms = hands([vec2(-0.2, -0.24), vec2(0.2, -0.24)]);
    pose.head = 0.15 * amp * dip(p);
    let next = 1 - stamping;
    for side in 0..2 {
        let out = if side == 0 { -1.0 } else { 1.0 };
        pose.feet[side] = vec2(out * 0.18, if side == next { lift } else { 0.0 });
    }
    pose
}

/// A small integer hash, for picking steps.
fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// The step a style dances in `phrase`: its first step to begin with, then
/// a different one each phrase, the same every time.
pub fn step_in(style: usize, phrase: u32) -> usize {
    let n = STEPS[0].len() as u32;
    let mut step = 0;
    for p in 1..=phrase {
        step = (step + 1 + mix(p ^ ((style as u32) << 20)) % (n - 1)) % n;
    }
    step as usize
}

/// A big move asked for by an accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BigMove {
    Jump,
    Spin,
    Reach,
}

/// Which big move accent number `count` (from 1) is, in a style.
pub fn big_move(style: usize, count: u32) -> BigMove {
    use BigMove::*;
    let order = match style {
        1 => [Spin, Reach, Jump],
        2 => [Reach, Jump, Spin],
        _ => [Jump, Reach, Spin],
    };
    order[(count.saturating_sub(1) % 3) as usize]
}

/// Where the music is, for the figure: from the Dancer's readout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    pub style: usize,
    /// Whole beats since the dance started (wrapping).
    pub beat: u32,
    /// How far through the beat, 0 to 1.
    pub phase: f32,
    /// How loud the music is, 0 to 1.
    pub energy: f32,
    /// A hit in the music, 0 to 1.
    pub punch: f32,
    /// The last accent's number (0 for none) and how many beats ago.
    pub accent: (u32, f32),
}

/// The figure's pose at a moment in a dance.
pub fn dance(m: &Moment) -> Pose {
    let style = m.style.min(STEPS.len() - 1);
    let amp = 0.3 + 0.7 * m.energy.clamp(0.0, 1.0);
    let phase = m.phase.clamp(0.0, 1.0);
    let t = (m.beat % 2) as f32 + phase;
    let phrase = m.beat / PHRASE;
    let step = STEPS[style][step_in(style, phrase)];
    let mut pose = step(t, amp);

    // A new step blends in from the last over the first quarter beat
    let blend = 0.25;
    if m.beat % PHRASE == 0 && phase < blend && phrase > 0 {
        let before = STEPS[style][step_in(style, phrase - 1)](t, amp);
        pose = before.lerp(&pose, smooth(phase / blend));
    }

    pose.hip.y -= 0.025 * m.punch.clamp(0.0, 1.0);
    let (count, age) = m.accent;
    if count > 0 {
        pose = with_big_move(pose, big_move(style, count), age);
    }
    pose
}

/// `pose` with a big move `age` beats in; it lasts a beat.
pub fn with_big_move(pose: Pose, big: BigMove, age: f32) -> Pose {
    if !(0.0..1.0).contains(&age) {
        return pose;
    }
    let arc = (PI * age).sin();
    match big {
        BigMove::Jump => {
            let lift = 0.2 * 4.0 * age * (1.0 - age);
            let mut pose = pose;
            pose.hip.y += lift;
            for foot in &mut pose.feet {
                foot.y += lift + 0.06 * arc;
                foot.x *= 1.0 - 0.3 * arc;
            }
            let mut up = pose;
            up.arms = hands([vec2(-0.16, 0.22), vec2(0.16, 0.22)]);
            pose.lerp(&up, arc)
        }
        BigMove::Spin => {
            let mut pose = pose;
            pose.turn = TAU * smooth(age);
            pose.hip.y += 0.04 * arc;
            for foot in &mut pose.feet {
                foot.y += 0.04 * arc;
            }
            let mut tucked = pose;
            tucked.arms = hands([vec2(-0.05, -0.12), vec2(0.05, -0.12)]);
            pose.lerp(&tucked, arc)
        }
        BigMove::Reach => {
            // Flung up fast, held, and let down
            let held = smooth(age / 0.2) * (1.0 - smooth((age - 0.6) / 0.4));
            let mut up = pose;
            up.arms = hands([vec2(-0.2, 0.25), vec2(0.2, 0.25)]);
            up.head = 0.0;
            up.lean = 0.0;
            up.hip.y += 0.03;
            pose.lerp(&up, held)
        }
    }
}

/// Standing about while there's no beat: breathing, shifting weight,
/// tapping a foot now and then, looking round. Music without a beat sets
/// it swaying, as far as `energy` says.
pub fn idle(time: f64, energy: f32) -> Pose {
    let s = time as f32;
    let mut pose = Pose::stand();
    // Breathing
    pose.hip.y -= 0.006 * (0.5 + 0.5 * (TAU * 0.22 * s).sin());
    // Weight shifting from foot to foot, slowly
    let weight = (TAU * 0.07 * s).sin();
    pose.hip.x = 0.015 * weight;
    pose.lean = -0.02 * weight;
    // Swaying to music without a beat
    let sway = (TAU * 0.35 * s).sin() * energy.clamp(0.0, 1.0);
    pose.hip.x += 0.04 * sway;
    pose.lean -= 0.08 * sway;
    pose.head = 0.12 * sway;
    // Every so often, a few taps of a foot
    let cycle = (s / 7.0).fract();
    if cycle > 0.55 && cycle < 0.85 {
        let tap = (TAU * 1.6 * s).sin().max(0.0);
        let window = (PI * (cycle - 0.55) / 0.3).sin();
        pose.feet[1].y = 0.025 * tap * window;
    }
    // And now and then a look round
    let look = (s / 11.0).fract();
    if look > 0.2 && look < 0.45 {
        pose.head += 0.18 * (PI * (look - 0.2) / 0.25).sin() * if (s / 11.0) as u32 % 2 == 0 { 1.0 } else { -1.0 };
    }
    pose
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_step() -> impl Iterator<Item = (usize, usize, Step)> {
        STEPS.iter().enumerate().flat_map(|(style, steps)| steps.iter().enumerate().map(move |(n, step)| (style, n, *step)))
    }

    fn bones(j: &Joints) -> Vec<f32> {
        let d = |a: Vec2, b: Vec2| (a - b).length();
        let mut lengths = vec![d(j.hip, j.neck), d(j.neck, j.head)];
        for side in 0..2 {
            lengths.extend([d(j.shoulders, j.elbows[side]), d(j.elbows[side], j.hands[side]), d(j.hip, j.knees[side]), d(j.knees[side], j.feet[side])]);
        }
        lengths
    }

    fn inside(j: &Joints) -> bool {
        j.all().iter().all(|p| p.x.is_finite() && p.y.is_finite() && BOUNDS.contains(p.to_pos2()))
            && j.head.y + HEAD <= BOUNDS.max.y
            && (j.head.x - HEAD) >= BOUNDS.min.x
            && (j.head.x + HEAD) <= BOUNDS.max.x
    }

    /// The furthest any joint moves between two poses.
    fn moved(a: &Pose, b: &Pose) -> f32 {
        let (a, b) = (joints(a).turned(a.turn), joints(b).turned(b.turn));
        a.all().iter().zip(b.all()).map(|(a, b)| (*a - b).length()).fold(0.0, f32::max)
    }

    #[test]
    fn test_reach_keeps_bone_lengths() {
        let root = vec2(0.0, 0.0);
        for target in [vec2(0.0, -0.1), vec2(0.3, 0.2), vec2(-1.0, 0.0), vec2(0.0, 0.0), vec2(0.05, 0.31)] {
            let (joint, end) = reach(root, target, 0.17, 0.16, vec2(1.0, -0.5));
            assert!(((joint - root).length() - 0.17).abs() < 1e-4, "{target:?}");
            assert!(((end - joint).length() - 0.16).abs() < 1e-4, "{target:?}");
        }
    }

    #[test]
    fn test_every_step_keeps_its_bones_and_stays_in_the_box() {
        let rest = bones(&joints(&Pose::stand()));
        for (style, n, step) in every_step() {
            for amp in [0.3, 0.65, 1.0] {
                for i in 0..400 {
                    let t = i as f32 / 200.0;
                    let j = joints(&step(t, amp));
                    for (a, b) in bones(&j).iter().zip(&rest) {
                        assert!((a - b).abs() < 1e-3, "style {style} step {n} at {t}: bone {a} not {b}");
                    }
                    assert!(inside(&j), "style {style} step {n} at {t}, amp {amp}: {j:?}");
                }
            }
        }
    }

    #[test]
    fn test_feet_stay_on_or_above_the_floor() {
        for (style, n, step) in every_step() {
            for i in 0..400 {
                let t = i as f32 / 200.0;
                let j = joints(&step(t, 1.0));
                for foot in j.feet {
                    assert!(foot.y >= -1e-4, "style {style} step {n} at {t}: a foot through the floor");
                }
            }
        }
    }

    #[test]
    fn test_every_step_loops_smoothly() {
        for (style, n, step) in every_step() {
            for amp in [0.3, 1.0] {
                // Across each beat and the loop's end, and through the middle
                let mut last = step(0.0, amp);
                for i in 1..=4000 {
                    let t = (i as f32 / 2000.0) % 2.0;
                    let now = step(t, amp);
                    let d = moved(&last, &now);
                    assert!(d < 0.03, "style {style} step {n} jumps {d} at {t}");
                    last = now;
                }
            }
        }
    }

    #[test]
    fn test_a_dance_flows_across_phrases_and_big_moves() {
        for style in 0..4 {
            let at = |tick: u32| {
                let beat = tick / 500;
                let phase = (tick % 500) as f32 / 500.0;
                let accent = if beat >= 20 { (1 + (beat - 20) / 4, (beat - 20) as f32 % 4.0 + phase) } else { (0, 16.0) };
                dance(&Moment { style, beat, phase, energy: 0.8, punch: 0.0, accent })
            };
            let mut last = at(0);
            for tick in 1..500 * 40 {
                let now = at(tick);
                let d = moved(&last, &now);
                assert!(d < 0.05, "style {style} jumps {d} at beat {}", tick as f32 / 500.0);
                assert!(inside(&joints(&now).turned(now.turn)), "style {style} at beat {}", tick as f32 / 500.0);
                last = now;
            }
        }
    }

    #[test]
    fn test_steps_change_every_phrase_and_repeat_the_same() {
        for style in 0..4 {
            assert_eq!(step_in(style, 0), 0, "a dance starts on its first step");
            for phrase in 1..64 {
                assert_ne!(step_in(style, phrase), step_in(style, phrase - 1), "style {style} phrase {phrase}");
            }
            let routine: Vec<usize> = (0..64).map(|p| step_in(style, p)).collect();
            assert_eq!(routine, (0..64).map(|p| step_in(style, p)).collect::<Vec<_>>());
            // Every step gets danced
            for step in 0..4 {
                assert!(routine.contains(&step), "style {style} never dances step {step}");
            }
        }
    }

    #[test]
    fn test_idle_is_calm_and_in_the_box() {
        let mut last = idle(0.0, 0.0);
        for i in 1..6000 {
            let pose = idle(i as f64 / 60.0, if i > 3000 { 0.8 } else { 0.0 });
            assert!(moved(&last, &pose) < 0.02, "idle jumps at {}", i as f64 / 60.0);
            assert!(inside(&joints(&pose)));
            last = pose;
        }
    }
}
