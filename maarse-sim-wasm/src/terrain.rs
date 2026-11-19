//! Ground heights for radio links.
//!
//! A height grid (whole metres, one byte per cell) on a regular lat/lon
//! grid, loaded once per page with the `terrain` call and shared by every
//! engine built afterwards. A scenario with `"terrain": true` then:
//! * adds the ground height to the antenna heights for the radio horizon, and
//! * adds diffraction loss over hills: Deygout's method (ITU-R P.526) with
//!   the main obstacle and one obstacle on each side, earth curvature k = 4/3.
//!
//! Trees and buildings are not in the grid; they stay clutter areas.

use std::sync::{Arc, RwLock};

pub struct Grid {
    pub lat0: f64,
    pub lon0: f64,
    pub dlat: f64,
    pub dlon: f64,
    pub w: usize,
    pub h: usize,
    pub z: Vec<u8>,
}

static GRID: RwLock<Option<Arc<Grid>>> = RwLock::new(None);

pub fn set(g: Option<Grid>) {
    *GRID.write().unwrap() = g.map(Arc::new);
}
pub fn get() -> Option<Arc<Grid>> {
    GRID.read().unwrap().clone()
}

impl Grid {
    /// Ground height (m) at a point, bilinear; 0 outside the grid (sea).
    pub fn at(&self, lat: f64, lon: f64) -> f64 {
        let fy = (lat - self.lat0) / self.dlat;
        let fx = (lon - self.lon0) / self.dlon;
        if fx < 0.0 || fy < 0.0 || fx >= (self.w - 1) as f64 || fy >= (self.h - 1) as f64 {
            return 0.0;
        }
        let (x, y) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - x as f64, fy - y as f64);
        let v = |x: usize, y: usize| self.z[y * self.w + x] as f64;
        let top = v(x, y) * (1.0 - tx) + v(x + 1, y) * tx;
        let bot = v(x, y + 1) * (1.0 - tx) + v(x + 1, y + 1) * tx;
        top * (1.0 - ty) + bot * ty
    }

    /// The link between `a` and `b` (lat, lon), `d` metres apart, antennas
    /// `ha`, `hb` above the ground, at `freq_mhz`, compared with the
    /// engine's calibrated flat-ground model.
    ///
    /// The log-distance model already holds ordinary ground, partly blocked
    /// Fresnel zones and the earth's curve. So the ground is taken relative
    /// to the straight line between the two ends' ground, and only hills that
    /// cut the line of sight between the antennas cost extra: knife-edge loss
    /// beyond the 6 dB of a grazing edge, Deygout for up to three edges. A
    /// mast above the mean ground near its end counts as that much taller
    /// (for the horizon and the engine's height gain).
    pub fn path(&self, a: (f64, f64), b: (f64, f64), d: f64, ha: f64, hb: f64, freq_mhz: f64) -> Path {
        // One sample per ~200 m: as fine as the data.
        let n = ((d / 200.0).ceil() as usize).clamp(8, 600);
        let ground: Vec<f64> = (0..=n)
            .map(|i| {
                let f = i as f64 / n as f64;
                self.at(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
            })
            .collect();
        let (g0, g1) = (ground[0], ground[n]);
        let rel: Vec<f64> = ground.iter().enumerate().map(|(i, g)| g - (g0 + (g1 - g0) * i as f64 / n as f64)).collect();
        let lambda = 299.792_458 / freq_mhz;
        let excess = deygout(&rel, 0, n, ha, hb, d / n as f64, lambda, 1);
        // Height above the ground near each end (up to 15 km or half way),
        // as Longley-Rice does: what the antenna looks out over.
        let near = ((15_000.0 / (d / n as f64)) as usize).clamp(1, n / 2);
        let mean = |r: &[f64]| r.iter().sum::<f64>() / r.len() as f64;
        let (ma, mb) = (mean(&ground[1..=near]), mean(&ground[n - near..n]));
        Path { excess_db: excess, ha_eff: ha + (g0 - ma).max(0.0), hb_eff: hb + (g1 - mb).max(0.0) }
    }
}

pub struct Path {
    /// Diffraction loss over the hills beyond what flat ground costs.
    pub excess_db: f64,
    /// Antenna heights above the mean ground along the path (at least the
    /// height above their own ground).
    pub ha_eff: f64,
    pub hb_eff: f64,
}

/// Knife-edge loss J(v) (ITU-R P.526-15 eq. 31).
fn j(v: f64) -> f64 {
    if v <= -0.78 { 0.0 } else { 6.9 + 20.0 * (((v - 0.1).powi(2) + 1.0).sqrt() + v - 0.1).log10() }
}

/// Deygout over samples `i0..i1` of `g` (spacing `step` m) between antenna
/// tops `h0`, `h1`; only edges above the line of sight count, each with the
/// knife-edge loss beyond a grazing edge's 6 dB. `depth` levels of side edges.
#[allow(clippy::too_many_arguments)]
fn deygout(g: &[f64], i0: usize, i1: usize, h0: f64, h1: f64, step: f64, lambda: f64, depth: u8) -> f64 {
    if i1 <= i0 + 1 {
        return 0.0;
    }
    let d = (i1 - i0) as f64 * step;
    let (mut best_v, mut best_i) = (0.0, 0);
    for i in i0 + 1..i1 {
        let d1 = (i - i0) as f64 * step;
        let los = h0 + (h1 - h0) * d1 / d;
        let v = (g[i] - los) * (2.0 * d / (lambda * d1 * (d - d1))).sqrt();
        if v > best_v {
            best_v = v;
            best_i = i;
        }
    }
    if best_i == 0 {
        return 0.0;
    }
    let main = j(best_v) - j(0.0);
    if depth == 0 {
        return main;
    }
    main + deygout(g, i0, best_i, h0, g[best_i], step, lambda, depth - 1) + deygout(g, best_i, i1, g[best_i], h1, step, lambda, depth - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_with_hill(hill: u8) -> Grid {
        // 0.01 degree cells, 100 x 10; a hill row at x = 50.
        let (w, h) = (100, 10);
        let mut z = vec![10u8; w * h];
        for y in 0..h {
            z[y * w + 13] = hill;
            z[y * w + 14] = hill;
        }
        Grid { lat0: 56.0, lon0: 9.0, dlat: 0.01, dlon: 0.01, w, h, z }
    }

    #[test]
    fn flat_ground_costs_nothing_and_a_hill_costs() {
        // 12.5 km east-west; the hill row is 3 km from the west end.
        let (a, b) = ((56.05, 9.10), (56.05, 9.30));
        let d = 0.2 * 111_320.0 * 56f64.to_radians().cos();
        let flat = flat_with_hill(10).path(a, b, d, 3.0, 3.0, 869.5);
        assert!(flat.excess_db < 0.5 && flat.ha_eff == 3.0, "flat {}", flat.excess_db);
        let hill = flat_with_hill(80).path(a, b, d, 3.0, 3.0, 869.5);
        assert!(hill.excess_db > 20.0, "hill {}", hill.excess_db);
        // Antennas well above the hill clear it.
        let high = flat_with_hill(80).path(a, b, d, 150.0, 150.0, 869.5);
        assert!(high.excess_db < 1.0, "high {}", high.excess_db);
    }
}
