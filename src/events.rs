use strawberry_plugin_api::{Context, EventHandler, Server, events::*, events_wit::*};

use crate::{
    checks,
    runtime::{self as rt, Shared},
};

/// Cancels a warmup. Registered only on a server with a delay: without one nothing can be
/// cancelled, and movement and damage cost nothing.
struct Warmup(Shared);

impl Warmup {
    fn cancel_for(&self, server: &Server, player: &strawberry_plugin_api::Player) {
        let id = rt::id(player);
        let r = { rt::lock(&self.0.state).model.moving(&id) };
        if let Some(r) = r {
            rt::cancel(&self.0, server, r, "moved");
        }
    }
    fn movement(
        &self,
        server: &Server,
        player: &strawberry_plugin_api::Player,
        from: (f64, f64, f64),
        to: (f64, f64, f64),
    ) {
        if !rt::lock(&self.0.state).model.has_warmups() {
            return;
        }
        if checks::moved(from, to) {
            self.cancel_for(server, player);
        }
    }
    fn damage(&self, server: &Server, entity: i32, damage: f32) {
        if damage <= 0.0 {
            return;
        }
        let r = { rt::lock(&self.0.state).model.damaged(entity) };
        if let Some(id) = r {
            rt::cancel(&self.0, server, id, "damaged");
        }
    }
}

impl EventHandler<PlayerMoveEvent> for Warmup {
    fn handle(&self, server: Server, event: PlayerMoveEventData) -> PlayerMoveEventData {
        if !event.cancelled {
            self.movement(
                &server,
                &event.player,
                event.from_position,
                event.to_position,
            );
        }
        event
    }
}
impl EventHandler<PlayerTeleportEvent> for Warmup {
    fn handle(&self, server: Server, event: PlayerTeleportEventData) -> PlayerTeleportEventData {
        if !event.cancelled {
            self.movement(
                &server,
                &event.player,
                event.from_position,
                event.to_position,
            );
        }
        event
    }
}
impl EventHandler<PlayerChangeWorldEvent> for Warmup {
    fn handle(
        &self,
        server: Server,
        event: PlayerChangeWorldEventData,
    ) -> PlayerChangeWorldEventData {
        if !event.cancelled && rt::lock(&self.0.state).model.has_warmups() {
            self.cancel_for(&server, &event.player);
        }
        event
    }
}
macro_rules! warmup_damage {
    ($marker:ty, $data:ty) => {
        impl EventHandler<$marker> for Warmup {
            fn handle(&self, server: Server, event: $data) -> $data {
                if !event.cancelled {
                    self.damage(&server, event.entity_id, event.damage);
                }
                event
            }
        }
    };
}
warmup_damage!(EntityDamageEvent, EntityDamageEventData);
warmup_damage!(EntityDamageByEntityEvent, EntityDamageByEntityEventData);
warmup_damage!(EntityDamageByBlockEvent, EntityDamageByBlockEventData);

/// Short protection after arriving: in memory only, never the player's saved invulnerable flag.
/// Registered only when the setting is on, with no work while nobody is protected.
struct Arrival(Shared);

macro_rules! arrival_victim {
    ($marker:ty, $data:ty) => {
        impl EventHandler<$marker> for Arrival {
            fn handle(&self, _server: Server, mut event: $data) -> $data {
                if self.0.any_protected() && self.0.is_protected(event.entity_id) {
                    event.cancelled = true;
                }
                event
            }
        }
    };
}
arrival_victim!(EntityDamageEvent, EntityDamageEventData);
arrival_victim!(EntityDamageByBlockEvent, EntityDamageByBlockEventData);

impl EventHandler<EntityDamageByEntityEvent> for Arrival {
    fn handle(
        &self,
        _server: Server,
        mut event: EntityDamageByEntityEventData,
    ) -> EntityDamageByEntityEventData {
        if self.0.any_protected()
            && (self.0.is_protected(event.entity_id)
                || self.0.is_protected(event.damager_id)
                || damage_projectile_shooter().is_some_and(|shooter| self.0.is_protected(shooter)))
        {
            event.cancelled = true;
        }
        event
    }
}

/// A player who is gone leaves no request and no protection behind.
struct Lifecycle(Shared);

impl EventHandler<PlayerLeaveEvent> for Lifecycle {
    fn handle(&self, server: Server, event: PlayerLeaveEventData) -> PlayerLeaveEventData {
        let id = rt::id(&event.player);
        self.0.unprotect(event.player.as_entity().get_id() as i32);
        let gone = { rt::lock(&self.0.state).model.leave(&id) };
        if !gone.is_empty() {
            for r in &gone {
                rt::notify(&self.0, &server, r, "unavailable");
            }
            rt::arm(&self.0, &server);
        }
        event
    }
}
impl EventHandler<PlayerDeathEvent> for Lifecycle {
    fn handle(&self, server: Server, event: PlayerDeathEventData) -> PlayerDeathEventData {
        let id = rt::id(&event.player);
        self.0.unprotect(event.player.as_entity().get_id() as i32);
        let involved = { rt::lock(&self.0.state).model.involving(&id) };
        for r in involved {
            rt::cancel(&self.0, &server, r.id, "unavailable");
        }
        event
    }
}

pub fn register(context: &Context, shared: &Shared) -> strawberry_plugin_api::Result<()> {
    let observe = HandlerOptions {
        priority: HandlerPriority::Monitor,
        blocking: true,
        ignore_cancelled: true,
        // Listeners only cancel our own teleport, never a vanilla movement or damage event.
        on_failure: FailureVerdict::Allow,
    };
    macro_rules! register {
        ($listener:expr, $options:expr, $($event:ty),+ $(,)?) => { $(
            context.register_event_handler_with_options::<$event, _>($listener(shared.clone()), $options)?;
        )+ };
    }
    register!(Lifecycle, observe, PlayerLeaveEvent, PlayerDeathEvent);
    if shared.config.delay_ms > 0 {
        register!(
            Warmup,
            observe,
            PlayerMoveEvent,
            PlayerTeleportEvent,
            PlayerChangeWorldEvent,
            EntityDamageEvent,
            EntityDamageByEntityEvent,
            EntityDamageByBlockEvent,
        );
    }
    if shared.config.arrival_protection_ms > 0 {
        // Below the highest priority, so a plugin that decides a fight has the last word.
        let protect = HandlerOptions {
            priority: HandlerPriority::High,
            blocking: true,
            ignore_cancelled: true,
            // A short comfort for the player: it never holds up damage if this plugin fails.
            on_failure: FailureVerdict::Allow,
        };
        register!(
            Arrival,
            protect,
            EntityDamageEvent,
            EntityDamageByEntityEvent,
            EntityDamageByBlockEvent,
        );
    }
    Ok(())
}
