use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;

use base64::{prelude::BASE64_STANDARD, Engine};
use ntex::web::types::Path;
use ntex::web::{self, HttpRequest, HttpResponse, ServiceConfig};
use serde_json::{json, Value};
use simeis_data::crew::{CrewId, CrewMemberType};
use simeis_data::galaxy::station::StationId;
use simeis_data::galaxy::SpaceUnit;
use simeis_data::market::fee_rate;
use simeis_data::player::{PlayerId, PlayerKey};
use simeis_data::ship::module::{ShipModuleId, ShipModuleType};
use simeis_data::ship::resources::Resource;
use simeis_data::ship::upgrade::ShipUpgrade;
use simeis_data::ship::ShipId;
use strum::IntoEnumIterator;

pub type ApiResult = Result<serde_json::Value, Errcode>;

use simeis_data::errors::Errcode;

use crate::GameState;

// TODO (#35) Use query parameters (with ntex::web::types::Query) instead of plain URLs

macro_rules! get_player {
    ($srv:ident, $req:ident) => {{
        let Some(key) = get_player_key(&$req) else {
            return build_response(Err(Errcode::NoPlayerKey));
        };
        let index = $srv.player_index.read().unwrap();
        let Some(id) = index.get(&key) else {
            return build_response(Err(Errcode::NoPlayerWithKey));
        };
        let players = $srv.players.read().unwrap();
        let player = players.get(id).unwrap();
        if player.read().unwrap().lost {
            return build_response(Err(Errcode::PlayerLost));
        }
        player.clone()
    }};
}

macro_rules! get_station {
    ($srv:ident, $player:ident, $id:expr) => {{
        let player = $player.read().unwrap();
        let Some(station_coord) = player.stations.get($id) else {
            return build_response(Err(Errcode::NoSuchStation(*$id)));
        };
        $srv.galaxy.get_station(station_coord).unwrap()
    }};
}

fn get_player_key(req: &HttpRequest) -> Option<PlayerKey> {
    for q in req.query_string().split("&") {
        if q.starts_with("key=") {
            let key = q.split("=").nth(1)?;
            let deckey = urlencoding::decode(key).ok()?;
            let mut key = [0; 128];
            BASE64_STANDARD
                .decode_slice(deckey.as_ref(), &mut key)
                .ok()?;
            return Some(key);
        }
    }
    None
}

pub fn jsonmerge(a: &mut Value, b: &Value) {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                jsonmerge(a.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (a, b) => *a = b.clone(),
    }
}

fn build_response(res: ApiResult) -> HttpResponse {
    let body = match res {
        Ok(mut data) => {
            jsonmerge(&mut data, &serde_json::json!({"error": "ok"}));
            data
        }
        Err(e) => {
            serde_json::json!({"error": e.errmsg(), "type": format!("{e:?}")})
        }
    };

    HttpResponse::Ok()
        .content_type("application/json")
        .json(&body)
}

#[web::get("/ping")]
async fn ping() -> impl web::Responder {
    build_response(Ok(json!({"ping": "pong"})))
}

#[web::get("/syslogs")]
async fn get_syslogs(srv: GameState, req: HttpRequest) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().unwrap();
    let allfifo = srv.fifo_events.read().unwrap();
    let Some(fifo) = allfifo.get(&player.id) else {
        return build_response(Ok(json!({"nb": 0, "events": []})));
    };
    let mut fifo = fifo.write().unwrap();
    let all_ev = fifo.remove_all();
    let res = all_ev
        .into_iter()
        .map(|(t, ev)| {
            let s: &'static str = ev.into();
            json!({
                "timestamp": srv.tstart + t,
                "type": s,
                "event": ev,
            })
        })
        .collect::<Vec<serde_json::Value>>();

    log::debug!("GOT LOGS {res:?}");
    build_response(Ok(json!({ "nb": res.len(), "events": res, })))
}

#[web::get("/player/new/{name}")]
async fn new_player(srv: GameState, name: Path<String>) -> impl web::Responder {
    build_response(srv.new_player(&name).map(|(id, key)| {
        serde_json::json!({
            "playerId": id,
            "key": key,
        })
    }))
}

#[web::get("/player/{id}")]
async fn get_player(srv: GameState, id: Path<PlayerId>, req: HttpRequest) -> impl web::Responder {
    let Some(key) = get_player_key(&req) else {
        return build_response(Err(Errcode::NoPlayerKey));
    };
    build_response(crate::player::get_player(srv, *id, key))
}

#[web::get("/station/{station_id}")]
async fn get_station_status(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, id.as_ref());
    let station = station.read().unwrap();
    build_response(Ok(serde_json::json!({
        "id": station.id,
        "position": station.position,
        "crew": station.crew,
        "cargo": station.cargo,
        "idle_crew": station.idle_crew,
        "trader": station.trader,
    })))
}

#[web::get("/station/{station_id}/shipyard/list")]
async fn list_shipyard_ships(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, id.as_ref());
    let station = station.read().unwrap();
    let mut ships = vec![];
    for ship in station.shipyard.iter() {
        ships.push(serde_json::json!({
            "id": ship.id,
            "modules": ship.modules,
            "reactor_power": ship.reactor_power,
            "cargo_capacity": ship.cargo.capacity,
            "fuel_tank_capacity": ship.fuel_tank_capacity,
            "hull_decay_capacity": ship.hull_decay_capacity,
            "price": ship.compute_price(),
        }));
    }
    build_response(Ok(serde_json::json!({ "ships": ships })))
}

#[web::get("/station/{station_id}/shipyard/buy/{id}")]
async fn shipyard_buy_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let mut station = station.write().unwrap();
    build_response(
        player
            .buy_ship(&mut station, *ship_id)
            .map(|v| serde_json::json!({ "shipId": v, })),
    )
}

#[web::get("/station/{station_id}/shipyard/upgrade")]
async fn shipyard_list_upgrades(
    srv: GameState,
    station_id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id.as_ref());
    let station = station.read().unwrap();
    let mut res = BTreeMap::new();
    for upgr in ShipUpgrade::iter() {
        res.insert(
            upgr,
            serde_json::json!({
                "price": station.get_ship_upgrade_price(&upgr),
                "description": upgr.description(),
            }),
        );
    }
    build_response(Ok(serde_json::to_value(res).unwrap()))
}

#[web::get("/station/{station_id}/shipyard/upgrade/{ship_id}/{upgrade_type}")]
async fn shipyard_buy_upgrade(
    srv: GameState,
    args: Path<(StationId, ShipId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, upgrade_type) = args.as_ref();
    let Ok(upgrade_type) = ShipUpgrade::from_str(upgrade_type) else {
        return build_response(Err(Errcode::InvalidArgument("upgrade type")));
    };
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let mut station = station.write().unwrap();
    build_response(
        player
            .buy_ship_upgrade(&mut station, ship_id, &upgrade_type)
            .map(|v| serde_json::json!({ "cost": v })),
    )
}

#[web::get("/station/{station_id}/crew/hire/{crewtype}")]
async fn hire_crew(
    srv: GameState,
    args: Path<(StationId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crewtype) = args.as_ref();
    let Ok(crewtype) = CrewMemberType::from_str(crewtype.as_str()) else {
        return build_response(Err(Errcode::InvalidArgument("crewtype")));
    };
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    build_response(crate::crew::hire_crew(
        &srv.galaxy,
        player,
        station,
        crewtype,
    ))
}

#[web::get("/station/{station_id}/crew/upgrade/ship/{ship_id}")]
async fn get_crew_upgrades(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let player = player.read().unwrap();
    let Some(ship) = player.ships.get(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };
    if ship.position != station.read().unwrap().position {
        return build_response(Err(Errcode::ShipNotInStation));
    }

    let mut res = BTreeMap::new();
    for (cid, cm) in ship.crew.0.iter() {
        res.insert(
            cid,
            serde_json::json!({
                "member-type": cm.member_type,
                "rank": cm.rank,
                "price": cm.price_next_rank(),
            }),
        );
    }
    build_response(Ok(serde_json::to_value(res).unwrap()))
}

#[web::get("/station/{station_id}/crew/upgrade/ship/{ship_id}/{crew_id}")]
async fn buy_crew_upgrade(
    srv: GameState,
    args: Path<(StationId, ShipId, CrewId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, crew_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let station = station.read().unwrap();
    let res = player.upgrade_crew_rank(&station, ship_id, crew_id);
    if res.is_ok() {
        player.update_wages(&srv.galaxy);
    }
    build_response(res.map(|(p, r)| serde_json::json!({ "new-rank": r, "cost": p})))
}

#[web::get("/station/{station_id}/crew/upgrade/station/trader")]
async fn upgrade_station_trader(
    station_id: Path<StationId>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id.as_ref());
    let mut player = player.write().unwrap();
    let res = player.upgrade_station_trader(station.write().unwrap().deref_mut());
    if res.is_ok() {
        player.update_wages(&srv.galaxy);
    }
    build_response(res.map(|(p, r)| serde_json::json!({ "new-rank": r, "cost": p })))
}

#[web::get("/station/{station_id}/crew/assign/{crewid}/trading")]
async fn assign_trader(
    args: Path<(StationId, CrewId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut station = station.write().unwrap();
    build_response(
        station
            .assign_trader(*crew_id)
            .map(|_| serde_json::json!({})),
    )
}

#[web::get("/station/{station_id}/crew/assign/{crewid}/{shipid}/pilot")]
async fn assign_pilot(
    args: Path<(StationId, CrewId, ShipId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };
    let mut station = station.write().unwrap();
    build_response(
        station
            .onboard_pilot(*crew_id, ship)
            .map(|_| serde_json::json!({})),
    )
}

#[web::get("/station/{station_id}/crew/assign/{crewid}/{shipid}/{modid}")]
async fn assign_operator(
    args: Path<(StationId, CrewId, ShipId, ShipModuleId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id, ship_id, modid) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };
    let mut station = station.write().unwrap();
    build_response(
        station
            .onboard_operator(*crew_id, ship, modid)
            .map(|_| serde_json::json!({})),
    )
}

#[web::get("/station/{station_id}/scan")]
async fn scan(id: Path<StationId>, srv: GameState, req: HttpRequest) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, id.as_ref());
    let results = station.read().unwrap().scan(&srv.galaxy);
    build_response(Ok(serde_json::to_value(&results).unwrap()))
}

#[web::get("/station/{station_id}/shop/modules")]
async fn get_prices_ship_module(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let _station = get_station!(srv, player, id.as_ref());
    // TODO (#22) Price based on station
    let mut res: BTreeMap<ShipModuleType, f64> = BTreeMap::new();
    for smod in ShipModuleType::iter() {
        let price = smod.get_price_buy();
        res.insert(smod, price);
    }
    build_response(Ok(serde_json::to_value(res).unwrap()))
}

#[web::get("/station/{station_id}/shop/modules/{ship_id}/buy/{modtype}")]
async fn buy_ship_module(
    srv: GameState,
    args: Path<(StationId, ShipId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, modtype) = args.as_ref();
    let player = get_player!(srv, req);

    let Ok(modtype) = ShipModuleType::from_str(modtype.as_str()) else {
        return build_response(Err(Errcode::InvalidArgument("modtype")));
    };
    let mut player = player.write().unwrap();
    build_response(
        player
            .buy_ship_module(station_id, ship_id, modtype)
            .map(|v| {
                serde_json::json!({
                    "id": v,
                })
            }),
    )
}

#[web::get("/station/{station_id}/shop/modules/{ship_id}/upgrade")]
async fn get_ship_module_upgrade_prices(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let player = player.read().unwrap();
    let Some(ship) = player.ships.get(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };
    if ship.position != station.read().unwrap().position {
        return build_response(Err(Errcode::ShipNotInStation));
    }

    let mut res = BTreeMap::new();
    for (id, smod) in ship.modules.iter() {
        res.insert(
            id,
            serde_json::json!({
                "module-type": smod.modtype,
                "price": smod.price_next_rank(),
            }),
        );
    }
    build_response(Ok(serde_json::to_value(res).unwrap()))
}

#[web::get("/station/{station_id}/shop/modules/{ship_id}/upgrade/{modid}")]
async fn buy_ship_module_upgrade(
    srv: GameState,
    args: Path<(StationId, ShipId, ShipModuleId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, mod_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let station = station.read().unwrap();
    build_response(
        player
            .buy_ship_module_upgrade(&station, ship_id, mod_id)
            .map(|(c, r)| {
                serde_json::json!({
                    "new-rank": r,
                    "cost": c,
                })
            }),
    )
}

#[web::get("/station/{station_id}/shop/cargo/buy/{amount}")]
async fn buy_station_cargo(
    srv: GameState,
    args: Path<(StationId, usize)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, amnt) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, id);

    let mut player = player.write().unwrap();
    let mut station = station.write().unwrap();
    build_response(
        station
            .buy_cargo(player.deref_mut(), amnt)
            .map(|v| serde_json::to_value(v).unwrap()),
    )
}

#[web::get("/station/{station_id}/upgrades")]
async fn get_station_upgrades(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, id.as_ref());
    let station = station.read().unwrap();
    let cargoprice = station.cargo_price();
    let traderprice = station.trader.map(|trader| {
        let cm = station.crew.0.get(&trader).unwrap();
        cm.price_next_rank()
    });
    build_response(Ok(serde_json::json!({
        "cargo-expansion": cargoprice,
        "trader-upgrade": traderprice,
    })))
}

#[web::get("/station/{station_id}/refuel/{ship_id}")]
async fn refuel_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut station = station.write().unwrap();
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    build_response(
        station
            .refuel_ship(ship)
            .map(|v| serde_json::json!({"added-fuel": v})),
    )
}

#[web::get("/station/{station_id}/repair/{ship_id}")]
async fn repair_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut station = station.write().unwrap();
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    build_response(
        station
            .repair_ship(ship)
            .map(|v| serde_json::json!({"added-hull": v})),
    )
}

#[web::get("/ship/{ship_id}")]
async fn get_ship_status(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().unwrap();
    let Some(ship) = player.ships.get(id.as_ref()) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    build_response(Ok(serde_json::to_value(ship).unwrap()))
}

#[web::get("/ship/{ship_id}/travelcost/{x}/{y}/{z}")]
async fn compute_travel_costs(
    srv: GameState,
    args: Path<(ShipId, SpaceUnit, SpaceUnit, SpaceUnit)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, x, y, z) = args.as_ref();
    let player = get_player!(srv, req);
    let player = player.read().unwrap();
    let Some(ship) = player.ships.get(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    build_response(
        ship.compute_travel_costs((*x, *y, *z))
            .map(|v| serde_json::to_value(v).unwrap()),
    )
}

#[web::get("/ship/{ship_id}/navigate/{x}/{y}/{z}")]
async fn ask_navigate(
    srv: GameState,
    args: Path<(ShipId, SpaceUnit, SpaceUnit, SpaceUnit)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, x, y, z) = args.as_ref();
    let coord = (*x, *y, *z);
    let player = get_player!(srv, req);
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    build_response(ship.set_travel(coord).map(|cost| serde_json::json!(cost)))
}

#[web::get("/ship/{ship_id}/extraction/start")]
async fn start_extraction(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(id.as_ref()) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    build_response(
        ship.start_extraction(&srv.galaxy)
            .map(|v| serde_json::to_value(v).unwrap()),
    )
}

#[web::get("/ship/{ship_id}/extraction/stop")]
async fn stop_extraction(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let mut player = player.write().unwrap();
    let Some(ship) = player.ships.get_mut(id.as_ref()) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    build_response(
        ship.stop_extraction()
            .map(|v| serde_json::to_value(v).unwrap()),
    )
}

#[web::get("/ship/{ship_id}/unload/{resource}/{amount}")]
async fn unload_ship_cargo(
    srv: GameState,
    args: Path<(ShipId, String, f64)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, resource, amnt) = args.as_ref();
    let Ok(resource) = Resource::from_str(resource) else {
        return build_response(Err(Errcode::InvalidArgument("resource")));
    };

    let player = get_player!(srv, req);
    let mut player = player.write().unwrap();

    let Some(ship) = player.ships.get(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };

    let Some(station) = player.stations.iter().find(|(_, s)| *s == &ship.position) else {
        return build_response(Err(Errcode::ShipNotInStation));
    };

    let station = srv.galaxy.get_station(station.1).unwrap();
    let mut station = station.write().unwrap();
    let ship = player.ships.get_mut(id).unwrap();
    build_response(
        ship.unload_cargo(&resource, *amnt, station.deref_mut())
            .map(|v| serde_json::json!({ "unloaded": v })),
    )
}

#[web::get("/market/prices")]
async fn get_market_prices(srv: GameState) -> impl web::Responder {
    let market = srv.market.read().unwrap();
    build_response(Ok(serde_json::to_value(market.deref()).unwrap()))
}

#[web::get("/market/{station_id}/buy/{resource}/{amnt}")]
async fn buy_resource(
    srv: GameState,
    args: Path<(StationId, String, f64)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, resource, amnt) = args.as_ref();
    let Ok(resource) = Resource::from_str(resource) else {
        return build_response(Err(Errcode::InvalidArgument("resource")));
    };
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let mut station = station.write().unwrap();
    let mut market = srv.market.write().unwrap();
    build_response(
        station
            .buy_resource(&resource, *amnt, player.deref_mut(), market.deref_mut())
            .map(|tx| serde_json::to_value(tx).unwrap()),
    )
}

#[web::get("/market/{station_id}/sell/{resource}/{amnt}")]
async fn sell_resource(
    srv: GameState,
    args: Path<(StationId, String, f64)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, resource, amnt) = args.as_ref();
    let Ok(resource) = Resource::from_str(resource) else {
        return build_response(Err(Errcode::InvalidArgument("resource")));
    };
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id);
    let mut player = player.write().unwrap();
    let mut station = station.write().unwrap();
    let mut market = srv.market.write().unwrap();
    build_response(
        station
            .sell_resource(&resource, *amnt, player.deref_mut(), market.deref_mut())
            .map(|tx| serde_json::to_value(tx).unwrap()),
    )
}

#[web::get("/market/{station_id}/fee_rate")]
async fn get_fee_rate(
    srv: GameState,
    station_id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, station_id.as_ref());
    let station = station.read().unwrap();
    let Some(trader) = station.trader else {
        return build_response(Err(Errcode::NoTraderAssigned));
    };
    let cm = station.crew.0.get(&trader).unwrap();
    let fee = fee_rate(cm.rank);
    build_response(Ok(serde_json::json!({
        "fee_rate": fee,
    })))
}

pub fn configure(srv: &mut ServiceConfig) {
    srv.service(ping)
        .service(get_syslogs)
        .service(hire_crew)
        .service(get_crew_upgrades)
        .service(buy_crew_upgrade)
        .service(upgrade_station_trader)
        .service(assign_pilot)
        .service(assign_operator)
        .service(assign_trader)
        .service(scan)
        .service(compute_travel_costs)
        .service(get_ship_status)
        .service(ask_navigate)
        .service(shipyard_buy_ship)
        .service(list_shipyard_ships)
        .service(shipyard_buy_upgrade)
        .service(shipyard_list_upgrades)
        .service(buy_ship_module)
        .service(get_ship_module_upgrade_prices)
        .service(buy_ship_module_upgrade)
        .service(get_prices_ship_module)
        .service(start_extraction)
        .service(stop_extraction)
        .service(unload_ship_cargo)
        .service(get_station_status)
        .service(get_station_upgrades)
        .service(buy_station_cargo)
        .service(refuel_ship)
        .service(repair_ship)
        .service(get_fee_rate)
        .service(get_market_prices)
        .service(buy_resource)
        .service(sell_resource)
        .service(get_player)
        .service(new_player);
}
