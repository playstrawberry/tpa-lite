//! The questions themselves and the two player helpers (the plugin target only).
use crate::{
    IGNORE_EXEMPT_NODE, IGNORE_PLUGIN, Id, MAX_SCAN, MAX_SUGGESTIONS, SPY_PLUGIN, ignore_request,
    parse_ignore_answer, parse_spies_answer, spies_request,
};
use strawberry_plugin_api::{
    Player, Server,
    command::{CommandSender, CommandSuggestion, CommandSuggestions, SuggestionRequest},
    commands::CommandSuggestionHandler,
    ipc::send_ipc_message,
    uuid::Uuid,
};

/// The player's id.
pub fn id(player: &Player) -> Id {
    let uuid = player.get_id();
    (uuid.high, uuid.low)
}

/// The online player with this id.
pub fn find(server: &Server, key: Id) -> Option<Player> {
    server.get_player_by_uuid(Uuid {
        high: key.0,
        low: key.1,
    })
}

/// Whether `reader` ignores `sender`. A sender who cannot be ignored (staff) is never ignored. No ignore
/// script, or an answer that is not one: nobody ignores.
pub fn ignores(reader: Id, sender: &Player) -> bool {
    if sender.has_permission(IGNORE_EXEMPT_NODE) {
        return false;
    }
    match send_ipc_message(IGNORE_PLUGIN, &ignore_request(reader, id(sender))) {
        Ok(Ok(bytes)) => parse_ignore_answer(&bytes).unwrap_or(false),
        _ => false,
    }
}

/// The staff members who switched social spy on. No spy script, or an answer that is not one: nobody.
pub fn spies() -> Vec<Id> {
    match send_ipc_message(SPY_PLUGIN, &spies_request()) {
        Ok(Ok(bytes)) => parse_spies_answer(&bytes).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Suggests the names of the other players the asker can see, for a `<player>` argument. A hidden player is
/// never offered. One request looks at no more than [`MAX_SCAN`] players and offers no more than
/// [`MAX_SUGGESTIONS`] names.
pub struct Names;

impl CommandSuggestionHandler for Names {
    fn suggest(
        &self,
        source: CommandSender,
        server: Server,
        request: SuggestionRequest,
    ) -> CommandSuggestions {
        let mut values = Vec::new();
        if let Some(viewer) = source.as_player() {
            let me = id(&viewer);
            for other in server.get_all_players().into_iter().take(MAX_SCAN) {
                let name = other.get_name();
                if crate::prefix_match(&request.remaining, &name)
                    && id(&other) != me
                    && viewer.can_see_player(other)
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
            length: u32::try_from(request.remaining.len()).unwrap_or(0),
            values,
        }
    }
}
