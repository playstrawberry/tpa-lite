use std::sync::Arc;

use strawberry_plugin_api::{
    Context, Player, Server,
    command::{
        Arg, ArgumentType, Command, CommandError, CommandNode, CommandSender, CommandSuggestion,
        CommandSuggestions, ConsumedArgs, StringType, SuggestionRequest,
    },
    commands::{CommandHandler, CommandSuggestionHandler},
};

use crate::{
    checks, lang,
    model::{self, Kind, Phase, Pick, Request},
    prefs,
    runtime::{self as rt, Shared},
    safe,
};

const MAX_SCAN: usize = 1024;
const MAX_SUGGESTIONS: usize = 20;

#[derive(Clone, Copy)]
enum Action {
    Send(Kind),
    Respond {
        accept: bool,
        all: bool,
    },
    Cancel,
    /// `/tpa toggle`: a visible player called `toggle` comes first.
    ToggleWord,
    Toggle,
    Usage,
}

struct Run(Shared, Action);

fn argument(args: &ConsumedArgs, name: &str) -> Option<String> {
    match args.get_value(name) {
        Arg::Simple(s) | Arg::Msg(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// A word on `/tpaccept` and `/tpdeny`. Buttons send a request id, which is longer than any name.
fn picked(word: &str) -> Option<Pick<'_>> {
    if word.len() > 17 && word.bytes().all(|b| b.is_ascii_digit()) {
        return word.parse().ok().map(Pick::Id);
    }
    checks::valid_name(word).then_some(Pick::Name(word))
}

impl CommandHandler for Run {
    fn handle(
        &self,
        sender: CommandSender,
        server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(p) = sender.as_player() else {
            sender.send_message(strawberry_plugin_api::text::TextComponent::text(
                &lang::text("en", "players_only", &[]),
            ));
            return Ok(0);
        };
        let permission = match self.1 {
            Action::Send(Kind::To) | Action::Usage | Action::ToggleWord => rt::USE,
            Action::Send(Kind::Here) => rt::HERE,
            Action::Respond { accept: true, .. } => rt::ACCEPT,
            Action::Respond { accept: false, .. } => rt::DENY,
            Action::Cancel => rt::CANCEL,
            Action::Toggle => rt::TOGGLE,
        };
        if !p.has_permission(permission) {
            self.0.say(&p, "permission", &[]);
            return Ok(0);
        }
        let me = rt::id(&p);
        match self.1 {
            Action::Usage => self.0.say(&p, "usage", &[]),
            Action::Send(kind) => match argument(&args, "player") {
                Some(name) => self.send(&server, &p, &me, kind, &name),
                None => self.0.say(&p, "not_online", &[]),
            },
            Action::Respond { accept, all } => {
                let word = argument(&args, "request");
                let pick = if all {
                    Some(Pick::All)
                } else {
                    match &word {
                        Some(word) => picked(word),
                        None => Some(Pick::Newest),
                    }
                };
                let Some(pick) = pick else {
                    self.0.say(&p, "missing", &[]);
                    return Ok(0);
                };
                self.respond(&server, &p, &me, accept, pick);
            }
            Action::Cancel => {
                let name = argument(&args, "player");
                let mine = { rt::lock(&self.0.state).model.outgoing(&me, name.as_deref()) };
                if mine.is_empty() {
                    self.0.say(&p, "missing", &[]);
                }
                for r in mine {
                    rt::cancel(&self.0, &server, r.id, "cancelled");
                }
            }
            Action::ToggleWord => {
                let named = server
                    .get_player_by_name("toggle")
                    .filter(|t| rt::id(t) != me && rt::mutually_visible(&server, &p, t));
                if named.is_some() || !p.has_permission(rt::TOGGLE) {
                    self.send(&server, &p, &me, Kind::To, "toggle");
                } else {
                    self.toggle(&server, &p, &me);
                }
            }
            Action::Toggle => self.toggle(&server, &p, &me),
        }
        Ok(1)
    }
}

impl Run {
    fn respond(&self, server: &Server, p: &Player, me: &str, accept: bool, pick: Pick<'_>) {
        let listed = { rt::lock(&self.0.state).model.pick(me, pick, self.0.now()) };
        let requests = match listed {
            Ok(requests) => requests,
            Err(e) => {
                rt::error(&self.0, p, e);
                return;
            }
        };
        let bulk = matches!(pick, Pick::All);
        // One accept never moves the player who gave it more than once.
        let mut moved_me = false;
        for r in requests {
            if !accept {
                let key = if rt::pair(server, &r).is_some() {
                    "denied"
                } else {
                    "unavailable"
                };
                rt::cancel(&self.0, server, r.id, key);
                continue;
            }
            let i_travel = r.mover() == me;
            if i_travel && moved_me {
                continue;
            }
            match self.accept(server, me, &r) {
                Ok(()) => moved_me |= i_travel,
                Err(e) if !bulk => rt::error(&self.0, p, e),
                Err(_) => {}
            }
        }
    }

    fn accept(&self, server: &Server, me: &str, r: &Request) -> Result<(), model::Error> {
        let Some((from, to)) = rt::pair(server, r) else {
            rt::cancel(&self.0, server, r.id, "unavailable");
            return Ok(());
        };
        let (mover, anchor) = rt::roles(r, &from, &to);
        if self.0.config.delay_ms == 0 {
            // No delay: the destination is where the other player stands at this moment.
            let Some(dest) = rt::landing(&self.0, mover, anchor) else {
                rt::cancel(&self.0, server, r.id, "unsafe");
                return Ok(());
            };
            let taken = { rt::lock(&self.0.state).model.take(me, r.id, self.0.now()) };
            let taken = taken?;
            rt::arrive(&self.0, server, &taken, (mover, anchor), &dest, true);
            rt::arm(&self.0, server);
            return Ok(());
        }
        let origin = safe::here(mover);
        let entity = mover.as_entity().get_id() as i32;
        let accepted = {
            rt::lock(&self.0.state).model.accept(
                me,
                r.id,
                self.0.now(),
                self.0.config.delay_ms,
                origin,
                entity,
            )
        };
        accepted?;
        self.0.say(
            mover,
            "warmup",
            &[&self.0.config.delay_ms.div_ceil(1000).to_string()],
        );
        self.0.say(anchor, "accepted", &[]);
        rt::arm(&self.0, server);
        Ok(())
    }

    fn toggle(&self, server: &Server, p: &Player, me: &str) {
        let Some(uuid) = prefs::parse_uuid(me) else {
            rt::error(&self.0, p, model::Error::Full);
            return;
        };
        let result = {
            let mut state = rt::lock(&self.0.state);
            let enabled = !state.prefs.enabled(uuid);
            state.prefs.set(uuid, enabled).map(|_| enabled)
        };
        let Ok(enabled) = result else {
            rt::error(&self.0, p, model::Error::Full);
            return;
        };
        rt::schedule_save(&self.0);
        if !enabled {
            let waiting = { rt::lock(&self.0.state).model.waiting(me) };
            for r in waiting {
                rt::cancel(&self.0, server, r.id, "cancelled");
            }
        }
        self.0
            .say(p, if enabled { "toggle_on" } else { "toggle_off" }, &[]);
    }

    /// The player a name means. A name only ever finds someone the sender may see, so a hidden
    /// player answers as a player who is not there.
    fn find(&self, server: &Server, p: &Player, me: &str, name: &str) -> Option<Player> {
        if !checks::valid_name(name) {
            return None;
        }
        if let Some(to) = server.get_player_by_name(name)
            && rt::mutually_visible(server, p, &to)
        {
            return Some(to);
        }
        if !self.0.config.partial_names {
            return None;
        }
        // Two candidates are no answer: never a guess, and no sign of who else matched.
        let mut found = None;
        for other in server.get_all_players().into_iter().take(MAX_SCAN) {
            if !checks::prefix_match(name, &other.get_name())
                || rt::id(&other) == me
                || !rt::mutually_visible(server, p, &other)
            {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(other);
        }
        found
    }

    fn send(&self, server: &Server, p: &Player, me: &str, kind: Kind, name: &str) {
        let Some(to) = self.find(server, p, me, name) else {
            self.0.say(p, "not_online", &[]);
            return;
        };
        if p.get_health() <= 0.0 || to.get_health() <= 0.0 || !to.has_permission(rt::ACCEPT) {
            self.0.say(p, "unavailable", &[]);
            return;
        }
        let to_id = rt::id(&to);
        let my_name = p.get_name();
        let to_name = to.get_name();
        let sent = {
            let mut state = rt::lock(&self.0.state);
            // After the visibility check: the switch of a hidden player is never revealed.
            let open = prefs::parse_uuid(&to_id).is_some_and(|id| state.prefs.enabled(id));
            if open || me == to_id {
                state.model.send(
                    (me, &my_name),
                    (&to_id, &to_name),
                    kind,
                    self.0.now(),
                    self.0.config.expiry_ms,
                    self.0.config.cooldown_ms,
                )
            } else {
                Err(model::Error::Disabled)
            }
        };
        let sent = match sent {
            Ok(sent) => sent,
            Err(e) => {
                rt::error(&self.0, p, e);
                return;
            }
        };
        if let Some(old) = &sent.evicted
            && let Some(sender) = rt::player(server, &old.from)
        {
            self.0.say(&sender, "expired", &[]);
        }
        self.announce(&to, &sent.request, &my_name);
        self.0.say(p, "sent", &[&to_name]);
        rt::arm(&self.0, server);
    }

    fn announce(&self, to: &Player, r: &Request, from_name: &str) {
        debug_assert_eq!(r.phase, Phase::Waiting);
        if self.0.config.expiry_ms == 0 {
            let key = match r.kind {
                Kind::To => "request_to_no_expiry",
                Kind::Here => "request_here_no_expiry",
            };
            self.0.say(to, key, &[from_name]);
        } else {
            let key = match r.kind {
                Kind::To => "request_to",
                Kind::Here => "request_here",
            };
            let seconds = self.0.config.expiry_ms.div_ceil(1000).to_string();
            self.0.say(to, key, &[from_name, &seconds]);
        }
        self.0.say(to, "typed_hint", &[]);
        if self.0.config.buttons {
            rt::buttons(&self.0, to, r);
        }
    }
}

/// Offers only the names the sender may see.
struct Names;

impl CommandSuggestionHandler for Names {
    fn suggest(
        &self,
        sender: CommandSender,
        server: Server,
        request: SuggestionRequest,
    ) -> CommandSuggestions {
        let start = request.start as usize;
        let typed = request.input.get(start..).unwrap_or("").to_owned();
        let mut values = Vec::new();
        if let Some(viewer) = sender.as_player() {
            let me = rt::id(&viewer);
            for other in server.get_all_players().into_iter().take(MAX_SCAN) {
                let name = other.get_name();
                if (typed.is_empty() || checks::prefix_match(&typed, &name))
                    && rt::id(&other) != me
                    && rt::mutually_visible(&server, &viewer, &other)
                {
                    values.push(CommandSuggestion {
                        value: name,
                        tooltip: None,
                    });
                    if values.len() == MAX_SUGGESTIONS {
                        break;
                    }
                }
            }
        }
        CommandSuggestions {
            start: request.start,
            length: u32::try_from(request.input.len().saturating_sub(start)).unwrap_or(0),
            values,
        }
    }
}

pub fn register(context: &Context, shared: &Shared) {
    let run = |action| Run(Arc::clone(shared), action);
    let word = ArgumentType::String(StringType::SingleWord);
    let command = |name: &str, description: &str| {
        let mut names = vec![name.to_owned()];
        if let Some(aliases) = shared.config.aliases.get(name) {
            names.extend(aliases.iter().cloned());
        }
        Command::new(&names, &lang::text("en", description, &[]))
    };
    let player = || CommandNode::argument("player", &word).suggest(Names);
    let send = |name: &str, kind, description: &str| {
        command(name, description)
            .execute(run(Action::Usage))
            .then(player().execute(run(Action::Send(kind))))
    };
    let mut tpa = send("tpa", Kind::To, "desc_tpa");
    if shared.config.toggle_subcommand {
        tpa = tpa.then(CommandNode::literal("toggle").execute(run(Action::ToggleWord)));
    }
    context.register_command(tpa, rt::USE);
    context.register_command(send("tpahere", Kind::Here, "desc_here"), rt::HERE);
    for (name, accept, perm, description) in [
        ("tpaccept", true, rt::ACCEPT, "desc_accept"),
        ("tpdeny", false, rt::DENY, "desc_deny"),
    ] {
        let cmd = command(name, description)
            .execute(run(Action::Respond { accept, all: false }))
            .then(CommandNode::literal("*").execute(run(Action::Respond { accept, all: true })))
            .then(
                CommandNode::argument("request", &word)
                    .execute(run(Action::Respond { accept, all: false })),
            );
        context.register_command(cmd, perm);
    }
    context.register_command(
        command("tpacancel", "desc_cancel")
            .execute(run(Action::Cancel))
            .then(player().execute(run(Action::Cancel))),
        rt::CANCEL,
    );
    context.register_command(
        command("tptoggle", "desc_toggle").execute(run(Action::Toggle)),
        rt::TOGGLE,
    );
}
