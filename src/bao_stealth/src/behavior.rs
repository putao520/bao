// @trace REQ-STL-006 [api:stealth behavior simulation]
// Human-like behavior simulation: Bezier mouse paths, rhythm typing, inertia scroll.
//
// All randomness is seed-based (deterministic). Firefox and Chrome profiles
// use different BehaviorConfig parameters to produce distinct behavior fingerprints.

use std::f64::consts::PI;

// ---------------------------------------------------------------------------
// Configuration — Firefox/Chrome behavior parameter differences
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BehaviorConfig {
    pub mouse: MouseConfig,
    pub keyboard: KeyboardConfig,
    pub scroll: ScrollConfig,
    pub click: ClickConfig,
}

#[derive(Debug, Clone)]
pub struct MouseConfig {
    /// Control point offset range (px)
    pub control_point_spread: f64,
    /// Fitts' Law coefficient a (intercept, ms)
    pub fitts_a: f64,
    /// Fitts' Law coefficient b (slope, ms/bit)
    pub fitts_b: f64,
    /// Min move time (ms)
    pub min_move_time_ms: f64,
    /// Max move time (ms)
    pub max_move_time_ms: f64,
    /// Jitter amplitude (px)
    pub jitter_amplitude: f64,
    /// Tremor amplitude (px)
    pub tremor_amplitude: f64,
    /// Sampling interval (ms)
    pub sampling_interval_ms: f64,
}

#[derive(Debug, Clone)]
pub struct KeyboardConfig {
    /// Base key interval mean (ms)
    pub base_interval_ms: f64,
    /// Key interval stddev (ms)
    pub interval_stddev_ms: f64,
    /// Thinking pause probability at word start
    pub thinking_pause_probability: f64,
    /// Thinking pause mean (ms)
    pub thinking_pause_mean_ms: f64,
    /// Thinking pause stddev (ms)
    pub thinking_pause_stddev_ms: f64,
    /// Typo probability
    pub typo_probability: f64,
    /// Typo correction delay (ms)
    pub typo_correction_delay_ms: f64,
    /// Word gap mean (ms)
    pub word_gap_mean_ms: f64,
    /// Word gap stddev (ms)
    pub word_gap_stddev_ms: f64,
    /// Punctuation pause mean (ms)
    pub punctuation_pause_ms: f64,
    /// Punctuation pause stddev (ms)
    pub punctuation_pause_stddev_ms: f64,
}

#[derive(Debug, Clone)]
pub struct ScrollConfig {
    /// Friction coefficient per step (0.90-0.98)
    pub friction: f64,
    /// Overshoot probability
    pub overshoot_probability: f64,
    /// Overshoot ratio (0.05-0.15)
    pub overshoot_ratio: f64,
    /// Stop threshold (px/step)
    pub stop_threshold: f64,
    /// Noise amplitude ratio
    pub noise_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct ClickConfig {
    /// Press duration mean (ms)
    pub press_duration_mean_ms: f64,
    /// Press duration stddev (ms)
    pub press_duration_stddev_ms: f64,
    /// Pre-click micro-move probability
    pub pre_click_move_probability: f64,
    /// Pre-click micro-move amplitude (px)
    pub pre_click_move_amplitude: f64,
    /// Double-click interval mean (ms)
    pub dbl_click_interval_mean_ms: f64,
    /// Double-click interval stddev (ms)
    pub dbl_click_interval_stddev_ms: f64,
    /// Move-to-click delay (ms)
    pub move_to_click_delay_ms: f64,
}

impl BehaviorConfig {
    pub fn firefox() -> Self {
        BehaviorConfig {
            mouse: MouseConfig {
                control_point_spread: 60.0,
                fitts_a: 50.0,
                fitts_b: 150.0,
                min_move_time_ms: 100.0,
                max_move_time_ms: 1500.0,
                jitter_amplitude: 2.0,
                tremor_amplitude: 0.8,
                sampling_interval_ms: 8.0,
            },
            keyboard: KeyboardConfig {
                base_interval_ms: 95.0,
                interval_stddev_ms: 28.0,
                thinking_pause_probability: 0.15,
                thinking_pause_mean_ms: 350.0,
                thinking_pause_stddev_ms: 120.0,
                typo_probability: 0.04,
                typo_correction_delay_ms: 100.0,
                word_gap_mean_ms: 150.0,
                word_gap_stddev_ms: 50.0,
                punctuation_pause_ms: 220.0,
                punctuation_pause_stddev_ms: 60.0,
            },
            scroll: ScrollConfig {
                friction: 0.94,
                overshoot_probability: 0.3,
                overshoot_ratio: 0.10,
                stop_threshold: 0.5,
                noise_ratio: 0.05,
            },
            click: ClickConfig {
                press_duration_mean_ms: 85.0,
                press_duration_stddev_ms: 18.0,
                pre_click_move_probability: 0.4,
                pre_click_move_amplitude: 2.0,
                dbl_click_interval_mean_ms: 300.0,
                dbl_click_interval_stddev_ms: 45.0,
                move_to_click_delay_ms: 30.0,
            },
        }
    }

    pub fn chrome() -> Self {
        BehaviorConfig {
            mouse: MouseConfig {
                control_point_spread: 40.0,
                fitts_a: 40.0,
                fitts_b: 120.0,
                min_move_time_ms: 80.0,
                max_move_time_ms: 1200.0,
                jitter_amplitude: 1.5,
                tremor_amplitude: 0.5,
                sampling_interval_ms: 8.0,
            },
            keyboard: KeyboardConfig {
                base_interval_ms: 85.0,
                interval_stddev_ms: 22.0,
                thinking_pause_probability: 0.12,
                thinking_pause_mean_ms: 300.0,
                thinking_pause_stddev_ms: 100.0,
                typo_probability: 0.03,
                typo_correction_delay_ms: 90.0,
                word_gap_mean_ms: 130.0,
                word_gap_stddev_ms: 40.0,
                punctuation_pause_ms: 200.0,
                punctuation_pause_stddev_ms: 50.0,
            },
            scroll: ScrollConfig {
                friction: 0.92,
                overshoot_probability: 0.25,
                overshoot_ratio: 0.08,
                stop_threshold: 0.5,
                noise_ratio: 0.04,
            },
            click: ClickConfig {
                press_duration_mean_ms: 75.0,
                press_duration_stddev_ms: 15.0,
                pre_click_move_probability: 0.3,
                pre_click_move_amplitude: 1.5,
                dbl_click_interval_mean_ms: 280.0,
                dbl_click_interval_stddev_ms: 40.0,
                move_to_click_delay_ms: 25.0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Event types for click/typing sequences
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ClickEventType {
    MouseDown,
    MouseUp,
    Click,
    DoubleClick,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClickEvent {
    pub event_type: ClickEventType,
    pub x: f64,
    pub y: f64,
    pub delay_after_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypingEvent {
    pub char: char,
    pub delay_before_ms: u64,
    pub is_backspace: bool,
}

// ---------------------------------------------------------------------------
// BehaviorSimulator — seed-based deterministic PRNG + behavior generation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BehaviorSimulator {
    seed: u64,
    pub config: BehaviorConfig,
}

impl BehaviorSimulator {
    pub fn new(seed: u64) -> Self {
        BehaviorSimulator {
            seed,
            config: BehaviorConfig::firefox(),
        }
    }

    pub fn with_config(seed: u64, config: BehaviorConfig) -> Self {
        BehaviorSimulator { seed, config }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    // -----------------------------------------------------------------------
    // PRNG: xorshift64 — fast, deterministic, good distribution
    // -----------------------------------------------------------------------

    fn next_random(&self, state: &mut u64) -> f64 {
        *state = state.wrapping_mul(0x2545F4914F6CDD1D);
        *state ^= *state >> 33;
        *state = state.wrapping_mul(0x27D4EB2D1659B4D6);
        *state ^= *state >> 33;
        (*state as f64) / (u64::MAX as f64)
    }

    /// Box-Muller transform — zero-dependency normal distribution
    fn normal_random(&self, state: &mut u64, mean: f64, stddev: f64) -> f64 {
        let u1 = self.next_random(state).max(1e-10); // avoid ln(0)
        let u2 = self.next_random(state);
        let z0 = (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
        mean + z0 * stddev
    }

    // -----------------------------------------------------------------------
    // Mouse: Cubic Bezier + Fitts' Law + ease-in-out speed curve
    // -----------------------------------------------------------------------

    /// Cubic Bezier interpolation: B(t) = (1-t)³P0 + 3(1-t)²tP1 + 3(1-t)t²P2 + t³P3
    fn cubic_bezier(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
        let u = 1.0 - t;
        u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
    }

    /// Ease-in-out speed curve: position(t) = (1 - cos(π*t)) / 2
    /// This produces natural acceleration at start and deceleration at end.
    fn ease_in_out(t: f64) -> f64 {
        (1.0 - (PI * t).cos()) / 2.0
    }

    /// Fitts' Law: T = a + b * log2(D/W + 1)
    /// D = distance, W = target width
    fn fitts_time(&self, distance: f64, target_width: f64) -> f64 {
        let w = target_width.max(1.0);
        let d = distance.max(1.0);
        let t = self.config.mouse.fitts_a + self.config.mouse.fitts_b * (d / w + 1.0).log2();
        t.clamp(self.config.mouse.min_move_time_ms, self.config.mouse.max_move_time_ms)
    }

    /// Generate human-like mouse path using cubic Bezier curves.
    /// Returns (x, y, time_ms) tuples with Fitts' Law timing and ease-in-out speed.
    pub fn generate_human_mouse_path(
        &self,
        start: (f64, f64),
        end: (f64, f64),
        target_width: f64,
    ) -> Vec<(f64, f64, f64)> {
        let (x1, y1) = start;
        let (x2, y2) = end;
        let distance = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();

        if distance < 1.0 {
            return vec![(x1, y1, 0.0)];
        }

        let total_time = self.fitts_time(distance, target_width);
        let steps = (total_time / self.config.mouse.sampling_interval_ms).ceil() as usize;
        let steps = steps.max(5);

        let mut rng = self.seed;

        // Generate 2 control points perpendicular to the line
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = distance;

        // Perpendicular direction
        let perp_x = -dy / len;
        let perp_y = dx / len;

        let spread = self.config.mouse.control_point_spread;

        // P1: ~1/3 along the line + perpendicular offset
        let offset1 = (self.next_random(&mut rng) - 0.5) * 2.0 * spread;
        let cx1 = x1 + dx * 0.33 + perp_x * offset1;
        let cy1 = y1 + dy * 0.33 + perp_y * offset1;

        // P2: ~2/3 along the line + perpendicular offset
        let offset2 = (self.next_random(&mut rng) - 0.5) * 2.0 * spread;
        let cx2 = x1 + dx * 0.66 + perp_x * offset2;
        let cy2 = y1 + dy * 0.66 + perp_y * offset2;

        let mut path = Vec::with_capacity(steps + 1);

        for i in 0..=steps {
            let t_linear = i as f64 / steps as f64;
            // Apply ease-in-out for speed variation
            let t = Self::ease_in_out(t_linear);

            let bx = Self::cubic_bezier(x1, cx1, cx2, x2, t);
            let by = Self::cubic_bezier(y1, cy1, cy2, y2, t);

            // Add jitter (random micro-movement)
            let jitter_x = if i > 0 && i < steps {
                (self.next_random(&mut rng) - 0.5) * self.config.mouse.jitter_amplitude
            } else {
                0.0
            };
            let jitter_y = if i > 0 && i < steps {
                (self.next_random(&mut rng) - 0.5) * self.config.mouse.jitter_amplitude
            } else {
                0.0
            };

            // Add tremor (sinusoidal micro-vibration)
            let tremor_phase = t_linear * 2.0 * PI * 8.0; // ~8 Hz tremor
            let tremor_x = self.config.mouse.tremor_amplitude * tremor_phase.cos();
            let tremor_y = self.config.mouse.tremor_amplitude * (tremor_phase + 1.5).sin();

            let time_ms = t_linear * total_time;

            path.push((
                bx + jitter_x + tremor_x,
                by + jitter_y + tremor_y,
                time_ms,
            ));
        }

        // Force exact start and end positions
        if let Some(first) = path.first_mut() {
            first.0 = x1;
            first.1 = y1;
            first.2 = 0.0;
        }
        if let Some(last) = path.last_mut() {
            last.0 = x2;
            last.1 = y2;
            last.2 = total_time;
        }

        path
    }

    /// Legacy API — delegates to cubic Bezier internally.
    /// Returns (x, y) pairs for backward compatibility.
    pub fn generate_mouse_path(&self, x1: f64, y1: f64, x2: f64, y2: f64, steps: usize) -> Vec<(f64, f64)> {
        let human_path = self.generate_human_mouse_path((x1, y1), (x2, y2), 20.0);
        // Resample to requested step count
        let total = human_path.len();
        if total <= 1 || steps == 0 {
            return vec![(x1, y1)];
        }

        let mut result = Vec::with_capacity(steps + 1);
        for i in 0..=steps {
            let idx_f = i as f64 / steps as f64 * (total - 1) as f64;
            let idx = idx_f as usize;
            let frac = idx_f - idx as f64;

            if idx + 1 < total {
                let (ax, ay, _) = human_path[idx];
                let (bx, by, _) = human_path[idx + 1];
                result.push((ax + (bx - ax) * frac, ay + (by - ay) * frac));
            } else {
                let (x, y, _) = human_path[total - 1];
                result.push((x, y));
            }
        }
        result
    }

    // -----------------------------------------------------------------------
    // Click: Three-phase simulation (move → micro-adjust → press → release)
    // -----------------------------------------------------------------------

    pub fn generate_click_sequence(&self, x: f64, y: f64, target_width: f64) -> Vec<ClickEvent> {
        let mut rng = self.seed;
        let mut events = Vec::new();

        // Pre-click micro-adjustment
        let micro_x = if self.next_random(&mut rng) < self.config.click.pre_click_move_probability {
            x + (self.next_random(&mut rng) - 0.5) * self.config.click.pre_click_move_amplitude
        } else {
            x
        };
        let micro_y = if self.next_random(&mut rng) < self.config.click.pre_click_move_probability {
            y + (self.next_random(&mut rng) - 0.5) * self.config.click.pre_click_move_amplitude
        } else {
            y
        };

        // Delay before mousedown (arrival settling)
        let settle_delay = self.config.click.move_to_click_delay_ms as u64 + (self.next_random(&mut rng) * 20.0) as u64;

        // MouseDown
        events.push(ClickEvent {
            event_type: ClickEventType::MouseDown,
            x: micro_x,
            y: micro_y,
            delay_after_ms: settle_delay,
        });

        // Press duration (Box-Muller normal distribution)
        let press_duration = self
            .normal_random(&mut rng, self.config.click.press_duration_mean_ms, self.config.click.press_duration_stddev_ms)
            .clamp(40.0, 200.0) as u64;

        // MouseUp
        events.push(ClickEvent {
            event_type: ClickEventType::MouseUp,
            x: micro_x,
            y: micro_y,
            delay_after_ms: press_duration,
        });

        // Click fires after mouseup
        let click_delay = (self.next_random(&mut rng) * 10.0) as u64;
        events.push(ClickEvent {
            event_type: ClickEventType::Click,
            x: micro_x,
            y: micro_y,
            delay_after_ms: click_delay,
        });

        let _ = target_width; // used by caller for move path generation
        events
    }

    pub fn generate_double_click_sequence(&self, x: f64, y: f64, target_width: f64) -> Vec<ClickEvent> {
        let first_click = self.generate_click_sequence(x, y, target_width);

        let mut rng = self.seed.wrapping_add(0xDEADBEEF);
        let dbl_interval = self
            .normal_random(&mut rng, self.config.click.dbl_click_interval_mean_ms, self.config.click.dbl_click_interval_stddev_ms)
            .clamp(150.0, 500.0) as u64;

        let second_click = self.generate_click_sequence(x, y, target_width);

        let mut events = first_click;
        // Add delay between first and second click
        if let Some(last) = events.last_mut() {
            last.delay_after_ms += dbl_interval;
        }
        events.extend(second_click);

        // Add dblclick event
        let micro_x = x + (self.next_random(&mut rng) - 0.5) * 1.0;
        let micro_y = y + (self.next_random(&mut rng) - 0.5) * 1.0;
        events.push(ClickEvent {
            event_type: ClickEventType::DoubleClick,
            x: micro_x,
            y: micro_y,
            delay_after_ms: 5,
        });

        events
    }

    // -----------------------------------------------------------------------
    // Keyboard: Human rhythm typing with thinking pauses and typo correction
    // -----------------------------------------------------------------------

    /// Generate human-like typing events for the given text.
    pub fn generate_human_typing(&self, text: &str) -> Vec<TypingEvent> {
        let mut rng = self.seed;
        let mut events = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let ch = chars[i];
            let is_word_start = i == 0
                || chars[i - 1] == ' '
                || chars[i - 1] == '\n';
            let is_after_punct = i > 0 && is_punctuation(chars[i - 1]);

            // Decide if this character triggers a typo
            let typo_roll = self.next_random(&mut rng);
            if typo_roll < self.config.keyboard.typo_probability && ch.is_alphabetic() {
                // Insert wrong character
                let wrong_char = random_adjacent_char(ch, &mut rng);
                let wrong_delay = self.normal_random(
                    &mut rng,
                    self.config.keyboard.base_interval_ms,
                    self.config.keyboard.interval_stddev_ms,
                ).clamp(20.0, 300.0) as u64;

                events.push(TypingEvent {
                    char: wrong_char,
                    delay_before_ms: wrong_delay,
                    is_backspace: false,
                });

                // Backspace after typo
                let bs_delay = self.config.keyboard.typo_correction_delay_ms as u64
                    + (self.next_random(&mut rng) * 60.0) as u64;
                events.push(TypingEvent {
                    char: '\u{0008}', // backspace
                    delay_before_ms: bs_delay,
                    is_backspace: true,
                });

                // Now type the correct character
                let correct_delay = self.normal_random(
                    &mut rng,
                    self.config.keyboard.base_interval_ms,
                    self.config.keyboard.interval_stddev_ms,
                ).clamp(20.0, 300.0) as u64;
                events.push(TypingEvent {
                    char: ch,
                    delay_before_ms: correct_delay,
                    is_backspace: false,
                });
            } else {
                // Normal character with appropriate delay
                let delay = if is_word_start && self.next_random(&mut rng) < self.config.keyboard.thinking_pause_probability {
                    // Thinking pause
                    self.normal_random(
                        &mut rng,
                        self.config.keyboard.thinking_pause_mean_ms,
                        self.config.keyboard.thinking_pause_stddev_ms,
                    ).clamp(100.0, 800.0) as u64
                } else if is_after_punct {
                    // Punctuation pause
                    self.normal_random(
                        &mut rng,
                        self.config.keyboard.punctuation_pause_ms,
                        self.config.keyboard.punctuation_pause_stddev_ms,
                    ).clamp(50.0, 500.0) as u64
                } else if ch == ' ' {
                    // Word gap
                    self.normal_random(
                        &mut rng,
                        self.config.keyboard.word_gap_mean_ms,
                        self.config.keyboard.word_gap_stddev_ms,
                    ).clamp(30.0, 400.0) as u64
                } else {
                    // Regular key interval (Box-Muller normal)
                    self.normal_random(
                        &mut rng,
                        self.config.keyboard.base_interval_ms,
                        self.config.keyboard.interval_stddev_ms,
                    ).clamp(20.0, 300.0) as u64
                };

                events.push(TypingEvent {
                    char: ch,
                    delay_before_ms: delay,
                    is_backspace: false,
                });
            }

            i += 1;
        }

        events
    }

    /// Legacy API — simple typing delays for backward compatibility.
    pub fn generate_typing_delays(&self, count: usize) -> Vec<u64> {
        let text: String = "a".repeat(count);
        let events = self.generate_human_typing(&text);
        events.iter().map(|e| e.delay_before_ms).collect()
    }

    // -----------------------------------------------------------------------
    // Scroll: Inertia-based with friction decay and overshoot
    // -----------------------------------------------------------------------

    /// Generate inertia scroll deltas. Velocity decays by friction each step.
    /// May include overshoot (scroll past target then bounce back).
    pub fn generate_inertia_scroll(&self, initial_speed: f64) -> Vec<f64> {
        let mut rng = self.seed;
        let mut deltas = Vec::new();

        let mut speed = initial_speed.abs();
        let direction = if initial_speed >= 0.0 { 1.0 } else { -1.0 };
        let friction = self.config.scroll.friction;
        let threshold = self.config.scroll.stop_threshold;

        // Main scroll phase
        while speed > threshold {
            let noise = 1.0 + (self.next_random(&mut rng) - 0.5) * 2.0 * self.config.scroll.noise_ratio;
            deltas.push(direction * speed * noise);
            speed *= friction;
        }

        // Overshoot phase
        let overshoot_roll = self.next_random(&mut rng);
        if overshoot_roll < self.config.scroll.overshoot_probability && !deltas.is_empty() {
            let total: f64 = deltas.iter().sum();
            let overshoot_amount = total.abs() * self.config.scroll.overshoot_ratio;
            let mut overshoot_speed = overshoot_amount * 0.3; // small initial speed for bounce

            // Overshoot forward
            while overshoot_speed > threshold * 0.5 {
                let noise = 1.0 + (self.next_random(&mut rng) - 0.5) * 2.0 * self.config.scroll.noise_ratio;
                deltas.push(direction * overshoot_speed * noise);
                overshoot_speed *= friction;
            }

            // Bounce back
            let mut bounce_speed = overshoot_amount * 0.2;
            while bounce_speed > threshold * 0.3 {
                let noise = 1.0 + (self.next_random(&mut rng) - 0.5) * 2.0 * self.config.scroll.noise_ratio;
                deltas.push(-direction * bounce_speed * noise);
                bounce_speed *= friction;
            }
        }

        deltas
    }

    /// Legacy API — simple scroll deltas for backward compatibility.
    pub fn generate_scroll_deltas(&self, total: f64, steps: usize) -> Vec<f64> {
        if steps == 0 {
            return Vec::new();
        }
        let speed = (total / steps as f64) * 3.0; // approximate initial speed
        let deltas = self.generate_inertia_scroll(speed);
        if deltas.is_empty() {
            return vec![total / steps as f64; steps];
        }
        // Normalize to match total
        let sum: f64 = deltas.iter().sum();
        if sum.abs() < f64::EPSILON {
            return vec![total / steps as f64; steps];
        }
        let scale = total / sum;
        deltas.iter().map(|d| d * scale).collect()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_punctuation(ch: char) -> bool {
    matches!(ch, '.' | ',' | '!' | '?' | ';' | ':' | '-' | '(' | ')' | '"' | '\'')
}

/// Generate a random adjacent key on QWERTY keyboard for typo simulation
fn random_adjacent_char(ch: char, rng: &mut u64) -> char {
    let neighbors: &str = match ch.to_ascii_lowercase() {
        'a' => "qwsz",
        'b' => "vghn",
        'c' => "xdfv",
        'd' => "serfc",
        'e' => "wrsd",
        'f' => "drtgc",
        'g' => "ftyhv",
        'h' => "gyujb",
        'i' => "ujko",
        'j' => "huikn",
        'k' => "jiolm",
        'l' => "kop",
        'm' => "njk",
        'n' => "bhjm",
        'o' => "iklp",
        'p' => "ol",
        'q' => "wa",
        'r' => "edft",
        's' => "wadxe",
        't' => "rfgy",
        'u' => "yhji",
        'v' => "cfgb",
        'w' => "qase",
        'x' => "zsdc",
        'y' => "tghu",
        'z' => "xsa",
        _ => return ch,
    };
    let bytes = neighbors.as_bytes();
    if bytes.is_empty() {
        return ch;
    }
    // Simple PRNG step for index
    *rng = rng.wrapping_mul(0x2545F4914F6CDD1D);
    *rng ^= *rng >> 33;
    let idx = (*rng as usize) % bytes.len();
    let neighbor = bytes[idx] as char;
    if ch.is_uppercase() { neighbor.to_ascii_uppercase() } else { neighbor }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_seed() {
        let sim = BehaviorSimulator::new(42);
        assert_eq!(sim.seed, 42);
    }

    #[test]
    fn seed_getter() {
        let sim = BehaviorSimulator::new(99);
        assert_eq!(sim.seed(), 99);
    }

    // ---- Cubic Bezier mouse path ----

    #[test]
    fn human_mouse_path_start_is_exact() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_human_mouse_path((10.0, 20.0), (500.0, 300.0), 20.0);
        let (x, y, t) = path[0];
        assert!((x - 10.0).abs() < 1e-9);
        assert!((y - 20.0).abs() < 1e-9);
        assert!((t - 0.0).abs() < 1e-9);
    }

    #[test]
    fn human_mouse_path_end_is_exact() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_human_mouse_path((0.0, 0.0), (500.0, 300.0), 20.0);
        let (x, y, _) = path[path.len() - 1];
        assert!((x - 500.0).abs() < 1e-9, "end x={x}");
        assert!((y - 300.0).abs() < 1e-9, "end y={y}");
    }

    #[test]
    fn human_mouse_path_deterministic() {
        let sim = BehaviorSimulator::new(12345);
        let p1 = sim.generate_human_mouse_path((0.0, 0.0), (800.0, 600.0), 20.0);
        let p2 = sim.generate_human_mouse_path((0.0, 0.0), (800.0, 600.0), 20.0);
        assert_eq!(p1, p2);
    }

    #[test]
    fn human_mouse_path_different_seed() {
        let s1 = BehaviorSimulator::new(1);
        let s2 = BehaviorSimulator::new(2);
        let p1 = s1.generate_human_mouse_path((0.0, 0.0), (800.0, 600.0), 20.0);
        let p2 = s2.generate_human_mouse_path((0.0, 0.0), (800.0, 600.0), 20.0);
        assert_ne!(p1, p2);
    }

    #[test]
    fn human_mouse_path_has_speed_variation() {
        let sim = BehaviorSimulator::new(42);
        let path = sim.generate_human_mouse_path((0.0, 0.0), (1000.0, 0.0), 20.0);
        // Speed at start and end should be lower than in middle (ease-in-out)
        assert!(path.len() > 5);
        let start_speed = distance_between(&path[0], &path[1]);
        let mid_idx = path.len() / 2;
        let mid_speed = distance_between(&path[mid_idx], &path[mid_idx + 1]);
        let end_speed = distance_between(&path[path.len() - 2], &path[path.len() - 1]);
        // Middle speed should be greater than start and end speeds
        assert!(mid_speed > start_speed * 0.8, "mid_speed={} should be > start_speed={}", mid_speed, start_speed);
        assert!(mid_speed > end_speed * 0.8, "mid_speed={} should be > end_speed={}", mid_speed, end_speed);
    }

    #[test]
    fn human_mouse_path_fitts_longer_distance_more_time() {
        let sim = BehaviorSimulator::new(1);
        let short = sim.generate_human_mouse_path((0.0, 0.0), (100.0, 0.0), 20.0);
        let long = sim.generate_human_mouse_path((0.0, 0.0), (1000.0, 0.0), 20.0);
        let short_time = short.last().unwrap().2;
        let long_time = long.last().unwrap().2;
        assert!(long_time > short_time, "long_time={} should be > short_time={}", long_time, short_time);
    }

    #[test]
    fn human_mouse_path_short_distance() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_human_mouse_path((0.0, 0.0), (0.5, 0.5), 20.0);
        assert_eq!(path.len(), 1); // sub-1px distance → single point
    }

    // ---- Legacy mouse path ----

    #[test]
    fn legacy_mouse_path_deterministic() {
        let sim = BehaviorSimulator::new(77);
        let p1 = sim.generate_mouse_path(0.0, 0.0, 800.0, 600.0, 15);
        let p2 = sim.generate_mouse_path(0.0, 0.0, 800.0, 600.0, 15);
        assert_eq!(p1, p2);
    }

    #[test]
    fn legacy_mouse_path_correct_length() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
        assert_eq!(path.len(), 11);
    }

    // ---- Click sequence ----

    #[test]
    fn click_sequence_has_three_phases() {
        let sim = BehaviorSimulator::new(42);
        let events = sim.generate_click_sequence(100.0, 200.0, 20.0);
        assert!(events.len() >= 3);
        assert_eq!(events[0].event_type, ClickEventType::MouseDown);
        assert_eq!(events[1].event_type, ClickEventType::MouseUp);
        assert_eq!(events[2].event_type, ClickEventType::Click);
    }

    #[test]
    fn click_sequence_press_duration_reasonable() {
        let sim = BehaviorSimulator::new(42);
        let events = sim.generate_click_sequence(100.0, 200.0, 20.0);
        let press_duration = events[0].delay_after_ms; // mousedown→mouseup delay
        assert!(press_duration >= 40 && press_duration <= 200,
            "press_duration={} out of range [40, 200]", press_duration);
    }

    #[test]
    fn click_sequence_deterministic() {
        let sim = BehaviorSimulator::new(42);
        let e1 = sim.generate_click_sequence(100.0, 200.0, 20.0);
        let e2 = sim.generate_click_sequence(100.0, 200.0, 20.0);
        assert_eq!(e1.len(), e2.len());
        for (a, b) in e1.iter().zip(e2.iter()) {
            assert_eq!(a.event_type, b.event_type);
            assert_eq!(a.delay_after_ms, b.delay_after_ms);
        }
    }

    #[test]
    fn double_click_has_six_plus_events() {
        let sim = BehaviorSimulator::new(42);
        let events = sim.generate_double_click_sequence(100.0, 200.0, 20.0);
        assert!(events.len() >= 7); // mousedown+mouseup+click × 2 + dblclick
        assert!(events.iter().any(|e| e.event_type == ClickEventType::DoubleClick));
    }

    // ---- Human typing ----

    #[test]
    fn human_typing_has_correct_chars() {
        let sim = BehaviorSimulator::new(42);
        let events = sim.generate_human_typing("hello");
        // Extract non-backspace chars
        let typed: String = events.iter()
            .filter(|e| !e.is_backspace)
            .map(|e| e.char)
            .collect();
        assert!(typed.contains("hello"), "typed='{}'", typed);
    }

    #[test]
    fn human_typing_all_delays_positive() {
        let sim = BehaviorSimulator::new(42);
        let events = sim.generate_human_typing("The quick brown fox.");
        assert!(events.iter().all(|e| e.delay_before_ms > 0));
    }

    #[test]
    fn human_typing_deterministic() {
        let sim = BehaviorSimulator::new(77);
        let e1 = sim.generate_human_typing("test text");
        let e2 = sim.generate_human_typing("test text");
        assert_eq!(e1.len(), e2.len());
        for (a, b) in e1.iter().zip(e2.iter()) {
            assert_eq!(a.char, b.char);
            assert_eq!(a.delay_before_ms, b.delay_before_ms);
            assert_eq!(a.is_backspace, b.is_backspace);
        }
    }

    #[test]
    fn human_typing_different_seed() {
        let s1 = BehaviorSimulator::new(1);
        let s2 = BehaviorSimulator::new(2);
        let e1 = s1.generate_human_typing("hello");
        let e2 = s2.generate_human_typing("hello");
        assert_ne!(e1, e2);
    }

    #[test]
    fn human_typing_word_gap_larger_than_intra_word() {
        let sim = BehaviorSimulator::new(42);
        let events = sim.generate_human_typing("ab cd");
        // Find the space character event
        let space_event = events.iter().find(|e| e.char == ' ' && !e.is_backspace);
        let intra_event = events.iter().find(|e| e.char == 'a' && !e.is_backspace);
        if let (Some(space), Some(intra)) = (space_event, intra_event) {
            assert!(space.delay_before_ms >= intra.delay_before_ms,
                "word gap {} should be >= intra-word {}", space.delay_before_ms, intra.delay_before_ms);
        }
    }

    // ---- Inertia scroll ----

    #[test]
    fn inertia_scroll_deltas_converge() {
        let sim = BehaviorSimulator::new(42);
        let deltas = sim.generate_inertia_scroll(30.0);
        assert!(!deltas.is_empty());
        // Speed should decrease (deltas get smaller toward end)
        let last = deltas.last().unwrap().abs();
        let first = deltas.first().unwrap().abs();
        assert!(last < first, "last={} should be < first={}", last, first);
    }

    #[test]
    fn inertia_scroll_deterministic() {
        let sim = BehaviorSimulator::new(77);
        let d1 = sim.generate_inertia_scroll(25.0);
        let d2 = sim.generate_inertia_scroll(25.0);
        assert_eq!(d1, d2);
    }

    #[test]
    fn inertia_scroll_different_seed() {
        let s1 = BehaviorSimulator::new(1);
        let s2 = BehaviorSimulator::new(2);
        let d1 = s1.generate_inertia_scroll(25.0);
        let d2 = s2.generate_inertia_scroll(25.0);
        assert_ne!(d1, d2);
    }

    // ---- Legacy scroll deltas ----

    #[test]
    fn legacy_scroll_deltas_sum_approximately_total() {
        let sim = BehaviorSimulator::new(1);
        let total = 1000.0;
        let deltas = sim.generate_scroll_deltas(total, 30);
        let sum: f64 = deltas.iter().sum();
        assert!((sum - total).abs() / total < 0.5, "sum={sum}");
    }

    // ---- Firefox vs Chrome config ----

    #[test]
    fn firefox_chrome_configs_differ() {
        let ff = BehaviorConfig::firefox();
        let ch = BehaviorConfig::chrome();
        assert_ne!(ff.mouse.fitts_b, ch.mouse.fitts_b);
        assert_ne!(ff.keyboard.base_interval_ms, ch.keyboard.base_interval_ms);
        assert_ne!(ff.scroll.friction, ch.scroll.friction);
        assert_ne!(ff.click.press_duration_mean_ms, ch.click.press_duration_mean_ms);
    }

    #[test]
    fn firefox_chrome_mouse_paths_differ() {
        let ff = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
        let ch = BehaviorSimulator::with_config(42, BehaviorConfig::chrome());
        let ff_path = ff.generate_human_mouse_path((0.0, 0.0), (500.0, 300.0), 20.0);
        let ch_path = ch.generate_human_mouse_path((0.0, 0.0), (500.0, 300.0), 20.0);
        assert_ne!(ff_path, ch_path);
    }

    #[test]
    fn firefox_chrome_typing_delays_differ() {
        let ff = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
        let ch = BehaviorSimulator::with_config(42, BehaviorConfig::chrome());
        let ff_events = ff.generate_human_typing("hello world");
        let ch_events = ch.generate_human_typing("hello world");
        assert_ne!(ff_events, ch_events);
    }

    // ---- Box-Muller normal distribution ----

    #[test]
    fn normal_random_produces_reasonable_values() {
        let sim = BehaviorSimulator::new(42);
        let mut rng = sim.seed;
        let mut sum = 0.0;
        let n = 1000;
        for _ in 0..n {
            let v = sim.normal_random(&mut rng, 100.0, 20.0);
            sum += v;
        }
        let mean = sum / n as f64;
        assert!((mean - 100.0).abs() < 5.0, "mean={mean} too far from 100.0");
    }

    // ---- Helpers ----

    fn distance_between(a: &(f64, f64, f64), b: &(f64, f64, f64)) -> f64 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }
}
