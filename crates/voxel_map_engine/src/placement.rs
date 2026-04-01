use bevy::prelude::*;

use crate::types::CHUNK_SIZE;

/// 2D Poisson disk sampling within a chunk's XZ footprint using Bridson's algorithm.
///
/// Returns positions in chunk-local XZ space `[0, CHUNK_SIZE)`.
/// Deterministic: same seed + chunk_pos + parameters → same result.
pub fn poisson_disk_sample(
    seed: u64,
    chunk_pos: IVec3,
    min_spacing: f64,
    max_candidates: usize,
) -> Vec<Vec2> {
    if max_candidates == 0 || min_spacing <= 0.0 {
        return Vec::new();
    }

    let extent = CHUNK_SIZE as f64;
    let cell_size = min_spacing / std::f64::consts::SQRT_2;
    let grid_size = (extent / cell_size).ceil() as usize;
    let mut grid: Vec<Option<usize>> = vec![None; grid_size * grid_size];
    let mut points: Vec<Vec2> = Vec::new();
    let mut active: Vec<usize> = Vec::new();

    let mut rng = simple_rng(placement_seed(seed, chunk_pos, 0));

    // First point
    let first = Vec2::new(
        rng_f64(&mut rng) as f32 * extent as f32,
        rng_f64(&mut rng) as f32 * extent as f32,
    );
    insert_point(
        &mut grid,
        &mut points,
        &mut active,
        first,
        cell_size,
        grid_size,
    );

    let k = 30; // candidates per active point (Bridson standard)
    while !active.is_empty() && points.len() < max_candidates {
        let active_idx = (rng_f64(&mut rng) * active.len() as f64) as usize % active.len();
        let center = points[active[active_idx]];
        let mut found = false;

        for _ in 0..k {
            let angle = rng_f64(&mut rng) * std::f64::consts::TAU;
            let radius = min_spacing + rng_f64(&mut rng) * min_spacing;
            let candidate = Vec2::new(
                center.x + (angle.cos() * radius) as f32,
                center.y + (angle.sin() * radius) as f32,
            );

            if candidate.x < 0.0
                || candidate.x >= extent as f32
                || candidate.y < 0.0
                || candidate.y >= extent as f32
            {
                continue;
            }

            if is_valid_point(&grid, &points, candidate, cell_size, grid_size, min_spacing) {
                insert_point(
                    &mut grid,
                    &mut points,
                    &mut active,
                    candidate,
                    cell_size,
                    grid_size,
                );
                found = true;
                break;
            }
        }

        if !found {
            active.swap_remove(active_idx);
        }
    }

    points
}

fn insert_point(
    grid: &mut [Option<usize>],
    points: &mut Vec<Vec2>,
    active: &mut Vec<usize>,
    point: Vec2,
    cell_size: f64,
    grid_size: usize,
) {
    let idx = points.len();
    let gx = (point.x as f64 / cell_size) as usize;
    let gz = (point.y as f64 / cell_size) as usize;
    if gx < grid_size && gz < grid_size {
        grid[gx * grid_size + gz] = Some(idx);
    }
    points.push(point);
    active.push(idx);
}

fn is_valid_point(
    grid: &[Option<usize>],
    points: &[Vec2],
    candidate: Vec2,
    cell_size: f64,
    grid_size: usize,
    min_spacing: f64,
) -> bool {
    let gx = (candidate.x as f64 / cell_size) as i32;
    let gz = (candidate.y as f64 / cell_size) as i32;
    let min_dist_sq = (min_spacing * min_spacing) as f32;

    for dx in -2..=2 {
        for dz in -2..=2 {
            let nx = gx + dx;
            let nz = gz + dz;
            if nx < 0 || nz < 0 || nx >= grid_size as i32 || nz >= grid_size as i32 {
                continue;
            }
            if let Some(idx) = grid[nx as usize * grid_size + nz as usize] {
                if points[idx].distance_squared(candidate) < min_dist_sq {
                    return false;
                }
            }
        }
    }
    true
}

/// Derive a deterministic per-chunk, per-rule seed.
pub fn placement_seed(map_seed: u64, chunk_pos: IVec3, rule_index: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    map_seed.hash(&mut hasher);
    chunk_pos.hash(&mut hasher);
    rule_index.hash(&mut hasher);
    hasher.finish()
}

/// Simple xorshift64 RNG for deterministic sampling without external deps.
fn simple_rng(seed: u64) -> u64 {
    if seed == 0 { 1 } else { seed }
}

/// Next value from xorshift64, returns value in [0, 1).
fn rng_f64(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state as f64) / (u64::MAX as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisson_disk_produces_points_within_bounds() {
        let points = poisson_disk_sample(42, IVec3::ZERO, 3.0, 50);
        assert!(!points.is_empty(), "should produce at least one point");
        for p in &points {
            assert!(
                p.x >= 0.0 && p.x < CHUNK_SIZE as f32,
                "x out of bounds: {}",
                p.x
            );
            assert!(
                p.y >= 0.0 && p.y < CHUNK_SIZE as f32,
                "z out of bounds: {}",
                p.y
            );
        }
    }

    #[test]
    fn poisson_disk_respects_min_spacing() {
        let min_spacing = 4.0;
        let points = poisson_disk_sample(123, IVec3::new(5, 0, 3), min_spacing, 100);
        let min_sq = (min_spacing * min_spacing) as f32 - 0.001; // tiny epsilon
        for i in 0..points.len() {
            for j in (i + 1)..points.len() {
                let dist_sq = points[i].distance_squared(points[j]);
                assert!(
                    dist_sq >= min_sq,
                    "points {} and {} too close: dist={}, min={}",
                    i,
                    j,
                    dist_sq.sqrt(),
                    min_spacing
                );
            }
        }
    }

    #[test]
    fn poisson_disk_deterministic() {
        let a = poisson_disk_sample(42, IVec3::new(1, 2, 3), 3.0, 30);
        let b = poisson_disk_sample(42, IVec3::new(1, 2, 3), 3.0, 30);
        assert_eq!(a.len(), b.len());
        for (pa, pb) in a.iter().zip(b.iter()) {
            assert_eq!(pa, pb);
        }
    }

    #[test]
    fn poisson_disk_different_chunks_differ() {
        let a = poisson_disk_sample(42, IVec3::ZERO, 3.0, 30);
        let b = poisson_disk_sample(42, IVec3::new(10, 0, 10), 3.0, 30);
        // Different chunk positions should produce different layouts
        assert_ne!(a, b);
    }

    #[test]
    fn placement_seed_unique_across_chunks() {
        let s1 = placement_seed(42, IVec3::ZERO, 0);
        let s2 = placement_seed(42, IVec3::new(1, 0, 0), 0);
        assert_ne!(s1, s2);
    }

    #[test]
    fn placement_seed_unique_across_rules() {
        let s1 = placement_seed(42, IVec3::ZERO, 0);
        let s2 = placement_seed(42, IVec3::ZERO, 1);
        assert_ne!(s1, s2);
    }

    #[test]
    fn poisson_disk_zero_candidates_returns_empty() {
        let points = poisson_disk_sample(42, IVec3::ZERO, 3.0, 0);
        assert!(points.is_empty());
    }

    #[test]
    fn poisson_disk_zero_spacing_returns_empty() {
        let points = poisson_disk_sample(42, IVec3::ZERO, 0.0, 50);
        assert!(points.is_empty());
    }
}
