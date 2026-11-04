use std::collections::BTreeMap;

pub type Id = String;

/// The expiry of a request on a server with expiry turned off.
pub const NEVER: u64 = u64::MAX;
/// Hard limit of live requests on the whole server, whatever the settings say.
pub const MAX_REQUESTS: usize = 256;
pub const MAX_COOLDOWNS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    To,
    Here,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub world: String,
    pub pos: (f64, f64, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Waiting,
    Warming {
        until: u64,
        origin: Location,
        entity: i32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    pub from: Id,
    pub to: Id,
    pub from_name: String,
    pub to_name: String,
    pub kind: Kind,
    pub expires: u64,
    pub phase: Phase,
}

impl Request {
    pub fn mover(&self) -> &str {
        match self.kind {
            Kind::To => &self.from,
            Kind::Here => &self.to,
        }
    }
    #[cfg(test)]
    pub fn anchor(&self) -> &str {
        match self.kind {
            Kind::To => &self.to,
            Kind::Here => &self.from,
        }
    }
    pub fn deadline(&self) -> u64 {
        match self.phase {
            Phase::Waiting => self.expires,
            Phase::Warming { until, .. } => until,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    SelfRequest,
    Disabled,
    Busy,
    AlreadySent,
    Cooldown(u64),
    Full,
    Missing,
    Expired,
}

/// Which incoming request a recipient means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    Newest,
    Id(u64),
    Name(&'a str),
    All,
}

/// A request that went out, and the oldest one it pushed out of a full queue.
#[derive(Debug, PartialEq)]
pub struct Sent {
    pub request: Request,
    pub evicted: Option<Request>,
}

pub struct Model {
    pub requests: BTreeMap<u64, Request>,
    limit: usize,
    /// Travelling player to the request he is warming up for: one at a time.
    moving: BTreeMap<Id, u64>,
    cooldowns: BTreeMap<Id, u64>,
    warming: BTreeMap<i32, u64>,
    next: u64,
}

impl Model {
    /// `limit` is the most waiting requests one player can receive, and the most he can have out.
    pub fn new(limit: usize) -> Self {
        Self {
            requests: BTreeMap::new(),
            limit,
            moving: BTreeMap::new(),
            cooldowns: BTreeMap::new(),
            warming: BTreeMap::new(),
            next: 0,
        }
    }

    pub fn seed(&mut self, seed: u64) {
        self.next = seed;
    }

    /// `from` and `to` are (uuid, name). Every refusal leaves the state as it was.
    pub fn send(
        &mut self,
        from: (&str, &str),
        to: (&str, &str),
        kind: Kind,
        now: u64,
        ttl: u64,
        cooldown: u64,
    ) -> Result<Sent, Error> {
        if from.0 == to.0 {
            return Err(Error::SelfRequest);
        }
        let mut outgoing = 0;
        let mut waiting = 0;
        let mut oldest = None;
        for r in self.requests.values() {
            if r.from == from.0 {
                if r.to == to.0 {
                    return Err(Error::AlreadySent);
                }
                outgoing += 1;
            }
            if r.to == to.0 && r.phase == Phase::Waiting {
                waiting += 1;
                oldest.get_or_insert(r.id);
            }
        }
        if outgoing >= self.limit {
            return Err(Error::Busy);
        }
        self.cooldowns.retain(|_, until| *until > now);
        if let Some(until) = self.cooldowns.get(from.0) {
            return Err(Error::Cooldown(until - now));
        }
        let evict = oldest.filter(|_| waiting >= self.limit);
        if self.requests.len() - usize::from(evict.is_some()) >= MAX_REQUESTS
            || (cooldown > 0 && self.cooldowns.len() >= MAX_COOLDOWNS)
        {
            return Err(Error::Full);
        }
        self.next = self.next.checked_add(1).ok_or(Error::Full)?;
        let evicted = evict.and_then(|id| self.remove(id));
        let request = Request {
            id: self.next,
            from: from.0.into(),
            to: to.0.into(),
            from_name: from.1.into(),
            to_name: to.1.into(),
            kind,
            expires: if ttl == 0 {
                NEVER
            } else {
                now.saturating_add(ttl)
            },
            phase: Phase::Waiting,
        };
        if cooldown > 0 {
            self.cooldowns
                .insert(from.0.into(), now.saturating_add(cooldown));
        }
        self.requests.insert(request.id, request.clone());
        Ok(Sent { request, evicted })
    }

    fn live(r: &Request, now: u64) -> Result<Request, Error> {
        if now >= r.expires {
            Err(Error::Expired)
        } else {
            Ok(r.clone())
        }
    }

    pub fn pick(&self, to: &str, pick: Pick<'_>, now: u64) -> Result<Vec<Request>, Error> {
        let mut waiting = self
            .requests
            .values()
            .filter(|r| r.to == to && r.phase == Phase::Waiting);
        let one = match pick {
            Pick::All => {
                let all: Vec<_> = waiting.filter(|r| now < r.expires).cloned().collect();
                return if all.is_empty() {
                    Err(Error::Missing)
                } else {
                    Ok(all)
                };
            }
            Pick::Newest => waiting.next_back(),
            Pick::Id(id) => waiting.find(|r| r.id == id),
            Pick::Name(name) => waiting.rfind(|r| r.from_name.eq_ignore_ascii_case(name)),
        };
        Ok(vec![Self::live(one.ok_or(Error::Missing)?, now)?])
    }

    fn check(&self, to: &str, id: u64, now: u64) -> Result<Request, Error> {
        let r = self.requests.get(&id).ok_or(Error::Missing)?;
        if r.to != to || r.phase != Phase::Waiting {
            return Err(Error::Missing);
        }
        Self::live(r, now)
    }

    /// Takes a waiting request away for an immediate teleport; only its recipient can.
    pub fn take(&mut self, to: &str, id: u64, now: u64) -> Result<Request, Error> {
        let r = self.check(to, id, now)?;
        self.remove(id);
        Ok(r)
    }

    pub fn accept(
        &mut self,
        to: &str,
        id: u64,
        now: u64,
        delay: u64,
        origin: Location,
        entity: i32,
    ) -> Result<Request, Error> {
        let mut r = self.check(to, id, now)?;
        if self.moving.contains_key(r.mover()) {
            return Err(Error::Busy);
        }
        r.phase = Phase::Warming {
            until: now.saturating_add(delay),
            origin,
            entity,
        };
        self.moving.insert(r.mover().to_owned(), id);
        self.warming.insert(entity, id);
        self.requests.insert(id, r.clone());
        Ok(r)
    }

    pub fn damaged(&self, entity: i32) -> Option<u64> {
        self.warming.get(&entity).copied()
    }
    pub fn has_warmups(&self) -> bool {
        !self.warming.is_empty()
    }
    /// The warmup a player is travelling on.
    pub fn moving(&self, id: &str) -> Option<u64> {
        self.moving.get(id).copied()
    }
    pub fn remove(&mut self, id: u64) -> Option<Request> {
        let r = self.requests.remove(&id)?;
        if let Phase::Warming { entity, .. } = r.phase {
            self.warming.remove(&entity);
            self.moving.remove(r.mover());
        }
        Some(r)
    }
    /// Every request a player sends or receives, oldest first.
    pub fn involving(&self, id: &str) -> Vec<Request> {
        self.requests
            .values()
            .filter(|r| r.from == id || r.to == id)
            .cloned()
            .collect()
    }
    /// A sender's requests, oldest first; with a name only the one to that player.
    pub fn outgoing(&self, from: &str, name: Option<&str>) -> Vec<Request> {
        self.requests
            .values()
            .filter(|r| r.from == from && name.is_none_or(|n| r.to_name.eq_ignore_ascii_case(n)))
            .cloned()
            .collect()
    }
    /// The requests still waiting on this recipient, oldest first.
    pub fn waiting(&self, to: &str) -> Vec<Request> {
        self.requests
            .values()
            .filter(|r| r.to == to && r.phase == Phase::Waiting)
            .cloned()
            .collect()
    }
    pub fn leave(&mut self, id: &str) -> Vec<Request> {
        let gone = self.involving(id);
        for r in &gone {
            self.remove(r.id);
        }
        gone
    }
    /// The next moment something is due. A server with expiry off and no warmup has none.
    pub fn next_deadline(&self) -> Option<u64> {
        self.requests
            .values()
            .map(Request::deadline)
            .filter(|at| *at != NEVER)
            .min()
    }
    pub fn due(&self, now: u64) -> Vec<Request> {
        // A burst is drained across ticks; host work cannot scale with all online players.
        self.requests
            .values()
            .filter(|r| r.deadline() != NEVER && r.deadline() <= now)
            .take(16)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: (&str, &str) = ("a", "Alex");
    const B: (&str, &str) = ("b", "Bo");
    const C: (&str, &str) = ("c", "Cem");

    fn loc() -> Location {
        Location {
            world: "world".into(),
            pos: (1.5, 64.0, 2.5),
        }
    }
    fn model() -> Model {
        Model::new(5)
    }
    fn request(m: &mut Model, kind: Kind) -> Request {
        m.send(A, B, kind, 100, 60_000, 30_000).unwrap().request
    }
    fn party(prefix: &str, i: usize) -> (String, String) {
        (format!("{prefix}{i}"), format!("N{prefix}{i}"))
    }

    #[test]
    fn request_and_here_have_opposite_movers() {
        for (kind, mover, anchor) in [(Kind::To, "a", "b"), (Kind::Here, "b", "a")] {
            let r = request(&mut model(), kind);
            assert_eq!((r.mover(), r.anchor()), (mover, anchor));
        }
    }
    #[test]
    fn self_request_is_rejected_without_consuming_cooldown() {
        let mut m = model();
        assert_eq!(m.send(A, A, Kind::To, 100, 1, 30), Err(Error::SelfRequest));
        request(&mut m, Kind::To);
    }
    #[test]
    fn the_same_sender_cannot_repeat_a_pending_request() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        assert_eq!(m.send(A, B, Kind::To, 100, 1, 0), Err(Error::AlreadySent));
        assert_eq!(m.send(A, B, Kind::Here, 100, 1, 0), Err(Error::AlreadySent));
        assert_eq!(m.requests.len(), 1);
        assert_eq!(m.waiting("b"), vec![r]);
    }
    #[test]
    fn a_refused_repeat_does_not_restart_the_cooldown() {
        let mut m = model();
        request(&mut m, Kind::To);
        assert_eq!(
            m.send(A, B, Kind::To, 200, 1, 30_000),
            Err(Error::AlreadySent)
        );
        assert_eq!(
            m.send(A, C, Kind::To, 201, 1, 30_000),
            Err(Error::Cooldown(29_899))
        );
    }
    #[test]
    fn several_senders_can_queue_on_one_recipient_in_order() {
        let mut m = model();
        let a = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        let c = m.send(C, B, Kind::Here, 101, 1000, 0).unwrap().request;
        assert!(a.id < c.id);
        assert_eq!(m.waiting("b"), vec![a, c]);
    }
    #[test]
    fn a_player_can_ask_several_players_up_to_the_limit() {
        let mut m = Model::new(2);
        m.send(A, B, Kind::To, 100, 1000, 0).unwrap();
        m.send(A, C, Kind::To, 100, 1000, 0).unwrap();
        assert_eq!(
            m.send(A, ("d", "Dee"), Kind::To, 100, 1000, 0),
            Err(Error::Busy)
        );
        assert_eq!(m.outgoing("a", None).len(), 2);
    }
    #[test]
    fn a_full_queue_pushes_out_only_the_oldest_waiting_request() {
        let mut m = model();
        let mut first = None;
        for i in 0..5 {
            let (id, name) = party("s", i);
            let sent = m.send((&id, &name), B, Kind::To, 100, 1000, 0).unwrap();
            assert_eq!(sent.evicted, None);
            first.get_or_insert(sent.request);
        }
        let (id, name) = party("s", 5);
        let sent = m.send((&id, &name), B, Kind::To, 100, 1000, 0).unwrap();
        let first = first.unwrap();
        assert_eq!(sent.evicted, Some(first.clone()));
        assert_eq!(m.waiting("b").len(), 5);
        assert!(!m.requests.contains_key(&first.id));
        assert_eq!(m.involving("s0"), vec![]);
    }
    #[test]
    fn a_warming_request_is_not_in_the_queue_and_cannot_be_pushed_out() {
        let mut m = Model::new(1);
        let a = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        m.accept("b", a.id, 100, 3000, loc(), 9).unwrap();
        let c = m.send(C, B, Kind::To, 100, 1000, 0).unwrap();
        assert_eq!(c.evicted, None);
        assert!(m.requests.contains_key(&a.id));
        assert_eq!(m.waiting("b"), vec![c.request]);
    }
    #[test]
    fn a_refused_request_leaves_a_full_queue_alone() {
        let mut m = Model::new(1);
        let a = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        assert_eq!(
            m.send(A, B, Kind::To, 100, 1000, 0),
            Err(Error::AlreadySent)
        );
        assert_eq!(
            m.send(B, B, Kind::To, 100, 1000, 0),
            Err(Error::SelfRequest)
        );
        assert_eq!(m.waiting("b"), vec![a]);
    }
    #[test]
    fn no_argument_means_the_newest_waiting_request() {
        let mut m = model();
        m.send(A, B, Kind::To, 100, 1000, 0).unwrap();
        let c = m.send(C, B, Kind::To, 100, 1000, 0).unwrap().request;
        assert_eq!(m.pick("b", Pick::Newest, 100), Ok(vec![c]));
    }
    #[test]
    fn a_name_picks_that_sender_whatever_its_case() {
        let mut m = model();
        let a = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        m.send(C, B, Kind::To, 100, 1000, 0).unwrap();
        assert_eq!(m.pick("b", Pick::Name("aLeX"), 100), Ok(vec![a]));
        assert_eq!(m.pick("b", Pick::Name("Ale"), 100), Err(Error::Missing));
    }
    #[test]
    fn an_id_picks_exactly_that_request_of_that_recipient() {
        let mut m = model();
        let a = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        let c = m.send(C, A, Kind::To, 100, 1000, 0).unwrap().request;
        assert_eq!(m.pick("b", Pick::Id(a.id), 100), Ok(vec![a]));
        assert_eq!(m.pick("b", Pick::Id(c.id), 100), Err(Error::Missing));
        assert_eq!(m.pick("b", Pick::Id(c.id + 1), 100), Err(Error::Missing));
    }
    #[test]
    fn all_lists_the_live_requests_oldest_first() {
        let mut m = model();
        let a = m.send(A, B, Kind::To, 100, 50, 0).unwrap().request;
        let c = m.send(C, B, Kind::To, 100, 1000, 0).unwrap().request;
        assert_eq!(m.pick("b", Pick::All, 100), Ok(vec![a, c.clone()]));
        assert_eq!(m.pick("b", Pick::All, 150), Ok(vec![c]));
        assert_eq!(m.pick("a", Pick::All, 100), Err(Error::Missing));
    }
    #[test]
    fn ttl_boundary_is_expired_and_never_accepted() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        assert!(m.pick("b", Pick::Newest, r.expires - 1).is_ok());
        assert_eq!(m.pick("b", Pick::Newest, r.expires), Err(Error::Expired));
        assert_eq!(
            m.accept("b", r.id, r.expires, 3000, loc(), 9),
            Err(Error::Expired)
        );
        assert_eq!(m.take("b", r.id, r.expires), Err(Error::Expired));
        assert_eq!(m.due(r.expires), vec![r]);
    }
    #[test]
    fn with_expiry_off_a_request_waits_and_arms_no_timer() {
        let mut m = model();
        let r = m.send(A, B, Kind::To, 100, 0, 0).unwrap().request;
        assert_eq!(r.expires, NEVER);
        assert_eq!(m.next_deadline(), None);
        assert!(m.due(u64::MAX - 1).is_empty());
        assert!(m.pick("b", Pick::Newest, u64::MAX - 1).is_ok());
    }
    #[test]
    fn with_expiry_off_a_warmup_still_has_a_deadline() {
        let mut m = model();
        let r = m.send(A, B, Kind::To, 100, 0, 0).unwrap().request;
        m.accept("b", r.id, 200, 3000, loc(), 9).unwrap();
        assert_eq!(m.next_deadline(), Some(3200));
    }
    #[test]
    fn stale_click_and_sender_cannot_accept_or_deny() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        assert_eq!(m.take("a", r.id, 200), Err(Error::Missing));
        assert_eq!(m.take("b", r.id + 1, 200), Err(Error::Missing));
        assert_eq!(m.accept("a", r.id, 200, 1, loc(), 9), Err(Error::Missing));
        m.remove(r.id);
        let new = m.send(C, B, Kind::Here, 200, 1000, 0).unwrap().request;
        assert_ne!(r.id, new.id);
        assert_eq!(m.take("b", r.id, 200), Err(Error::Missing));
    }
    #[test]
    fn take_consumes_once() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        assert_eq!(m.take("b", r.id, 200), Ok(r.clone()));
        assert_eq!(m.take("b", r.id, 200), Err(Error::Missing));
        assert!(m.requests.is_empty());
    }
    #[test]
    fn accepting_transitions_once_to_delay_not_teleport() {
        let mut m = model();
        let id = request(&mut m, Kind::To).id;
        let r = m.accept("b", id, 200, 3000, loc(), 9).unwrap();
        assert_eq!(
            r.phase,
            Phase::Warming {
                until: 3200,
                origin: loc(),
                entity: 9
            }
        );
        assert_eq!(m.accept("b", id, 201, 3000, loc(), 9), Err(Error::Missing));
        assert_eq!(m.take("b", id, 201), Err(Error::Missing));
        assert!(m.due(3199).is_empty());
        assert_eq!(m.due(3200), vec![r]);
    }
    #[test]
    fn a_traveller_has_one_warmup_at_a_time() {
        let mut m = model();
        let first = m.send(C, B, Kind::Here, 100, 1000, 0).unwrap().request;
        let second = m.send(A, B, Kind::Here, 100, 1000, 0).unwrap().request;
        m.accept("b", first.id, 100, 3000, loc(), 9).unwrap();
        assert_eq!(m.moving("b"), Some(first.id));
        assert_eq!(
            m.accept("b", second.id, 100, 3000, loc(), 9),
            Err(Error::Busy)
        );
        assert_eq!(m.waiting("b"), vec![second.clone()]);
        m.remove(first.id);
        assert_eq!(m.moving("b"), None);
        assert!(m.accept("b", second.id, 100, 3000, loc(), 9).is_ok());
    }
    #[test]
    fn the_same_traveller_can_arrive_at_two_anchors_only_in_turn() {
        let mut m = model();
        let first = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        let second = m.send(A, C, Kind::To, 100, 1000, 0).unwrap().request;
        m.accept("b", first.id, 100, 3000, loc(), 9).unwrap();
        assert_eq!(
            m.accept("c", second.id, 100, 3000, loc(), 9),
            Err(Error::Busy)
        );
    }
    #[test]
    fn accepted_request_uses_delay_not_the_old_expiry() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        let r = m.accept("b", r.id, r.expires - 1, 3000, loc(), 9).unwrap();
        assert!(m.due(r.expires).is_empty());
        assert_eq!(m.next_deadline(), Some(r.expires + 2999));
    }
    #[test]
    fn cancel_deny_expiry_and_completion_are_idempotent() {
        let mut m = model();
        let r = request(&mut m, Kind::Here);
        assert_eq!(m.remove(r.id), Some(r.clone()));
        assert!(m.remove(r.id).is_none());
        assert!(m.involving("a").is_empty());
        assert!(m.involving("b").is_empty());
        assert_eq!(m.next_deadline(), None);
    }
    #[test]
    fn outgoing_is_by_sender_and_optionally_by_target_name() {
        let mut m = model();
        let to_b = m.send(A, B, Kind::To, 100, 1000, 0).unwrap().request;
        let to_c = m.send(A, C, Kind::To, 100, 1000, 0).unwrap().request;
        assert_eq!(m.outgoing("a", None), vec![to_b, to_c.clone()]);
        assert_eq!(m.outgoing("a", Some("cEM")), vec![to_c]);
        assert!(m.outgoing("b", None).is_empty());
    }
    #[test]
    fn quit_cleans_every_role_but_preserves_cooldown() {
        for who in ["a", "b"] {
            let mut m = model();
            let r = request(&mut m, Kind::To);
            assert_eq!(m.leave(who), vec![r]);
            assert!(m.requests.is_empty());
            assert_eq!(
                m.send(A, C, Kind::To, 101, 1, 1),
                Err(Error::Cooldown(29_999))
            );
            assert!(m.send(A, C, Kind::To, 30_100, 1, 1).is_ok());
        }
    }
    #[test]
    fn quit_of_a_warming_anchor_removes_the_warmup_index() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        m.accept("b", r.id, 100, 3000, loc(), 9).unwrap();
        assert_eq!(m.leave("b").len(), 1);
        assert_eq!(m.moving("a"), None);
        assert_eq!(m.damaged(9), None);
        assert!(!m.has_warmups());
    }
    #[test]
    fn request_and_cooldown_memory_are_hard_bounded() {
        let mut m = model();
        for i in 0..MAX_REQUESTS {
            let (a, an) = party("a", i);
            let (b, bn) = party("b", i);
            m.send((&a, &an), (&b, &bn), Kind::To, 0, 100, 100).unwrap();
        }
        assert_eq!(
            m.send(("x", "X"), ("y", "Y"), Kind::To, 0, 100, 100),
            Err(Error::Full)
        );
        assert_eq!(m.requests.len(), MAX_REQUESTS);
        while !m.requests.is_empty() {
            let due = m.due(100);
            assert!(due.len() <= 16);
            for r in due {
                m.remove(r.id);
            }
        }
        assert!(
            m.send(("x", "X"), ("y", "Y"), Kind::To, 100, 100, 100)
                .is_ok()
        );
    }
    #[test]
    fn damage_index_is_only_for_warming_mover_and_removed_once() {
        let mut m = model();
        let r = request(&mut m, Kind::To);
        assert_eq!(m.damaged(9), None);
        m.accept("b", r.id, 200, 3000, loc(), 9).unwrap();
        assert_eq!(m.damaged(9), Some(r.id));
        assert_eq!(m.damaged(10), None);
        m.remove(r.id);
        assert_eq!(m.damaged(9), None);
    }
    #[test]
    fn cooldown_entries_remain_bounded_after_many_cancelled_requests() {
        let mut m = model();
        for i in 0..MAX_COOLDOWNS {
            let (id, name) = party("sender", i);
            let r = m
                .send((&id, &name), B, Kind::To, 0, 1, 1000)
                .unwrap()
                .request;
            m.remove(r.id);
        }
        assert_eq!(
            m.send(("x", "X"), B, Kind::To, 0, 1, 1000),
            Err(Error::Full)
        );
        assert!(m.requests.is_empty());
        assert!(m.send(("x", "X"), B, Kind::To, 1000, 1, 1000).is_ok());
    }
    #[test]
    fn request_id_exhaustion_refuses_without_mutating_state() {
        let mut m = model();
        m.seed(u64::MAX);
        assert_eq!(m.send(A, B, Kind::To, 0, 1, 1), Err(Error::Full));
        assert!(m.requests.is_empty());
        assert!(m.involving("a").is_empty());
    }
}
