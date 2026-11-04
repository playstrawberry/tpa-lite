use std::collections::BTreeMap;

pub const MAX_SEARCH_READS: usize = 256;

/// What the safety check needs to know about one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub name: String,
    pub support: bool,
    pub air: bool,
}

/// A literal player name. Bedrock names through a proxy start with one dot.
pub fn valid_name(name: &str) -> bool {
    let body = name.strip_prefix('.').unwrap_or(name);
    !body.is_empty()
        && body.len() <= 16
        && body.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
/// Whether a typed name can be the start of this player's name.
pub fn prefix_match(query: &str, name: &str) -> bool {
    !query.is_empty()
        && name
            .as_bytes()
            .get(..query.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(query.as_bytes()))
}
pub fn landing((x, y, z): (f64, f64, f64)) -> Option<(i32, i32, i32)> {
    if !x.is_finite()
        || !y.is_finite()
        || !z.is_finite()
        || x.abs() >= 29_999_984.0
        || z.abs() >= 29_999_984.0
        || !(-2048.0..=2048.0).contains(&y)
    {
        return None;
    }
    Some((x.floor() as i32, y.floor() as i32, z.floor() as i32))
}
pub fn safe_column(floor: &str, full: bool, feet_air: bool, head_air: bool) -> bool {
    let name = floor.strip_prefix("minecraft:").unwrap_or(floor);
    full && feet_air
        && head_air
        && !matches!(
            name,
            "magma_block"
                | "cactus"
                | "campfire"
                | "soul_campfire"
                | "powder_snow"
                | "lava"
                | "fire"
        )
}
/// Whether a player can stand with his feet in `feet`, on `floor`.
pub fn safe_cell(floor: &Cell, feet: &Cell, head: &Cell, water_ok: bool) -> bool {
    let clear = |cell: &Cell| {
        cell.air
            || (water_ok && cell.name.strip_prefix("minecraft:").unwrap_or(&cell.name) == "water")
    };
    safe_column(&floor.name, floor.support, clear(feet), clear(head))
}
/// The nearest safe standing spot around `start`, nearest first and always in the same order, or
/// `None`. `read` gives a block, or `None` in a chunk that is not loaded. At most
/// [`MAX_SEARCH_READS`] blocks are read.
pub fn search(
    start: (i32, i32, i32),
    radius: i32,
    water_ok: bool,
    mut read: impl FnMut((i32, i32, i32)) -> Option<Cell>,
) -> Option<(i32, i32, i32)> {
    let radius = radius.clamp(0, 4);
    let mut offsets = Vec::new();
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                offsets.push((dx * dx + dy * dy + dz * dz, dx, dy, dz));
            }
        }
    }
    offsets.sort_unstable();
    let mut cells: BTreeMap<(i32, i32, i32), Option<Cell>> = BTreeMap::new();
    for (_, dx, dy, dz) in offsets {
        let spot = (start.0 + dx, start.1 + dy, start.2 + dz);
        let [feet, head, floor] = [spot.1, spot.1 + 1, spot.1 - 1].map(|y| (spot.0, y, spot.2));
        // The feet first: most blocks fail there, on one read.
        let mut readable = true;
        for at in [feet, head, floor] {
            if !cells.contains_key(&at) {
                if cells.len() >= MAX_SEARCH_READS {
                    return None;
                }
                cells.insert(at, read(at));
            }
            let open = match cells.get(&at) {
                Some(Some(cell)) => at == floor || cell.air || water_ok,
                _ => false,
            };
            if !open {
                readable = false;
                break;
            }
        }
        if readable
            && let (Some(Some(feet)), Some(Some(head)), Some(Some(floor))) =
                (cells.get(&feet), cells.get(&head), cells.get(&floor))
            && safe_cell(floor, feet, head, water_ok)
        {
            return Some(spot);
        }
    }
    None
}
pub fn moved(from: (f64, f64, f64), to: (f64, f64, f64)) -> bool {
    [from.0, from.1, from.2, to.0, to.1, to.2]
        .iter()
        .any(|n| !n.is_finite())
        || (from.0 - to.0).abs() > 0.000_001
        || (from.1 - to.1).abs() > 0.000_001
        || (from.2 - to.2).abs() > 0.000_001
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn air() -> Cell {
        Cell {
            name: "minecraft:air".into(),
            support: false,
            air: true,
        }
    }
    fn solid(name: &str) -> Cell {
        Cell {
            name: name.into(),
            support: true,
            air: false,
        }
    }
    fn water() -> Cell {
        Cell {
            name: "minecraft:water".into(),
            support: false,
            air: false,
        }
    }
    /// A stone floor under y = 64, air above it, except what `change` says.
    fn world(
        change: impl Fn((i32, i32, i32)) -> Option<Cell>,
    ) -> impl FnMut((i32, i32, i32)) -> Option<Cell> {
        move |p| {
            Some(change(p).unwrap_or_else(|| {
                if p.1 < 64 {
                    solid("minecraft:stone")
                } else {
                    air()
                }
            }))
        }
    }

    #[test]
    fn names_are_literal_bounded_minecraft_names_not_selectors() {
        for s in [
            "Alex",
            "a_1",
            "1234567890123456",
            ".Bedrock_Name",
            ".1234567890123456",
        ] {
            assert!(valid_name(s), "{s}");
        }
        for s in [
            "",
            ".",
            "..a",
            "a.b",
            "a.",
            "@a",
            "@p",
            "a b",
            "a\n",
            "a\"x",
            "12345678901234567",
            ".12345678901234567",
            "\u{0130}",
        ] {
            assert!(!valid_name(s), "{s}");
        }
    }
    #[test]
    fn partial_names_match_from_the_start_in_any_case() {
        assert!(prefix_match("ale", "Alex"));
        assert!(prefix_match("ALEX", "Alex"));
        assert!(prefix_match(".bed", ".Bedrock"));
        assert!(!prefix_match("lex", "Alex"));
        assert!(!prefix_match("", "Alex"));
        assert!(!prefix_match("Alexander", "Alex"));
    }
    #[test]
    fn landing_is_finite_bounded_center_of_a_block_column() {
        assert_eq!(landing((-0.1, 64.9, 2.2)), Some((-1, 64, 2)));
        for p in [
            (f64::NAN, 64., 0.),
            (0., f64::INFINITY, 0.),
            (3e7, 64., 0.),
            (0., -2049., 0.),
        ] {
            assert_eq!(landing(p), None);
        }
    }
    #[test]
    fn safety_requires_full_support_and_two_air_blocks() {
        assert!(safe_column("minecraft:stone", true, true, true));
        assert!(!safe_column("minecraft:stone", false, true, true));
        assert!(!safe_column("minecraft:stone", true, false, true));
        assert!(!safe_column("minecraft:stone", true, true, false));
        for s in [
            "magma_block",
            "cactus",
            "campfire",
            "soul_campfire",
            "powder_snow",
            "lava",
            "fire",
        ] {
            assert!(!safe_column(s, true, true, true), "{s}");
            assert!(
                !safe_column(&format!("minecraft:{s}"), true, true, true),
                "{s}"
            );
        }
    }
    #[test]
    fn water_is_only_safe_when_the_setting_says_so() {
        let stone = solid("minecraft:stone");
        assert!(safe_cell(&stone, &air(), &air(), false));
        assert!(!safe_cell(&stone, &water(), &air(), false));
        assert!(!safe_cell(&stone, &air(), &water(), false));
        assert!(safe_cell(&stone, &water(), &water(), true));
        assert!(safe_cell(&stone, &water(), &air(), true));
        assert!(!safe_cell(&water(), &air(), &air(), true));
        let lava = Cell {
            name: "minecraft:lava".into(),
            support: false,
            air: false,
        };
        assert!(!safe_cell(&stone, &lava, &air(), true));
    }
    #[test]
    fn a_safe_start_is_kept_and_reads_only_its_three_blocks() {
        let mut reads = 0;
        let mut inner = world(|_| None);
        let found = search((0, 64, 0), 3, false, |p| {
            reads += 1;
            inner(p)
        });
        assert_eq!(found, Some((0, 64, 0)));
        assert_eq!(reads, 3);
    }
    #[test]
    fn radius_zero_never_looks_beyond_the_start() {
        let hazard = world(|p| (p == (0, 63, 0)).then(|| solid("minecraft:magma_block")));
        assert_eq!(search((0, 64, 0), 0, false, hazard), None);
    }
    #[test]
    fn the_nearest_safe_spot_wins_and_the_order_is_stable() {
        let hazard = || world(|p| (p == (0, 63, 0)).then(|| solid("minecraft:magma_block")));
        let first = search((0, 64, 0), 3, false, hazard()).unwrap();
        assert_eq!(first, search((0, 64, 0), 3, false, hazard()).unwrap());
        assert_eq!(first.1, 64);
        assert_eq!(first.0.abs() + first.2.abs(), 1);
    }
    #[test]
    fn a_player_in_the_air_lands_on_the_floor_below_within_the_radius() {
        assert_eq!(
            search((0, 66, 0), 3, false, world(|_| None)),
            Some((0, 64, 0))
        );
        assert_eq!(search((0, 70, 0), 3, false, world(|_| None)), None);
    }
    #[test]
    fn an_unloaded_chunk_is_skipped_and_never_read_as_safe() {
        let mut read = world(|_| None);
        let found = search((15, 64, 0), 2, false, |p| {
            // The chunk east of x = 15 and the start's own floor are not loaded.
            if p.0 >= 16 || p == (15, 63, 0) {
                None
            } else {
                read(p)
            }
        })
        .unwrap();
        assert!(found.0 < 16 && found != (15, 64, 0));
    }
    #[test]
    fn nothing_safe_is_none_and_the_reads_stay_bounded() {
        let mut seen = BTreeSet::new();
        let mut reads = 0;
        let found = search((0, 64, 0), 4, false, |p| {
            reads += 1;
            seen.insert(p);
            Some(solid("minecraft:stone"))
        });
        assert_eq!(found, None);
        assert!(reads <= MAX_SEARCH_READS, "{reads}");
        assert_eq!(reads, seen.len(), "a block is read once");
    }
    #[test]
    fn movement_includes_vertical_and_away_then_back_but_not_look() {
        assert!(!moved((0., 64., 0.), (0., 64., 0.)));
        assert!(moved((0., 64., 0.), (0., 64.01, 0.)));
        assert!(moved((0., 64., 0.), (0.01, 64., 0.)));
        assert!(moved((0., 64., 0.), (f64::NAN, 64., 0.)));
    }
}
