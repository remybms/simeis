use std::collections::BTreeMap;
use std::time::Instant;

use ntex::web;
use ntex::web::HttpRequest;
use ntex::web::ServiceConfig;

use serde_json::json;
use serde_json::to_value;
use serde_json::Value;
use strum::IntoEnumIterator;

use simeis_data::errors::Errcode;
use simeis_data::ship::resources::Resource;

use crate::api::build_response;
use crate::api::GameState;

// Test the connection to the server
#[web::get("/ping")]
async fn ping() -> impl web::Responder {
    build_response(Ok(json!({"ping": "pong"})))
}

// Get the logs from the server
#[web::get("/syslogs")]
async fn get_syslogs(srv: GameState, req: HttpRequest) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let data = match srv.get_syslogs(&pkey).await {
        Ok(got) => {
            let events = got
                .into_iter()
                .map(|(t, ev)| {
                    let s: &'static str = ev.clone().into();
                    json!({
                        "timestamp": srv.tstart + t,
                        "type": s,
                        "event": ev,
                    })
                })
                .collect::<Vec<Value>>();
            Ok(json!({ "nb": events.len(), "events": events }))
        }
        Err(e) => Err(e),
    };
    build_response(data)
}

// Get the version of the game
#[web::get("/version")]
async fn get_version() -> impl web::Responder {
    let v = env!("CARGO_PKG_VERSION");
    build_response(Ok(json!({"version": v})))
}

#[cfg(feature = "testing")]
// Make the server tick a single time
#[web::post("/tick")]
async fn tick_server(srv: GameState) -> impl web::Responder {
    let Ok(_) = srv.send_sig.send(simeis_data::game::GameSignal::Tick).await else {
        return build_response(Err(Errcode::GameSignalSend));
    };
    build_response(Ok(json!({})))
}

#[cfg(feature = "testing")]
// Make the server tick N times
#[web::post("/tick/{n}")]
async fn tick_server_n(srv: GameState, n: Path<usize>) -> impl web::Responder {
    let n = n.as_ref().clone();
    for _ in 0..n {
        let Ok(_) = srv.send_sig.send(simeis_data::game::GameSignal::Tick).await else {
            return build_response(Err(Errcode::GameSignalSend));
        };
    }
    build_response(Ok(json!({})))
}

// Get informations on all the resources on game
#[web::get("/resources")]
async fn resources_info() -> impl web::Responder {
    let mut data = BTreeMap::new();
    for res in Resource::iter() {
        if res.mineable(u8::MAX) || res.suckable(u8::MAX) {
            data.insert(
                format!("{res:?}"),
                json!({
                    "base-price": res.base_price(),
                    "volume": res.volume(),
                    "difficulty": res.extraction_difficulty(),
                    "min-rank": res.min_rank(),
                }),
            );
        } else {
            data.insert(
                format!("{res:?}"),
                json!({
                    "base-price": res.base_price(),
                    "volume": res.volume(),
                    "solid": res.mineable(u8::MAX),
                }),
            );
        }
    }
    build_response(Ok(to_value(data).unwrap()))
}

// Get the stats of the game, about all players
#[web::get("/gamestats")]
async fn gamestats(srv: GameState) -> impl web::Responder {
    let mut data = BTreeMap::new();
    let all_players = srv.players.get_all_keys().await;
    let mut all_stations = BTreeMap::new();
    for pid in all_players {
        let player = srv.players.clone_val(&pid).await.unwrap();
        let player = player.read().await;
        let potential = {
            let mut s = 0.0;
            for (sid, station) in player.stations.iter() {
                let sjson = station.to_json(&pid).await;
                all_stations.insert(*sid, sjson);
                s += station.get_cargo_potential_price(&pid).await;
            }
            s
        };

        let age = (Instant::now() - player.created).as_secs();
        data.insert(
            pid,
            json!({
                "name": player.name,
                "score": player.score,
                "potential": potential,
                "age": age,
                "lost": player.lost,
                "money": player.money,
                "stations": all_stations,
            }),
        );
    }
    build_response(Ok(to_value(data).unwrap()))
}

pub fn configure(srv: &mut ServiceConfig) {
    #[cfg(feature = "testing")]
    srv.service(tick_server).service(tick_server_n);

    srv.service(ping)
        .service(get_version)
        .service(gamestats)
        .service(resources_info);
}
