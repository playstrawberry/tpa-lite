use std::{
    collections::BTreeMap,
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use strawberry_plugin_api::{
    Player, Server,
    common::NamedColor,
    logging::{LogLevel, log},
    scheduler,
    scheduler::SchedulerExt,
    text::TextComponent,
    uuid,
};

use crate::{
    api, checks,
    config::Config,
    lang::{self, Overrides},
    model::{self, Location, Model, Phase, Request},
    prefs::{self, Preferences},
    safe,
};

pub const USE: &str = "tpa-lite:use";
pub const HERE: &str = "tpa-lite:here";
pub const ACCEPT: &str = "tpa-lite:accept";
pub const DENY: &str = "tpa-lite:deny";
pub const CANCEL: &str = "tpa-lite:cancel";
pub const TOGGLE: &str = "tpa-lite:toggle";

const MAX_PROTECTED: usize = 4096;
/// Request ids stay longer than any player name, so `/tpaccept <word>` is never ambiguous.
const MIN_REQUEST_ID: u64 = 1 << 60;
/// A change of mind within this time reaches the disk as one write.
const SAVE_DELAY_TICKS: u64 = 20;

pub struct State {
    pub model: Model,
    pub prefs: Preferences,
    protected: BTreeMap<i32, u64>,
    timer: Option<u32>,
    generation: u64,
    save_pending: bool,
}

pub struct Runtime {
    pub state: Mutex<State>,
    pub config: Config,
    messages: Overrides,
    data: PathBuf,
    start: Instant,
}

pub type Shared = Arc<Runtime>;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Reads the preferences at load. A file that cannot be read fully stops the load and stays as it
/// is, so a server owner's data is never replaced by an empty set.
pub fn read_preferences(data: &std::path::Path) -> Result<Preferences, &'static str> {
    let file = match std::fs::File::open(data.join(prefs::FILE)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(_) => return Err("preferences_error"),
    };
    let mut text = String::new();
    let read = file
        .take(prefs::MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| "preferences_error")?;
    if read as u64 > prefs::MAX_FILE_BYTES {
        return Err("preferences_error");
    }
    Preferences::parse(&text)
}

impl Runtime {
    pub fn new(config: Config, messages: Overrides, prefs: Preferences, data: PathBuf) -> Self {
        let mut model = Model::new(config.max_pending);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        model.seed(seed.max(MIN_REQUEST_ID));
        Self {
            state: Mutex::new(State {
                model,
                prefs,
                protected: BTreeMap::new(),
                timer: None,
                generation: 0,
                save_pending: false,
            }),
            config,
            messages,
            data,
            start: Instant::now(),
        }
    }
    pub fn now(&self) -> u64 {
        self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
    pub fn stop(&self) {
        self.save_preferences();
        let timer = {
            let mut state = lock(&self.state);
            let timer = state.timer.take();
            state.generation = state.generation.wrapping_add(1);
            state.model = Model::new(1);
            state.protected.clear();
            timer
        };
        if let Some(timer) = timer {
            scheduler::cancel_task(timer);
        }
    }

    pub fn say(&self, p: &Player, key: &str, args: &[&str]) {
        let code = lang::language(&p.get_locale());
        if let Some(custom) = self.messages.get(code, key) {
            if !custom.is_empty() {
                p.send_system_message(legacy(custom, args), false);
            }
            return;
        }
        let text = lang::text(code, key, args);
        p.send_system_message(
            TextComponent::text(&text).color_named(NamedColor::Yellow),
            false,
        );
    }

    /// Writes the preferences if they changed. On failure they stay marked: the next change or
    /// the unload tries again, and the file on disk stays whole.
    pub fn save_preferences(&self) -> bool {
        let text = {
            let mut state = lock(&self.state);
            state.save_pending = false;
            if !state.prefs.take_dirty() {
                return true;
            }
            state.prefs.render()
        };
        let path = self.data.join(prefs::FILE);
        let temporary = self.data.join(format!("{}.tmp", prefs::FILE));
        let written = std::fs::create_dir_all(&self.data)
            .and_then(|()| std::fs::write(&temporary, text))
            .and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(e) = written {
            log(
                LogLevel::Error,
                &format!("tpa-lite: could not save {}: {e}", prefs::FILE),
            );
            lock(&self.state).prefs.mark_dirty();
            return false;
        }
        true
    }

    pub fn protect(&self, entity: i32) {
        let duration = self.config.arrival_protection_ms;
        if duration == 0 {
            return;
        }
        let now = self.now();
        let mut state = lock(&self.state);
        state.protected.retain(|_, until| *until > now);
        if state.protected.len() < MAX_PROTECTED {
            state.protected.insert(entity, now.saturating_add(duration));
        }
    }
    pub fn unprotect(&self, entity: i32) {
        lock(&self.state).protected.remove(&entity);
    }
    pub fn any_protected(&self) -> bool {
        !lock(&self.state).protected.is_empty()
    }
    pub fn is_protected(&self, entity: i32) -> bool {
        let now = self.now();
        let mut state = lock(&self.state);
        match state.protected.get(&entity) {
            Some(until) if *until > now => true,
            Some(_) => {
                state.protected.remove(&entity);
                false
            }
            None => false,
        }
    }
}

fn legacy(raw: &str, args: &[&str]) -> TextComponent {
    // A name cannot bring in a colour code of its own.
    let clean: Vec<String> = args.iter().map(|a| a.replace('&', "")).collect();
    let clean: Vec<&str> = clean.iter().map(String::as_str).collect();
    TextComponent::from_legacy_string_with_code(&lang::fill(raw, &clean), '&')
}

/// Answers a message from another plugin.
pub fn ipc(shared: &Shared, from: &str, message: &[u8]) -> Result<Vec<u8>, String> {
    let (answer, changed) = {
        let mut state = lock(&shared.state);
        api::handle(
            &mut state.prefs,
            &shared.config.preference_api_plugins,
            from,
            message,
        )?
    };
    if changed {
        schedule_save(shared);
    }
    Ok(answer)
}

/// Writes off the tick, shortly after the last change. A task exists only while a write is due.
pub fn schedule_save(shared: &Shared) {
    {
        let mut state = lock(&shared.state);
        if state.save_pending {
            return;
        }
        state.save_pending = true;
    }
    let weak = Arc::downgrade(shared);
    scheduler::schedule_delayed_task(SAVE_DELAY_TICKS, move |server| {
        let weak = weak.clone();
        server.schedule_delayed_async_task(1, move |_| {
            if let Some(shared) = weak.upgrade() {
                shared.save_preferences();
            }
        });
    });
}

pub fn id(p: &Player) -> String {
    p.get_id().to_string()
}
pub fn player(server: &Server, id: &str) -> Option<Player> {
    uuid::parse(id).and_then(|id| server.get_player_by_uuid(id))
}
/// Whether `reader` ignores `sender`: the ignore script's answer over ipc (`sp-social`, contract `ignore/1`). No
/// such script on the server, or a sender who cannot be ignored: nobody ignores.
#[cfg(target_arch = "wasm32")]
pub fn ignored(reader: &Player, sender: &Player) -> bool {
    sp_social::runtime::ignores(sp_social::runtime::id(reader), sender)
}
#[cfg(not(target_arch = "wasm32"))]
pub fn ignored(_reader: &Player, _sender: &Player) -> bool {
    false
}
pub fn visible(server: &Server, viewer: &Player, other: &Player) -> bool {
    player(server, &id(other)).is_some_and(|other| viewer.can_see_player(other))
}
pub fn mutually_visible(server: &Server, a: &Player, b: &Player) -> bool {
    visible(server, a, b) && visible(server, b, a)
}
pub fn pair(server: &Server, r: &Request) -> Option<(Player, Player)> {
    let from = player(server, &r.from)?;
    let to = player(server, &r.to)?;
    let permission = match r.kind {
        model::Kind::To => USE,
        model::Kind::Here => HERE,
    };
    let from_health = from.get_health();
    let to_health = to.get_health();
    if !from.has_permission(permission)
        || !to.has_permission(ACCEPT)
        || !mutually_visible(server, &from, &to)
        || !from_health.is_finite()
        || from_health <= 0.0
        || !to_health.is_finite()
        || to_health <= 0.0
    {
        return None;
    }
    Some((from, to))
}
/// The traveller and the player he travels to.
pub fn roles<'a>(r: &Request, from: &'a Player, to: &'a Player) -> (&'a Player, &'a Player) {
    match r.kind {
        model::Kind::To => (from, to),
        model::Kind::Here => (to, from),
    }
}
pub fn notify(shared: &Shared, server: &Server, r: &Request, key: &str) {
    // No names here: visibility may have changed since the request was sent. The recipient of a hidden
    // request never heard of it, so its end is told to its sender alone.
    for id in [Some(&r.from), (!r.hidden).then_some(&r.to)]
        .into_iter()
        .flatten()
    {
        if let Some(p) = player(server, id) {
            shared.say(&p, key, &[]);
        }
    }
}
pub fn cancel(shared: &Shared, server: &Server, request: u64, key: &str) {
    let removed = { lock(&shared.state).model.remove(request) };
    if let Some(r) = removed {
        notify(shared, server, &r, key);
    }
    arm(shared, server);
}

pub fn arm(shared: &Shared, server: &Server) {
    let (old, next, generation) = {
        let mut state = lock(&shared.state);
        let old = state.timer.take();
        let next = state.model.next_deadline();
        state.generation = state.generation.wrapping_add(1);
        (old, next, state.generation)
    };
    if let Some(old) = old {
        scheduler::cancel_task(old);
    }
    let Some(next) = next else {
        return;
    };
    let ticks = next.saturating_sub(shared.now()).div_ceil(50).max(1);
    let weak = Arc::downgrade(shared);
    let timer = server.schedule_delayed_task(ticks, move |server| {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        {
            let mut state = lock(&shared.state);
            if state.generation != generation {
                return;
            }
            state.timer = None;
        }
        due(&shared, &server);
    });
    lock(&shared.state).timer = Some(timer);
}

/// The landing for this traveller, or `None` when there is no safe one.
pub fn landing(shared: &Shared, mover: &Player, anchor: &Player) -> Option<Location> {
    safe::destination(anchor, &safe::policy(&shared.config, mover))
}

/// Moves a traveller whose request is already consumed, so callbacks cannot apply it twice.
pub fn arrive(
    shared: &Shared,
    server: &Server,
    r: &Request,
    (mover, anchor): (&Player, &Player),
    dest: &Location,
    immediate: bool,
) {
    if shared.config.dismount {
        safe::dismount(mover);
    }
    let ok = safe::go(mover, anchor, dest);
    if ok {
        shared.protect(mover.as_entity().get_id() as i32);
    }
    if ok && immediate {
        shared.say(mover, "teleported", &[]);
        shared.say(anchor, "accepted", &[]);
    } else {
        notify(
            shared,
            server,
            r,
            if ok { "teleported" } else { "teleport_failed" },
        );
    }
}

fn due(shared: &Shared, server: &Server) {
    let now = shared.now();
    let batch = { lock(&shared.state).model.due(now) };
    for r in batch {
        match &r.phase {
            Phase::Waiting => {
                let taken = { lock(&shared.state).model.remove(r.id) };
                if let Some(r) = taken {
                    notify(shared, server, &r, "expired");
                }
            }
            Phase::Warming { origin, entity, .. } => {
                let Some((from, to)) = pair(server, &r) else {
                    cancel(shared, server, r.id, "unavailable");
                    continue;
                };
                let (mover, anchor) = roles(&r, &from, &to);
                if mover.as_entity().get_id() as i32 != *entity
                    || safe::here(mover).world != origin.world
                    || checks::moved(mover.get_position(), origin.pos)
                {
                    cancel(shared, server, r.id, "moved");
                    continue;
                }
                let Some(dest) = landing(shared, mover, anchor) else {
                    cancel(shared, server, r.id, "unsafe");
                    continue;
                };
                // Consume before calling the host: teleport callbacks can re-enter us.
                let taken = { lock(&shared.state).model.remove(r.id) };
                if taken.is_none() {
                    continue;
                }
                arrive(shared, server, &r, (mover, anchor), &dest, false);
            }
        }
    }
    arm(shared, server);
}

/// The chat line with the buttons. They are real commands that name the exact request.
pub fn buttons(shared: &Shared, to: &Player, r: &Request) {
    let code = lang::language(&to.get_locale());
    let mut row = TextComponent::text("");
    let mut any = false;
    for (key, command, color) in [
        ("accept_button", "tpaccept", NamedColor::Green),
        ("deny_button", "tpdeny", NamedColor::Red),
    ] {
        let label = match shared.messages.get(code, key) {
            Some("") => continue,
            Some(custom) => legacy(custom, &[]),
            None => TextComponent::text(&lang::text(code, key, &[])).color_named(color),
        };
        row = row
            .add_child(label.click_run_command(&format!("/{command} {}", r.id)))
            .add_text(" ");
        any = true;
    }
    if any {
        to.send_system_message(row, false);
    }
}

pub fn error(shared: &Shared, p: &Player, error: model::Error) {
    match error {
        model::Error::Cooldown(ms) => {
            shared.say(p, "cooldown", &[&ms.div_ceil(1000).to_string()]);
        }
        model::Error::SelfRequest => shared.say(p, "self", &[]),
        model::Error::Disabled => shared.say(p, "disabled", &[]),
        model::Error::Busy => shared.say(p, "busy", &[]),
        model::Error::AlreadySent => shared.say(p, "already_sent", &[]),
        model::Error::Full => shared.say(p, "full", &[]),
        model::Error::Missing => shared.say(p, "missing", &[]),
        model::Error::Expired => shared.say(p, "expired", &[]),
    }
}
