use serde_json::json;

use simeis_data::errors::Errcode;
use simeis_data::player::{PlayerId, PlayerKey};
use simeis_data::ship::Ship;

use crate::{api::ApiResult, GameState};

pub fn get_player(srv: GameState, id: PlayerId, key: PlayerKey) -> ApiResult {
    let players = srv.players.read().unwrap();
    let Some(playerlck) = players.get(&id) else {
        return Err(Errcode::PlayerNotFound(id));
    };

    let player = playerlck.read().unwrap();

    if player.key == key {
        Ok(json!({
            "id": id,
            "name": player.name,
            "stations": player.stations,
            "money": player.money,
            "ships": serde_json::to_value(
                player.ships.values().collect::<Vec<&Ship>>()
            ).unwrap(),
            "costs": player.costs,
        }))
    } else {
        Ok(json!({
            "id": id,
            "name": player.name,
            "stations": player.stations,
        }))
    }
}
