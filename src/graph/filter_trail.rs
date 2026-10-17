//! The trail along a filter display's floor: the last two seconds of the
//! sound, newest at the top, so a sweeping cutoff leaves its path behind.
//!
//! Each frame the band readings [`super::filter_spectrum`] takes go in, and
//! a row is kept every sixtieth of a second, each band's brightness its
//! level against the loudest band coming in. What comes out is green and
//! what came in a faint blue beneath, as in the plot above it. The display
//! draws it as one small texture, rebuilt each frame at the strip's height
//! in pixels, every pixel row the brightest of the rows it spans.

use std::cell::RefCell;

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};

use super::filter_spectrum::BANDS;

/// How long the trail remembers, in seconds.
pub const SECONDS: f32 = 2.0;

/// Rows kept a second, and so how many in all.
const ROWS_PER_SECOND: f32 = 60.0;
const ROWS: usize = (SECONDS * ROWS_PER_SECOND) as usize;

/// How fast a band's reading falls back, in seconds: quick, so a sweep
/// leaves a sharp line, but not so quick a frame without fresh samples
/// flickers it.
const FALL_SECONDS: f32 = 0.08;

/// How far below the loudest band coming in a band still glows, in dB.
const RANGE_DB: f32 = 36.0;

/// The oldest part of the trail fades away, from this far along it.
const FADE_FROM: f32 = 0.6;

/// One row: each band's brightness, 0 to 1, in and out.
#[derive(Clone, Copy)]
struct Row {
    input: [f32; BANDS],
    output: [f32; BANDS],
}

const DARK: Row = Row { input: [0.0; BANDS], output: [0.0; BANDS] };

/// One filter's trail.
pub struct Trail {
    rows: Box<[Row; ROWS]>,
    /// Where the newest row is; older ones run on from it, wrapping.
    newest: usize,
    /// What the next row is taken from, in dB.
    reading_in: [f32; BANDS],
    reading_out: [f32; BANDS],
    /// Time since the last row was kept, in seconds.
    since: f32,
    /// Rows kept since the last one with anything in it.
    dark_rows: usize,
    /// The texture the display last drew it with, and the context it
    /// belongs to: the plugin draws in a new one each time its editor opens.
    texture: RefCell<Option<(egui::Context, TextureHandle)>>,
}

impl Default for Trail {
    fn default() -> Self {
        Self {
            rows: Box::new([DARK; ROWS]),
            newest: 0,
            reading_in: [-120.0; BANDS],
            reading_out: [-120.0; BANDS],
            since: 0.0,
            dark_rows: ROWS,
            texture: RefCell::new(None),
        }
    }
}

impl Trail {
    /// Takes in a frame's band levels in dB, `dt` seconds on, and keeps a
    /// row for every sixtieth of a second passed. Each is scaled to
    /// `reference`, the loudest band coming in, and dimmed by `presence`,
    /// so near-silence stays dark.
    pub fn advance(&mut self, input: &[f32; BANDS], output: &[f32; BANDS], reference: f32, presence: f32, dt: f32) {
        let fall = 1.0 - (-dt / FALL_SECONDS).exp();
        for (reading, &level) in self.reading_in.iter_mut().zip(input).chain(self.reading_out.iter_mut().zip(output)) {
            *reading = if level > *reading { level } else { *reading + (level - *reading) * fall };
        }

        self.since += dt;
        let step = 1.0 / ROWS_PER_SECOND;
        let mut kept = 0;
        while self.since >= step {
            self.since -= step;
            if kept < ROWS {
                self.keep(reference, presence);
                kept += 1;
            }
        }
    }

    fn keep(&mut self, reference: f32, presence: f32) {
        let brightness = |db: f32| ((db - reference + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0) * presence;
        let mut row = DARK;
        for band in 0..BANDS {
            row.input[band] = brightness(self.reading_in[band]);
            row.output[band] = brightness(self.reading_out[band]);
        }
        let lit = row.input.iter().chain(&row.output).any(|&b| b > 0.0);
        self.dark_rows = if lit { 0 } else { self.dark_rows.saturating_add(1) };
        self.newest = (self.newest + ROWS - 1) % ROWS;
        self.rows[self.newest] = row;
    }

    /// Forgets it all, as when the display has been off screen.
    pub fn clear(&mut self) {
        if !self.is_dark() {
            self.rows.fill(DARK);
            self.dark_rows = ROWS;
        }
    }

    /// Whether there's nothing left in it to see.
    pub fn is_dark(&self) -> bool {
        self.dark_rows >= ROWS
    }

    /// The row `age` rows back from the newest.
    fn row(&self, age: usize) -> &Row {
        &self.rows[(self.newest + age) % ROWS]
    }

    /// The trail as `height` rows of pixels, one column a band, newest at
    /// the top.
    pub fn image(&self, height: usize, green: Color32, blue: Color32) -> ColorImage {
        let height = height.max(1);
        let mut image = ColorImage::new([BANDS, height], Color32::TRANSPARENT);
        let per_pixel = ROWS as f32 / height as f32;
        let mut input = [0.0f32; BANDS];
        let mut output = [0.0f32; BANDS];
        for y in 0..height {
            // The brightest of the rows this pixel spans, so a short pluck
            // isn't averaged away when two seconds share a few pixels
            let (from, to) = (y as f32 * per_pixel, (y + 1) as f32 * per_pixel);
            input.fill(0.0);
            output.fill(0.0);
            let mut age = from.floor() as usize;
            while (age as f32) < to && age < ROWS {
                let row = self.row(age);
                for band in 0..BANDS {
                    input[band] = input[band].max(row.input[band]);
                    output[band] = output[band].max(row.output[band]);
                }
                age += 1;
            }

            let along = (from + to) * 0.5 / ROWS as f32;
            let fade = 1.0 - smoothstep(FADE_FROM, 1.0, along);
            for band in 0..BANDS {
                image.pixels[y * BANDS + band] = shade(input[band] * fade, output[band] * fade, green, blue);
            }
        }
        image
    }

    /// Draws it into `rect`, its texture made or refreshed for the purpose.
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect, green: Color32, blue: Color32) {
        let height = (rect.height() * painter.ctx().pixels_per_point()).round() as usize;
        let image = self.image(height, green, blue);
        let mut texture = self.texture.borrow_mut();
        match texture.as_mut() {
            Some((ctx, texture)) if ctx == painter.ctx() => texture.set(image, TextureOptions::LINEAR),
            _ => *texture = Some((painter.ctx().clone(), painter.ctx().load_texture("filter trail", image, TextureOptions::LINEAR))),
        }
        let Some((_, texture)) = texture.as_ref() else { return };
        // A column a band: the bands share the plot's width evenly, as the texels do
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(texture.id(), rect, uv, Color32::WHITE);
    }
}

/// A pixel: what came in, a faint blue, under what went out, green, which
/// pales toward white at its loudest.
fn shade(input: f32, output: f32, green: Color32, blue: Color32) -> Color32 {
    let input = input.powf(2.0) * 0.18;
    let output = output.powf(2.2);
    let hot = smoothstep(0.8, 1.0, output) * 0.6;
    let lerp = |a: u8, b: u8, t: f32| a as f32 + (b as f32 - a as f32) * t;
    let pale = Color32::from_rgb(214, 255, 226);
    // Premultiplied: each colour's channels scaled by how much of it there is
    let add = |channel: fn(Color32) -> u8| {
        let out = lerp(channel(green), channel(pale), hot) * output;
        let under = channel(blue) as f32 * input * (1.0 - output);
        (out + under).round().min(255.0) as u8
    };
    let alpha = (output + input * (1.0 - output)).clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(add(|c| c.r()), add(|c| c.g()), add(|c| c.b()), (alpha * 255.0).round() as u8)
}

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: Color32 = Color32::from_rgb(80, 200, 120);
    const BLUE: Color32 = Color32::from_rgb(80, 140, 255);

    /// Levels with one band at `db` and the rest silent.
    fn one_band(band: usize, db: f32) -> [f32; BANDS] {
        let mut levels = [-120.0; BANDS];
        levels[band] = db;
        levels
    }

    fn brightest_band(image: &ColorImage, y: usize) -> usize {
        (0..BANDS).max_by_key(|&band| image.pixels[y * BANDS + band].a()).unwrap()
    }

    #[test]
    fn a_sweep_leaves_its_path_newest_at_the_top() {
        let mut trail = Trail::default();
        // A tone climbing from band 10 to band 70 over two seconds
        for frame in 0..120 {
            let band = 10 + frame / 2;
            let levels = one_band(band, -6.0);
            trail.advance(&levels, &levels, -6.0, 1.0, 1.0 / 60.0);
        }
        let image = trail.image(20, GREEN, BLUE);
        let top = brightest_band(&image, 0);
        let middle = brightest_band(&image, 10);
        assert!(top >= 66, "the newest row, at the top, is where it ended: band {top}");
        assert!((35..=45).contains(&middle), "a second back it was halfway: band {middle}");
        assert!(image.pixels[top].a() > 80, "and it's bright: {}", image.pixels[top].a());
    }

    #[test]
    fn the_oldest_end_fades_away() {
        let mut trail = Trail::default();
        let levels = one_band(40, -6.0);
        for _ in 0..120 {
            trail.advance(&levels, &levels, -6.0, 1.0, 1.0 / 60.0);
        }
        let image = trail.image(40, GREEN, BLUE);
        let top = image.pixels[40].a();
        let bottom = image.pixels[39 * BANDS + 40].a();
        assert!(top > 200 && bottom < top / 4, "bright at the top ({top}), faded at the bottom ({bottom})");
    }

    #[test]
    fn quiet_bands_and_near_silence_stay_dark() {
        let mut trail = Trail::default();
        let mut levels = one_band(20, -6.0);
        levels[60] = -6.0 - RANGE_DB - 3.0;
        trail.advance(&levels, &levels, -6.0, 1.0, 1.0 / 60.0);
        let image = trail.image(4, GREEN, BLUE);
        assert!(image.pixels[20].a() > 0);
        assert_eq!(image.pixels[60].a(), 0, "further down than the range is dark");

        let mut hush = Trail::default();
        hush.advance(&levels, &levels, -6.0, 0.0, 1.0 / 60.0);
        assert!(hush.is_dark(), "with nothing there to see, nothing is kept");
    }

    #[test]
    fn it_goes_dark_two_seconds_after_the_sound_stops() {
        let mut trail = Trail::default();
        let levels = one_band(40, -6.0);
        trail.advance(&levels, &levels, -6.0, 1.0, 1.0 / 60.0);
        assert!(!trail.is_dark());
        let silence = [-120.0; BANDS];
        for _ in 0..60 {
            trail.advance(&silence, &silence, -6.0, 1.0, 1.0 / 60.0);
        }
        assert!(!trail.is_dark(), "a second on, it's still scrolling out");
        for _ in 0..70 {
            trail.advance(&silence, &silence, -6.0, 1.0, 1.0 / 60.0);
        }
        assert!(trail.is_dark());
    }

    #[test]
    fn keeps_time_whatever_the_frame_rate() {
        let (mut fast, mut slow) = (Trail::default(), Trail::default());
        let levels = one_band(40, -6.0);
        for _ in 0..144 {
            fast.advance(&levels, &levels, -6.0, 1.0, 1.0 / 144.0);
        }
        for _ in 0..30 {
            slow.advance(&levels, &levels, -6.0, 1.0, 1.0 / 30.0);
        }
        let lit = |trail: &Trail| (0..ROWS).filter(|&age| trail.row(age).output[40] > 0.0).count();
        assert!(lit(&fast).abs_diff(60) <= 1 && lit(&slow).abs_diff(60) <= 1, "{} and {} rows in a second", lit(&fast), lit(&slow));
    }
}
