// REQ-STL-006: Behavior simulation (mouse/typing/scroll)  @trace REQ-STL-006
#[derive(Debug, Clone)]
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
        // Returns steps+1 points: start + steps intermediate points including end
        let total_points = steps + 1;
        let mut path = Vec::with_capacity(total_points);
        let mut rng = self.seed;

        for i in 0..total_points {
            let t = i as f64 / steps as f64;
            let base_x = x1 + (x2 - x1) * t;
            let base_y = y1 + (y2 - y1) * t;

            let offset_x = if i > 0 && i < total_points - 1 {
                self.next_random(&mut rng) * 6.0 - 3.0
            } else {
                0.0
            };
            let offset_y = if i > 0 && i < total_points - 1 {
                self.next_random(&mut rng) * 6.0 - 3.0
            } else {
                0.0
            };

            let cx = if i > 0 && i < total_points - 1 {
                let mid_x = (x1 + x2) / 2.0;
                let mid_y = (y1 + y2) / 2.0;
                let ctrl_x = mid_x + offset_x * 10.0;
                let _ctrl_y = mid_y + offset_y * 10.0;
                let u = 1.0 - t;
                u * u * base_x + 2.0 * u * t * ctrl_x + t * t * base_x
            } else {
                base_x
            };
            let cy = if i > 0 && i < total_points - 1 {
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

#[cfg(test)]
mod tests {
    // @trace REQ-STL-006 [req:REQ-STL-006] [level:unit]
    use super::BehaviorSimulator;

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

    #[test]
    fn mouse_path_returns_steps_plus_one_points() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
        assert_eq!(path.len(), 11);
    }

    #[test]
    fn mouse_path_start_point_is_x1_y1() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_mouse_path(10.0, 20.0, 100.0, 200.0, 10);
        let (x, y) = path[0];
        assert!((x - 10.0).abs() < 1e-9);
        assert!((y - 20.0).abs() < 1e-9);
    }

    #[test]
    fn mouse_path_end_point_approximately_x2_y2() {
        let sim = BehaviorSimulator::new(1);
        let path = sim.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
        let (x, y) = path[path.len() - 1];
        assert!((x - 500.0).abs() < 10.0, "end x={x}, expected ~500.0");
        assert!((y - 300.0).abs() < 10.0, "end y={y}, expected ~300.0");
    }

    #[test]
    fn mouse_path_deterministic_same_seed() {
        let sim = BehaviorSimulator::new(12345);
        let p1 = sim.generate_mouse_path(0.0, 0.0, 800.0, 600.0, 15);
        let p2 = sim.generate_mouse_path(0.0, 0.0, 800.0, 600.0, 15);
        assert_eq!(p1, p2);
    }

    #[test]
    fn mouse_path_different_seed_different_path() {
        let s1 = BehaviorSimulator::new(1);
        let s2 = BehaviorSimulator::new(2);
        let p1 = s1.generate_mouse_path(0.0, 0.0, 800.0, 600.0, 15);
        let p2 = s2.generate_mouse_path(0.0, 0.0, 800.0, 600.0, 15);
        assert_ne!(p1, p2);
    }

    #[test]
    fn typing_delays_returns_count_elements() {
        let sim = BehaviorSimulator::new(1);
        let delays = sim.generate_typing_delays(20);
        assert_eq!(delays.len(), 20);
    }

    #[test]
    fn typing_delays_all_positive() {
        let sim = BehaviorSimulator::new(1);
        let delays = sim.generate_typing_delays(50);
        assert!(delays.iter().all(|&d| d > 0));
    }

    #[test]
    fn typing_delays_deterministic() {
        let sim = BehaviorSimulator::new(77);
        let d1 = sim.generate_typing_delays(30);
        let d2 = sim.generate_typing_delays(30);
        assert_eq!(d1, d2);
    }

    #[test]
    fn scroll_deltas_returns_steps_elements() {
        let sim = BehaviorSimulator::new(1);
        let deltas = sim.generate_scroll_deltas(500.0, 12);
        assert_eq!(deltas.len(), 12);
    }

    #[test]
    fn scroll_deltas_sum_approximately_equals_total() {
        let sim = BehaviorSimulator::new(1);
        let total = 1000.0;
        let steps = 30;
        let deltas = sim.generate_scroll_deltas(total, steps);
        let sum: f64 = deltas.iter().sum();
        assert!(
            (sum - total).abs() / total < 0.5,
            "sum={sum}, expected ~{total}"
        );
    }

    #[test]
    fn scroll_deltas_all_finite() {
        let sim = BehaviorSimulator::new(1);
        let deltas = sim.generate_scroll_deltas(500.0, 20);
        assert!(deltas.iter().all(|d| d.is_finite()));
    }
}
