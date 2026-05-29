// REQ-STL-006: Behavior simulation (mouse/typing/scroll)
pub struct BehaviorSimulator {
    seed: u64,
}

impl BehaviorSimulator {
    pub fn new(seed: u64) -> Self {
        BehaviorSimulator { seed }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn generate_mouse_path(&self, x1: f64, y1: f64, x2: f64, y2: f64, steps: usize) -> Vec<(f64, f64)> {
        let mut path = Vec::with_capacity(steps);
        let mut rng = self.seed;

        for i in 0..steps {
            let t = i as f64 / (steps - 1) as f64;
            let base_x = x1 + (x2 - x1) * t;
            let base_y = y1 + (y2 - y1) * t;

            let offset_x = if i > 0 && i < steps - 1 {
                self.next_random(&mut rng) * 6.0 - 3.0
            } else {
                0.0
            };
            let offset_y = if i > 0 && i < steps - 1 {
                self.next_random(&mut rng) * 6.0 - 3.0
            } else {
                0.0
            };

            let cx = if i > 0 && i < steps - 1 {
                let mid_x = (x1 + x2) / 2.0;
                let mid_y = (y1 + y2) / 2.0;
                let ctrl_x = mid_x + offset_x * 10.0;
                let _ctrl_y = mid_y + offset_y * 10.0;
                let u = 1.0 - t;
                u * u * base_x + 2.0 * u * t * ctrl_x + t * t * base_x
            } else {
                base_x
            };
            let cy = if i > 0 && i < steps - 1 {
                let mid_y = (y1 + y2) / 2.0;
                let ctrl_y = mid_y + offset_y * 10.0;
                let u = 1.0 - t;
                u * u * base_y + 2.0 * u * t * ctrl_y + t * t * base_y
            } else {
                base_y
            };

            path.push((cx + offset_x, cy + offset_y));
        }

        path
    }

    pub fn generate_typing_delays(&self, count: usize) -> Vec<u64> {
        let mut delays = Vec::with_capacity(count);
        let mut rng = self.seed;
        for _ in 0..count {
            let r = self.next_random(&mut rng);
            let delay = 30.0 + r * 120.0;
            delays.push(delay as u64);
        }
        delays
    }

    pub fn generate_scroll_deltas(&self, total: f64, steps: usize) -> Vec<f64> {
        let mut deltas = Vec::with_capacity(steps);
        let mut rng = self.seed;

        let accel_phase = steps / 3;
        let decel_phase = steps / 3;

        for i in 0..steps {
            let base = total / steps as f64;
            let factor = if i < accel_phase {
                (i as f64 / accel_phase as f64).powi(2)
            } else if i >= steps - decel_phase {
                ((steps - i) as f64 / decel_phase as f64).powi(2)
            } else {
                1.0
            };
            let noise = self.next_random(&mut rng) * 0.2 - 0.1;
            deltas.push(base * factor * (1.0 + noise));
        }

        deltas
    }

    fn next_random(&self, state: &mut u64) -> f64 {
        *state = state.wrapping_mul(0x2545F4914F6CDD1D);
        *state ^= *state >> 33;
        *state = state.wrapping_mul(0x27D4EB2D1659B4D6);
        *state ^= *state >> 33;
        (*state as f64) / (u64::MAX as f64)
    }
}
