//! Loaded-only landing: no chunk generation, and a short bounded search at most.
use std::collections::BTreeMap;

use strawberry_plugin_api::{Player, common::BlockPos};

use crate::{
    checks::{self, Cell},
    config::Config,
    model::Location,
};

pub struct Policy {
    pub check: bool,
    pub water: bool,
    pub radius: i32,
    pub center: bool,
}

pub fn policy(config: &Config, mover: &Player) -> Policy {
    let mode = format!("{:?}", mover.get_gamemode()).to_ascii_lowercase();
    Policy {
        check: config.safe_teleport && !config.safe_skip_modes.contains(&mode),
        water: config.water_safe,
        radius: config.safe_search_radius,
        center: config.center_on_block,
    }
}

pub fn here(p: &Player) -> Location {
    Location {
        world: p.get_world().get_name(),
        pos: p.get_position(),
    }
}

pub fn destination(anchor: &Player, policy: &Policy) -> Option<Location> {
    let world = anchor.get_world();
    let exact = anchor.get_position();
    let start = checks::landing(exact)?;
    let spot = if policy.check {
        let mut chunks = BTreeMap::new();
        checks::search(start, policy.radius, policy.water, |(x, y, z)| {
            let key = (x.div_euclid(16), z.div_euclid(16));
            // get_chunk returns only an already-loaded chunk; a missing chunk is never read.
            let loaded = *chunks
                .entry(key)
                .or_insert_with(|| world.get_chunk(key.0, key.1).is_some());
            if !loaded {
                return None;
            }
            let state = world.get_block_state(BlockPos { x, y, z });
            Some(Cell {
                support: state.is_full_cube && state.up_side_solid,
                air: state.is_air,
                name: state.block_name,
            })
        })?
    } else {
        let _chunk = world.get_chunk(start.0.div_euclid(16), start.2.div_euclid(16))?;
        start
    };
    let position = if spot == start && !policy.center {
        exact
    } else {
        (
            f64::from(spot.0) + 0.5,
            f64::from(spot.1),
            f64::from(spot.2) + 0.5,
        )
    };
    let border = world.get_world_border();
    for (dx, dz) in [(-0.3, -0.3), (-0.3, 0.3), (0.3, -0.3), (0.3, 0.3)] {
        if !border.contains(position.0 + dx, position.2 + dz) {
            return None;
        }
    }
    Some(Location {
        world: world.get_name(),
        pos: position,
    })
}

/// Off whatever the player rides, and everyone off him: nobody rides across a teleport.
pub fn dismount(p: &Player) {
    let entity = p.as_entity();
    if entity.get_vehicle().is_some() {
        entity.set_vehicle(None);
    }
    if !entity.get_passengers().is_empty() {
        entity.eject_passengers();
    }
}

pub fn go(p: &Player, anchor: &Player, dest: &Location) -> bool {
    let yaw = anchor.get_yaw();
    let pitch = anchor.get_pitch();
    if !yaw.is_finite() || !pitch.is_finite() {
        return false;
    }
    // The normal teleport path: a cancelled call is not retried and never reported as success.
    p.teleport(dest.pos, Some(yaw), Some(pitch), anchor.get_world());
    let current = here(p);
    current.world == dest.world && !checks::moved(current.pos, dest.pos)
}
