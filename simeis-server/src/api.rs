// Mutex acquisition order
// - Taken names
// - Player index
// - Player list
// - Player
// - Galaxy
// - Station
// - Market
// - SyslogFifo
// - PlayerFifo

use std::str::FromStr;
use std::ops::DerefMut;
use std::time::Instant;
use std::collections::BTreeMap;

use base64::{prelude::BASE64_STANDARD, Engine};
use serde_json::{json, to_value, Value};
use strum::IntoEnumIterator;

use ntex::web::types::Path;
use ntex::web::{self, HttpRequest, HttpResponse, ServiceConfig};

use simeis_data::crew::{CrewId, CrewMemberType};
use simeis_data::errors::Errcode;
use simeis_data::galaxy::station::StationId;
use simeis_data::galaxy::SpaceUnit;
use simeis_data::player::{PlayerId, PlayerKey};
use simeis_data::ship::module::{ShipModuleId, ShipModuleType};
use simeis_data::ship::resources::Resource;
use simeis_data::ship::upgrade::ShipUpgrade;
use simeis_data::ship::{Ship, ShipId};
use simeis_data::syslog::SyslogEvent;

use crate::GameState;

pub type ApiResult = Result<Value, Errcode>;

// TODO (#14) Use POST queries also, instead of everything with GET

// TODO (#14) Use query parameters (with ntex::web::types::Query) instead of plain URLs
// TODO (#14) Pass player key in HTTP headers

macro_rules! get_player {
    ($srv:ident, $req:ident) => {{
        let Some(key) = get_player_key(&$req) else {
            return build_response(Err(Errcode::NoPlayerKey));
        };
        let Some(id) = $srv.player_index.clone_val(&key).await else {
            return build_response(Err(Errcode::NoPlayerWithKey));
        };
        let player = $srv.players.clone_val(&id).await.unwrap();
        if player.read().await.lost {
            return build_response(Err(Errcode::PlayerLost));
        }
        player.clone()
    }};
}

macro_rules! get_station {
    ($srv:expr, $player:expr, $id:expr) => {{
        let player = $player.read().await;
        let Some(station) = player.stations.get($id).cloned() else {
            return build_response(Err(Errcode::NoSuchStation(*$id)));
        };
        station
    }};

    ($srv:expr, $id:expr; $player:expr) => {{
        let Some(station) = $player.stations.get($id).cloned() else {
            return build_response(Err(Errcode::NoSuchStation(*$id)));
        };
        station
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
            jsonmerge(&mut data, &json!({"error": "ok"}));
            data
        }
        Err(e) => {
            json!({"error": e.errmsg(), "type": format!("{e:?}")})
        }
    };

    HttpResponse::Ok()
        .content_type("application/json")
        .json(&body)
}

// Test the connection to the server
#[web::get("/ping")]
async fn ping() -> impl web::Responder {
    build_response(Ok(json!({"ping": "pong"})))
}

// Get the logs from the server
#[web::get("/syslogs")]
async fn get_syslogs(srv: GameState, req: HttpRequest) -> impl web::Responder {
    let player = get_player!(srv, req);
    let pid = player.read().await.id;
    let allfifo = srv.fifo_events.read().await;
    let Some(fifo) = allfifo.clone_val(&pid).await else {
        return build_response(Ok(json!({"nb": 0, "events": []})));
    };
    let mut fifo = fifo.write().await;
    let all_ev = fifo.remove_all();
    let res = all_ev
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
    build_response(Ok(json!({ "nb": res.len(), "events": res, })))
}

// Creates a new player in the game
#[web::get("/player/new/{name}")]
async fn new_player(srv: GameState, name: Path<String>) -> impl web::Responder {
    let name = name.to_string();
    for pname in srv.taken_names.read().await.iter() {
        if &name == pname {
            return build_response(Err(Errcode::PlayerAlreadyExists(name)));
        }
    }

    let res = srv.new_player(name).await;
    build_response(res.map(|(id, key)| {
        json!({
            "playerId": id,
            "key": key,
        })
    }))
}

// Get the status from the player of a given id. If the ID is yours, give extensive metadata, else, minimal informations
#[web::get("/player/{id}")]
async fn get_player(srv: GameState, id: Path<PlayerId>, req: HttpRequest) -> impl web::Responder {
    let Some(key) = get_player_key(&req) else {
        return build_response(Err(Errcode::NoPlayerKey));
    };
    let id = id.as_ref();

    let Some(player) = srv.players.clone_val(&id).await else {
        return build_response(Err(Errcode::PlayerNotFound(*id)));
    };
    let player = player.read().await;

    let res = if player.key == key {
        let mut stations = BTreeMap::new();
        for (id, station) in player.stations.iter() {
            let station = station.read().await;
            stations.insert(id, station.to_json(&id).await);
        }
        let ships = serde_json::to_value(
            player.ships.values().collect::<Vec<&Ship>>()
        ).unwrap();
        Ok(json!({
            "id": id,
            "name": player.name,
            "stations": stations,
            "money": player.money,
            "ships": ships,
            "costs": player.costs,
        }))
    } else {
        Ok(json!({
            "id": id,
            "name": player.name,
        }))
    };
    build_response(res)
}

// Get status of a station
#[web::get("/station/{station_id}")]
async fn get_station_status(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().await;
    let station = get_station!(srv, id.as_ref(); player);
    let station = station.read().await;
    build_response(Ok(station.to_json(&player.id).await))
}

// List all the ships available for buying
#[web::get("/station/{station_id}/shipyard/list")]
async fn list_shipyard_ships(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let station = get_station!(srv, player, id.as_ref());
    let station = station.read().await;

    let mut ships = vec![];
    for ship in station.shipyard.iter() {
        ships.push(json!({
            "id": ship.id,
            "modules": ship.modules,
            "reactor_power": ship.reactor_power,
            "cargo_capacity": ship.cargo.capacity,
            "fuel_tank_capacity": ship.fuel_tank_capacity,
            "hull_resistance": ship.hull_resistance,
            "price": ship.compute_price(),
        }));
    }
    build_response(Ok(json!({ "ships": ships })))
}

// Buy a ship from the station's shop
#[web::get("/station/{station_id}/shipyard/buy/{id}")]
async fn shipyard_buy_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    build_response(
        player
            .buy_ship(&mut station, *ship_id)
            .map(|v| json!({ "shipId": v, })),
    )
}

// List all upgrades available for buying on a specific ship, on the station
#[web::get("/station/{station_id}/shipyard/upgrade/{ship_id}")]
async fn shipyard_list_upgrades(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let player = player.read().await;
    let station = get_station!(srv, station_id; player);
    let station = station.read().await;

    let Some(ship) = player.ships.get(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    let mut res = BTreeMap::new();
    for upgr in ShipUpgrade::iter() {
        let price = station.get_ship_upgrade_price(ship, &upgr);
        res.insert(
            upgr,
            json!({
                "price": price,
                "description": upgr.description(),
            }),
        );
    }
    build_response(Ok(to_value(res).unwrap()))
}

// Buy an upgrade and install it on a ship
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
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    build_response(
        player
            .buy_ship_upgrade(&mut station, ship_id, &upgrade_type)
            .map(|v| json!({ "cost": v })),
    )
}

// Hire a new crew member on the station. Unless assigned, it will stay idle
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
    let mut player = player.write().await;
    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;
    let id = station.hire_crew(&player.id, crewtype).await;
    drop(station);
    player.update_costs().await;
    build_response(Ok(serde_json::json!({ "id": id })))
}

// List all the upgrades available for the crew of a specific ship
#[web::get("/station/{station_id}/crew/upgrade/ship/{ship_id}")]
async fn get_crew_upgrades(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();

    let player = get_player!(srv, req);
    let player = player.read().await;

    let Some(ship) = player.ships.get(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    let station = get_station!(srv, station_id; player);
    let station = station.read().await;
    if ship.position != station.position {
        return build_response(Err(Errcode::ShipNotInStation));
    }

    let mut res = BTreeMap::new();
    for (cid, cm) in ship.crew.0.iter() {
        res.insert(
            cid,
            json!({
                "member-type": cm.member_type,
                "rank": cm.rank + 1,
                "price": cm.price_next_rank(),
            }),
        );
    }
    build_response(Ok(to_value(res).unwrap()))
}

// Upgrade a crew member of a specific ship
#[web::get("/station/{station_id}/crew/upgrade/ship/{ship_id}/{crew_id}")]
async fn upgrade_ship_crew(
    srv: GameState,
    args: Path<(StationId, ShipId, CrewId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, crew_id) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;
    let station = get_station!(srv, station_id; player);
    let station = station.read().await;

    let res = player.upgrade_ship_crew(&station, ship_id, crew_id);
    if res.is_ok() {
        player.update_costs().await;
    }
    build_response(res.map(|(p, r)| json!({ "new-rank": r, "cost": p})))
}

// Upgrade a crew member of the station
#[web::get("/station/{station_id}/crew/upgrade/{crew_id}")]
async fn upgrade_station_crew(
    args: Path<(StationId, CrewId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id) = args.as_ref();
    let player = get_player!(srv, req);
    let mut player = player.write().await;
    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let res = player
        .upgrade_station_crew(station.deref_mut(), crew_id)
        .await;
    if res.is_ok() {
        drop(station);
        player.update_costs().await;
    }
    build_response(res.map(|(p, r)| json!({ "new-rank": r, "cost": p })))
}

// TODO (#14) Make this URL generic: work for any role on the station
// Assign a crew member as a trader on a station. The level of the trader will affect the fee rate applied on the market
#[web::get("/station/{station_id}/crew/assign/{crewid}/trading")]
async fn assign_trader(
    args: Path<(StationId, CrewId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id) = args.as_ref();

    let player = get_player!(srv, req);
    let player = player.read().await;
    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let res = station.assign_trader(&player.id, *crew_id).await;
    build_response(res.map(|_| json!({})))
}

// Assign a crew member as a pilot on a ship. The level of the pilot will affect the speed of the ship, as well as it's fuel consumption
#[web::get("/station/{station_id}/crew/assign/{crewid}/{shipid}/pilot")]
async fn assign_pilot(
    args: Path<(StationId, CrewId, ShipId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id, ship_id) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };
    let res = station.onboard_pilot(*crew_id, ship).await;
    build_response(res.map(|_| json!({})))
}

// Assign a crew member as an operator on a ship. The level of the crew member will affect the extraction rate of the resources
#[web::get("/station/{station_id}/crew/assign/{crewid}/{shipid}/{modid}")]
async fn assign_operator(
    args: Path<(StationId, CrewId, ShipId, ShipModuleId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id, ship_id, modid) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };
    let res = station.onboard_operator(*crew_id, ship, modid).await;
    build_response(res.map(|_| json!({})))
}

// Scan for planets around the station
#[web::get("/station/{station_id}/scan")]
async fn scan(id: Path<StationId>, srv: GameState, req: HttpRequest) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().await;

    let galaxy = srv.galaxy.read().await;

    let station = get_station!(srv, id.as_ref(); player);
    let station = station.read().await;

    let results = station.scan(&galaxy).await;
    build_response(Ok(to_value(&results).unwrap()))
}

// List all the modules available to buy on the station
#[web::get("/station/{station_id}/shop/modules")]
async fn get_prices_ship_module(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let _station = get_station!(srv, player, id.as_ref()); // Ensure it exists

    let mut res: BTreeMap<ShipModuleType, f64> = BTreeMap::new();
    for smod in ShipModuleType::iter() {
        let price = smod.get_price_buy();
        res.insert(smod, price);
    }

    build_response(Ok(to_value(res).unwrap()))
}

// Buy a ship module and install it on a ship
#[web::get("/station/{station_id}/shop/modules/{ship_id}/buy/{modtype}")]
async fn buy_ship_module(
    srv: GameState,
    args: Path<(StationId, ShipId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, modtype) = args.as_ref();
    let Ok(modtype) = ShipModuleType::from_str(modtype.as_str()) else {
        return build_response(Err(Errcode::InvalidArgument("modtype")));
    };

    let player = get_player!(srv, req);
    let mut player = player.write().await;
    let cost = modtype.get_price_buy();

    build_response(
        player
            .buy_ship_module(station_id, ship_id, modtype)
            .await
            .map(|v| {
                json!({
                    "id": v,
                    "cost": cost,
                })
            }),
    )
}

// List the available upgrades for a module on a ship
#[web::get("/station/{station_id}/shop/modules/{ship_id}/upgrade")]
async fn get_ship_module_upgrade_prices(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();

    let player = get_player!(srv, req);
    let player = player.read().await;

    let Some(ship) = player.ships.get(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    let station = get_station!(srv, station_id; player);
    let station = station.read().await;

    if ship.position != station.position {
        return build_response(Err(Errcode::ShipNotInStation));
    }

    let mut res = BTreeMap::new();
    for (id, smod) in ship.modules.iter() {
        res.insert(
            id,
            json!({
                "module-type": smod.modtype,
                "price": smod.price_next_rank(),
            }),
        );
    }
    build_response(Ok(to_value(res).unwrap()))
}

// Buy an upgrade for a module installed on a ship, the level of a module will affect the extraction rate for a resource, as well as what kind of resources it kind mine.
#[web::get("/station/{station_id}/shop/modules/{ship_id}/upgrade/{modid}")]
async fn buy_ship_module_upgrade(
    srv: GameState,
    args: Path<(StationId, ShipId, ShipModuleId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, mod_id) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let station = station.read().await;

    let res = player.buy_ship_module_upgrade(&station, ship_id, mod_id);

    build_response(res.map(|(c, r)| json!({
        "new-rank": r,
        "cost": c,
    })))
}

// Buy a storage expansion for the station
#[web::get("/station/{station_id}/shop/cargo/buy/{amount}")]
async fn buy_station_cargo(
    srv: GameState,
    args: Path<(StationId, usize)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, amnt) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, id; player);
    let mut station = station.write().await;

    let res = station.buy_cargo(player.deref_mut(), amnt).await;
    build_response(res.map(|v| to_value(v).unwrap()))
}

// List the upgrades for a station currently available
#[web::get("/station/{station_id}/upgrades")]
async fn get_station_upgrades(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().await;
    let station = get_station!(srv, id.as_ref(); player);
    let station = station.read().await;

    let cargoprice = station.cargo_price(&player.id).await;
    let traderprice = station.upgr_trader_price(&player.id).await;
    build_response(Ok(json!({
        "cargo-expansion": cargoprice,
        "trader-upgrade": traderprice,
    })))
}

// Use fuel in storage on the station to refuel the ship
#[web::get("/station/{station_id}/refuel/{ship_id}")]
async fn refuel_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    let res = station.refuel_ship(ship).await;
    build_response(res.map(|v| json!({"added-fuel": v})))
}

// Use the hull plates in storage on the station to repair the ship
#[web::get("/station/{station_id}/repair/{ship_id}")]
async fn repair_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = args.as_ref();
    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let Some(ship) = player.ships.get_mut(ship_id) else {
        return build_response(Err(Errcode::ShipNotFound(*ship_id)));
    };

    let res = station.repair_ship(ship).await;
    build_response(res.map(|v| json!({"added-hull": v})))
}

#[web::get("/ship/{ship_id}")]
async fn get_ship_status(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().await;

    let Some(ship) = player.ships.get(id.as_ref()) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    build_response(Ok(to_value(ship).unwrap()))
}

// Compute how much will cost a travel to a specific position (X, Y, Z)
#[web::get("/ship/{ship_id}/travelcost/{x}/{y}/{z}")]
async fn compute_travel_costs(
    srv: GameState,
    args: Path<(ShipId, SpaceUnit, SpaceUnit, SpaceUnit)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, x, y, z) = args.as_ref();

    let player = get_player!(srv, req);
    let player = player.read().await;

    let Some(ship) = player.ships.get(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    let costs = ship.compute_travel_costs((*x, *y, *z));
    build_response(costs.map(|v| to_value(v).unwrap()))
}

// Navigate to position (X, Y, Z), ship will have the state InFlight during the travel
#[web::get("/ship/{ship_id}/navigate/{x}/{y}/{z}")]
async fn ask_navigate(
    srv: GameState,
    args: Path<(ShipId, SpaceUnit, SpaceUnit, SpaceUnit)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (id, x, y, z) = args.as_ref();
    let coord = (*x, *y, *z);

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let Some(ship) = player.ships.get_mut(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    let costs = ship.set_travel(coord);
    build_response(costs.map(|cost| json!(cost)))
}

// Stop the naviguation, ship will become Idle, and stay in place
#[web::get("/ship/{ship_id}/navigation/stop")]
async fn stop_navigation(
    srv: GameState,
    args: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let id = args.as_ref();

    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let Some(ship) = player.ships.get_mut(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    let res = ship.stop_navigation();
    build_response(res.map(|pos| json!({"position": pos})))
}

// Start the extraction of resources on the planet, ship will have the state "Extracting" until its cargo is full
#[web::get("/ship/{ship_id}/extraction/start")]
async fn start_extraction(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let mut player = player.write().await;
    let Some(ship) = player.ships.get_mut(id.as_ref()) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    let galaxy = srv.galaxy.read().await;
    let res = ship.start_extraction(&galaxy).await;
    build_response(res.map(|v| to_value(v).unwrap()))
}

// Stop the extraction of resources on the planet
#[web::get("/ship/{ship_id}/extraction/stop")]
async fn stop_extraction(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let mut player = player.write().await;

    let Some(ship) = player.ships.get_mut(id.as_ref()) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };
    let res = ship.stop_extraction();

    build_response(res.map(|v| to_value(v).unwrap()))
}

// Unload a specific amount of a specific resource on the station's storage
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
    let mut player = player.write().await;

    let Some(ship) = player.ships.get(id) else {
        return build_response(Err(Errcode::ShipNotFound(*id)));
    };

    let Some(station) = player.find_station(|s| s.position == ship.position).await else {
        return build_response(Err(Errcode::ShipNotInStation));
    };

    let station = get_station!(srv, &station; player);
    let mut station = station.write().await;

    let pid = player.id;
    let ship = player.ships.get_mut(id).unwrap();
    let res = ship
        .unload_cargo(&resource, *amnt, station.deref_mut())
        .await;

    if let Ok(0.0) = res {
        srv.syslog
            .event(
                &pid,
                SyslogEvent::UnloadedNothing {
                    station_cargo: station.clone_cargo(&pid).await,
                    ship_cargo: ship.cargo.clone(),
                },
            )
            .await;
    }
    build_response(res.map(|v| json!({ "unloaded": v })))
}

// Get prices of each resources on the market
#[web::get("/market/prices")]
async fn get_market_prices(srv: GameState) -> impl web::Responder {
    log::debug!("Ask market prices");
    let tstart = std::time::Instant::now();
    let res = srv.market.to_json().await;
    log::debug!("Replied in {:?}", tstart.elapsed());
    build_response(Ok(res))
}

// Buy a specific resource on the market
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
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let res = station
        .buy_resource(&resource, *amnt, player.deref_mut(), &srv.market)
        .await;
    build_response(res.map(|tx| to_value(tx).unwrap()))
}

// Sell a specific resource on the market
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
    let mut player = player.write().await;

    let station = get_station!(srv, station_id; player);
    let mut station = station.write().await;

    let res = station
        .sell_resource(&resource, *amnt, player.deref_mut(), &srv.market)
        .await;
    build_response(res.map(|tx| to_value(tx).unwrap()))
}

// Get the fee rate applied on the market of a station, depends on the level of the trader
#[web::get("/market/{station_id}/fee_rate")]
async fn get_fee_rate(
    srv: GameState,
    station_id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let player = get_player!(srv, req);
    let player = player.read().await;
    let station = get_station!(srv, station_id.as_ref(); player);
    let station = station.read().await;
    let fee = station.get_fee_rate(&player.id).await;
    build_response(fee.map(|rate| json!({ "fee_rate": rate })))
}

#[cfg(feature = "testing")]
// Make the server tick a single time
#[web::get("/tick")]
async fn tick_server(srv: GameState) -> impl web::Responder {
    let Ok(_) = srv.send_sig.send(simeis_data::game::GameSignal::Tick).await else {
        return build_response(Err(Errcode::GameSignalSend));
    };
    build_response(Ok(json!({})))
}

#[cfg(feature = "testing")]
// Make the server tick N times
#[web::get("/tick/{n}")]
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
        let tstart = std::time::Instant::now();
        let player = srv.players.clone_val(&pid).await.unwrap();
        let player = player.read().await;
        let potential = {
            let mut s = 0.0;
            for (sid, station) in player.stations.iter() {
                let station = station.read().await;
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
        log::debug!("Got data for {} in {:?}", player.name, tstart.elapsed());
    }
    build_response(Ok(to_value(data).unwrap()))
}

// Get the version of the game
#[web::get("/version")]
async fn get_version() -> impl web::Responder {
    let v = env!("CARGO_PKG_VERSION");
    build_response(Ok(json!({"version": v})))
}

pub fn configure(srv: &mut ServiceConfig) {
    #[cfg(feature = "testing")]
    srv.service(tick_server).service(tick_server_n);

    srv.service(ping)
        .service(get_version)
        .service(gamestats)
        .service(resources_info)
        .service(get_syslogs)
        .service(hire_crew)
        .service(get_crew_upgrades)
        .service(upgrade_ship_crew)
        .service(upgrade_station_crew)
        .service(assign_pilot)
        .service(assign_operator)
        .service(assign_trader)
        .service(scan)
        .service(compute_travel_costs)
        .service(get_ship_status)
        .service(ask_navigate)
        .service(stop_navigation)
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
