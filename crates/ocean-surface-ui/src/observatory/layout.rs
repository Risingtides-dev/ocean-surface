use std::collections::BTreeSet;

use super::domain::ObservatoryState;

pub const TILE_W: f64 = 64.0;
pub const TILE_H: f64 = 32.0;
pub const CUBICLE_DEPTH: f64 = 22.0;
pub const CUBICLE_TILES_W: i32 = 5;
pub const CUBICLE_TILES_H: i32 = 5;
pub const CUBICLE_COLUMNS: usize = 3;
const CUBICLE_STRIDE: i32 = 6;

/// One immutable slot in the reactive floor plan. A cubicle owns a constant
/// footprint; adding another execution appends one more module and never
/// resizes or repacks modules that are already present.
#[derive(Debug, Clone, PartialEq)]
pub struct CubicleLayout {
    pub execution_id: String,
    pub root_execution_id: String,
    pub slot: usize,
    pub grid_x: i32,
    pub grid_y: i32,
    pub tiles_w: i32,
    pub tiles_h: i32,
    pub accent_index: u8,
    /// A present module in the neighboring slot turns that side into an
    /// interior partition; absent neighbors get the tall building envelope.
    pub neighbor_left: bool,
    pub neighbor_up: bool,
    pub neighbor_right: bool,
    pub neighbor_down: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StationLayout {
    pub execution_id: String,
    pub root_execution_id: String,
    pub slot: usize,
    pub grid_x: i32,
    pub grid_y: i32,
    pub screen_x: f64,
    pub screen_y: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FloorLayout {
    pub cubicles: Vec<CubicleLayout>,
    pub stations: Vec<StationLayout>,
    /// Walkway tiles connecting present modules into one facility. Each tile
    /// is owned by exactly one present cubicle, so evicted slots leave honest
    /// holes instead of phantom corridors.
    pub corridor: Vec<(i32, i32)>,
    pub width: f64,
    pub height: f64,
    pub min_x: f64,
    pub min_y: f64,
}

pub fn grid_to_screen(grid_x: i32, grid_y: i32) -> (f64, f64) {
    (
        f64::from(grid_x - grid_y) * (TILE_W / 2.0),
        f64::from(grid_x + grid_y) * (TILE_H / 2.0),
    )
}

/// Derive the whole facility from reducer state. The reducer owns stable slot
/// assignment, so layout is a pure projection: a live admission receives the
/// next slot while every existing module remains fixed.
pub fn build_layout(state: &ObservatoryState) -> FloorLayout {
    let mut nodes: Vec<_> = state.nodes.values().collect();
    nodes.sort_by(|left, right| {
        left.floor_slot
            .cmp(&right.floor_slot)
            .then_with(|| left.execution_id.cmp(&right.execution_id))
    });

    if nodes.is_empty() {
        return FloorLayout {
            cubicles: Vec::new(),
            stations: Vec::new(),
            corridor: Vec::new(),
            width: 920.0,
            height: 540.0,
            min_x: -460.0,
            min_y: -270.0,
        };
    }

    let present: BTreeSet<usize> = nodes.iter().map(|node| node.floor_slot).collect();
    let mut cubicles = Vec::with_capacity(nodes.len());
    let mut stations = Vec::with_capacity(nodes.len());
    let mut corridor = Vec::with_capacity(nodes.len() * 24);
    for node in nodes {
        let slot = node.floor_slot;
        let column = (slot % CUBICLE_COLUMNS) as i32;
        let row = (slot / CUBICLE_COLUMNS) as i32;

        // Grid-adjacent packing: modules sit shoulder to shoulder with one
        // corridor tile between footprints, so the floor reads as one
        // connected facility (a classic isometric diamond) instead of
        // detached islands.
        let base_x = column * CUBICLE_STRIDE;
        let base_y = row * CUBICLE_STRIDE;
        let station_grid_x = base_x + CUBICLE_TILES_W / 2;
        let station_grid_y = base_y + CUBICLE_TILES_H / 2;
        let (screen_x, screen_y) = grid_to_screen(station_grid_x, station_grid_y);

        let neighbor_left = column > 0 && present.contains(&(slot - 1));
        let neighbor_up = slot >= CUBICLE_COLUMNS && present.contains(&(slot - CUBICLE_COLUMNS));
        let neighbor_right = column + 1 < CUBICLE_COLUMNS as i32 && present.contains(&(slot + 1));
        let neighbor_down = present.contains(&(slot + CUBICLE_COLUMNS));

        // Corridor ownership: every present module paves the band east and
        // south of its footprint (shared with the next module), and boundary
        // modules pave the outer ring on their west/north sides.
        for k in -1..=CUBICLE_TILES_H {
            corridor.push((base_x + CUBICLE_TILES_W, base_y + k));
        }
        for k in -1..CUBICLE_TILES_W {
            corridor.push((base_x + k, base_y + CUBICLE_TILES_H));
        }
        if column == 0 {
            for k in -1..CUBICLE_TILES_H {
                corridor.push((base_x - 1, base_y + k));
            }
        }
        if row == 0 {
            for k in 0..CUBICLE_TILES_W {
                corridor.push((base_x + k, base_y - 1));
            }
        }

        cubicles.push(CubicleLayout {
            execution_id: node.execution_id.clone(),
            root_execution_id: node.root_execution_id.clone(),
            slot,
            grid_x: base_x,
            grid_y: base_y,
            tiles_w: CUBICLE_TILES_W,
            tiles_h: CUBICLE_TILES_H,
            accent_index: (stable_hash(&node.root_execution_id) % 3) as u8,
            neighbor_left,
            neighbor_up,
            neighbor_right,
            neighbor_down,
        });
        stations.push(StationLayout {
            execution_id: node.execution_id.clone(),
            root_execution_id: node.root_execution_id.clone(),
            slot,
            grid_x: station_grid_x,
            grid_y: station_grid_y,
            screen_x,
            screen_y,
        });
    }
    corridor.sort_unstable();
    corridor.dedup();

    cubicles.sort_by(|left, right| {
        (left.grid_x + left.grid_y)
            .cmp(&(right.grid_x + right.grid_y))
            .then_with(|| left.slot.cmp(&right.slot))
    });
    stations.sort_by(|left, right| {
        (left.grid_x + left.grid_y)
            .cmp(&(right.grid_x + right.grid_y))
            .then_with(|| left.slot.cmp(&right.slot))
    });

    // Keep slot zero (plus its corridor ring) in the world bounds after
    // reducer-cap eviction. Vacated early slots become honest gaps instead of
    // shifting every retained module.
    let (origin_left, origin_right, origin_top, origin_bottom) =
        footprint_bounds(-1, -1, CUBICLE_TILES_W + 2, CUBICLE_TILES_H + 2);
    let mut min_x = origin_left;
    let mut max_x = origin_right;
    let mut min_y = origin_top - 62.0;
    let mut max_y = origin_bottom + CUBICLE_DEPTH + 54.0;
    for cubicle in &cubicles {
        let (left, right, top, bottom) = footprint_bounds(
            cubicle.grid_x - 1,
            cubicle.grid_y - 1,
            cubicle.tiles_w + 2,
            cubicle.tiles_h + 2,
        );
        min_x = min_x.min(left);
        max_x = max_x.max(right);
        min_y = min_y.min(top - 62.0);
        max_y = max_y.max(bottom + CUBICLE_DEPTH + 54.0);
    }
    for station in &stations {
        min_x = min_x.min(station.screen_x - 72.0);
        max_x = max_x.max(station.screen_x + 72.0);
        min_y = min_y.min(station.screen_y - 108.0);
        max_y = max_y.max(station.screen_y + 76.0);
    }

    min_x -= 72.0;
    max_x += 72.0;
    min_y -= 54.0;
    max_y += 68.0;

    let mut width = max_x - min_x;
    let mut height = max_y - min_y;
    if width < 920.0 {
        min_x -= (920.0 - width) / 2.0;
        width = 920.0;
    }
    if height < 540.0 {
        min_y -= (540.0 - height) / 2.0;
        height = 540.0;
    }

    FloorLayout {
        cubicles,
        stations,
        corridor,
        width,
        height,
        min_x,
        min_y,
    }
}

fn footprint_bounds(grid_x: i32, grid_y: i32, tiles_w: i32, tiles_h: i32) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (gx, gy) in [
        (grid_x, grid_y),
        (grid_x + tiles_w - 1, grid_y),
        (grid_x + tiles_w - 1, grid_y + tiles_h - 1),
        (grid_x, grid_y + tiles_h - 1),
    ] {
        let (x, y) = grid_to_screen(gx, gy);
        min_x = min_x.min(x - TILE_W / 2.0);
        max_x = max_x.max(x + TILE_W / 2.0);
        min_y = min_y.min(y - TILE_H / 2.0);
        max_y = max_y.max(y + TILE_H / 2.0);
    }
    (min_x, max_x, min_y, max_y)
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::observatory::domain::{ExecutionPhase, ExecutionState, TruthProvenance};

    fn node(id: &str, root: &str, floor_slot: usize, started_at: &str) -> ExecutionState {
        ExecutionState {
            execution_id: id.into(),
            root_execution_id: root.into(),
            parent_execution_id: None,
            session_id: String::new(),
            turn_id: String::new(),
            phase: ExecutionPhase::Running,
            truth: TruthProvenance::HostObserved,
            labels: vec![],
            tools: Default::default(),
            permission_waiting: false,
            permission_reason: None,
            model_alias: None,
            floor_slot,
            started_at: started_at.into(),
            last_cursor: 1,
            duration_millis: None,
        }
    }

    fn station_positions(layout: &FloorLayout) -> BTreeMap<String, (i32, i32, usize)> {
        layout
            .stations
            .iter()
            .map(|station| {
                (
                    station.execution_id.clone(),
                    (station.grid_x, station.grid_y, station.slot),
                )
            })
            .collect()
    }

    #[test]
    fn placement_is_stable_across_input_insertion_order() {
        let mut left = ObservatoryState::default();
        left.nodes
            .insert("b".into(), node("b", "root", 1, "2026-07-17T00:00:01Z"));
        left.nodes
            .insert("a".into(), node("a", "root", 0, "2026-07-17T00:00:02Z"));
        let mut right = ObservatoryState::default();
        right
            .nodes
            .insert("a".into(), node("a", "root", 0, "2026-07-17T00:00:02Z"));
        right
            .nodes
            .insert("b".into(), node("b", "root", 1, "2026-07-17T00:00:01Z"));
        assert_eq!(build_layout(&left), build_layout(&right));
    }

    #[test]
    fn a_new_admission_appends_one_cubicle_without_moving_existing_modules() {
        let mut state = ObservatoryState::default();
        for (index, id) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
            state.nodes.insert(
                id.into(),
                node(
                    id,
                    "root",
                    index,
                    &format!("2026-07-17T00:00:0{}Z", 3 - index),
                ),
            );
        }
        let before = build_layout(&state);
        let before_positions = station_positions(&before);

        state.nodes.insert(
            "delta".into(),
            node("delta", "root", 3, "2020-01-01T00:00:00Z"),
        );
        let after = build_layout(&state);
        let after_positions = station_positions(&after);

        assert_eq!(after.cubicles.len(), before.cubicles.len() + 1);
        for (id, position) in before_positions {
            assert_eq!(after_positions.get(&id), Some(&position));
        }
        assert_eq!(after_positions.get("delta").map(|slot| slot.2), Some(3));
    }

    #[test]
    fn removing_an_early_slot_leaves_retained_modules_and_world_origin_fixed() {
        let mut state = ObservatoryState::default();
        for (slot, id) in ["alpha", "beta", "gamma", "delta"].into_iter().enumerate() {
            state
                .nodes
                .insert(id.into(), node(id, "root", slot, "2026-07-17T00:00:00Z"));
        }
        let before = build_layout(&state);
        let delta_before = station_positions(&before)["delta"];

        state.nodes.remove("alpha");
        let after = build_layout(&state);

        assert_eq!(station_positions(&after)["delta"], delta_before);
        assert_eq!(after.min_x, before.min_x);
        assert_eq!(after.min_y, before.min_y);
        assert_eq!(after.width, before.width);
        assert_eq!(after.height, before.height);
    }

    #[test]
    fn every_execution_owns_one_constant_footprint_cubicle() {
        let mut state = ObservatoryState::default();
        for index in 0..8 {
            let id = format!("node-{index}");
            state.nodes.insert(
                id.clone(),
                node(&id, "root", index, &format!("2026-07-17T00:00:{index:02}Z")),
            );
        }
        let layout = build_layout(&state);
        assert_eq!(layout.cubicles.len(), state.nodes.len());
        for cubicle in &layout.cubicles {
            assert_eq!(cubicle.tiles_w, CUBICLE_TILES_W);
            assert_eq!(cubicle.tiles_h, CUBICLE_TILES_H);
            let station = layout
                .stations
                .iter()
                .find(|station| station.execution_id == cubicle.execution_id)
                .expect("cubicle station");
            assert!(station.grid_x >= cubicle.grid_x);
            assert!(station.grid_x < cubicle.grid_x + cubicle.tiles_w);
            assert!(station.grid_y >= cubicle.grid_y);
            assert!(station.grid_y < cubicle.grid_y + cubicle.tiles_h);
        }
    }
}
