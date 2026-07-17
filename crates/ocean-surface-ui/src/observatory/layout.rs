use std::collections::BTreeMap;

use super::domain::ObservatoryState;

pub const TILE_W: f64 = 64.0;
pub const TILE_H: f64 = 32.0;
pub const SHELF_DEPTH: f64 = 22.0;

#[derive(Debug, Clone, PartialEq)]
pub struct StationLayout {
    pub execution_id: String,
    pub root_execution_id: String,
    pub grid_x: i32,
    pub grid_y: i32,
    pub screen_x: f64,
    pub screen_y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShelfLayout {
    pub root_execution_id: String,
    pub grid_x: i32,
    pub grid_y: i32,
    pub tiles_w: i32,
    pub tiles_h: i32,
    pub screen_x: f64,
    pub screen_y: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FloorLayout {
    pub shelves: Vec<ShelfLayout>,
    pub stations: Vec<StationLayout>,
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

pub fn build_layout(state: &ObservatoryState) -> FloorLayout {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in state.nodes.values() {
        grouped
            .entry(node.root_execution_id.clone())
            .or_default()
            .push(node.execution_id.clone());
    }

    let mut roots: Vec<_> = grouped.into_iter().collect();
    roots.sort_by(|(left, _), (right, _)| {
        stable_hash(left)
            .cmp(&stable_hash(right))
            .then_with(|| left.cmp(right))
    });
    for (_, ids) in &mut roots {
        ids.sort_by(|left, right| {
            stable_hash(left)
                .cmp(&stable_hash(right))
                .then_with(|| left.cmp(right))
        });
    }

    if roots.is_empty() {
        return FloorLayout {
            shelves: Vec::new(),
            stations: Vec::new(),
            width: 920.0,
            height: 540.0,
            min_x: -460.0,
            min_y: -270.0,
        };
    }

    let root_columns = match roots.len() {
        0 | 1 => 1,
        2 => 2,
        3..=6 => 3,
        _ => 4,
    };
    let mut shelves = Vec::with_capacity(roots.len());
    let mut stations = Vec::new();

    for (shelf_index, (root_id, ids)) in roots.into_iter().enumerate() {
        let shelf_column = (shelf_index % root_columns) as i32;
        let shelf_row = (shelf_index / root_columns) as i32;

        // Moving gx and gy in opposite directions lays shelves out horizontally;
        // moving both together creates the next screen row. Positions remain
        // deterministic and integer-tile aligned across reconnects and replay.
        let base_x = shelf_column * 8 + shelf_row * 9;
        let base_y = -shelf_column * 8 + shelf_row * 9;
        let local_columns = ids.len().clamp(1, 3);
        let local_rows = ids.len().div_ceil(local_columns);
        let tiles_w = (((local_columns - 1) * 3 + 5) as i32).max(7);
        let tiles_h = (((local_rows - 1) * 3 + 5) as i32).max(6);
        let (screen_x, screen_y) = grid_to_screen(base_x, base_y);

        shelves.push(ShelfLayout {
            root_execution_id: root_id.clone(),
            grid_x: base_x,
            grid_y: base_y,
            tiles_w,
            tiles_h,
            screen_x,
            screen_y,
        });

        let span_x = (local_columns.saturating_sub(1) * 3) as i32;
        let span_y = (local_rows.saturating_sub(1) * 3) as i32;
        let start_x = (tiles_w - 1 - span_x) / 2;
        let start_y = (tiles_h - 1 - span_y) / 2;
        for (station_index, execution_id) in ids.into_iter().enumerate() {
            let local_column = (station_index % local_columns) as i32;
            let local_row = (station_index / local_columns) as i32;
            let grid_x = base_x + start_x + local_column * 3;
            let grid_y = base_y + start_y + local_row * 3;
            let (screen_x, screen_y) = grid_to_screen(grid_x, grid_y);
            stations.push(StationLayout {
                execution_id,
                root_execution_id: root_id.clone(),
                grid_x,
                grid_y,
                screen_x,
                screen_y,
            });
        }
    }

    stations.sort_by(|left, right| {
        (left.grid_x + left.grid_y)
            .cmp(&(right.grid_x + right.grid_y))
            .then_with(|| left.execution_id.cmp(&right.execution_id))
    });

    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for shelf in &shelves {
        let (left, right, top, bottom) = shelf_bounds(shelf);
        min_x = min_x.min(left);
        max_x = max_x.max(right);
        min_y = min_y.min(top - 34.0);
        max_y = max_y.max(bottom + SHELF_DEPTH + 54.0);
    }
    for station in &stations {
        min_x = min_x.min(station.screen_x - 72.0);
        max_x = max_x.max(station.screen_x + 72.0);
        min_y = min_y.min(station.screen_y - 108.0);
        max_y = max_y.max(station.screen_y + 72.0);
    }

    min_x -= 72.0;
    max_x += 72.0;
    min_y -= 58.0;
    max_y += 64.0;

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
        shelves,
        stations,
        width,
        height,
        min_x,
        min_y,
    }
}

pub fn shelf_bounds(shelf: &ShelfLayout) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (gx, gy) in [
        (shelf.grid_x, shelf.grid_y),
        (shelf.grid_x + shelf.tiles_w - 1, shelf.grid_y),
        (
            shelf.grid_x + shelf.tiles_w - 1,
            shelf.grid_y + shelf.tiles_h - 1,
        ),
        (shelf.grid_x, shelf.grid_y + shelf.tiles_h - 1),
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
    use super::*;
    use crate::observatory::domain::{ExecutionPhase, ExecutionState, TruthProvenance};

    fn node(id: &str, root: &str) -> ExecutionState {
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
            last_cursor: 1,
            duration_millis: None,
        }
    }

    #[test]
    fn placement_is_stable_across_input_insertion_order() {
        let mut left = ObservatoryState::default();
        left.nodes.insert("b".into(), node("b", "root"));
        left.nodes.insert("a".into(), node("a", "root"));
        let mut right = ObservatoryState::default();
        right.nodes.insert("a".into(), node("a", "root"));
        right.nodes.insert("b".into(), node("b", "root"));
        assert_eq!(build_layout(&left), build_layout(&right));
    }

    #[test]
    fn every_station_is_inside_its_own_shelf() {
        let mut state = ObservatoryState::default();
        for index in 0..8 {
            let id = format!("node-{index}");
            state.nodes.insert(id.clone(), node(&id, "root"));
        }
        let layout = build_layout(&state);
        let shelf = &layout.shelves[0];
        for station in &layout.stations {
            assert!(station.grid_x >= shelf.grid_x);
            assert!(station.grid_x < shelf.grid_x + shelf.tiles_w);
            assert!(station.grid_y >= shelf.grid_y);
            assert!(station.grid_y < shelf.grid_y + shelf.tiles_h);
        }
    }
}
