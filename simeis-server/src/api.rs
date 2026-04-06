use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Instant;

use base64::{prelude::BASE64_STANDARD, Engine};
use serde_json::{json, to_value, Value};
use strum::IntoEnumIterator;

use ntex::web::types::Path;
use ntex::web::{self, HttpRequest, HttpResponse, ServiceConfig};

use simeis_data::crew::{CrewId, CrewMemberType};
use simeis_data::errors::Errcode;
use simeis_data::galaxy::station::StationId;
use simeis_data::galaxy::SpaceUnit;
use simeis_data::player::PlayerId;
use simeis_data::ship::module::{ShipModuleId, ShipModuleType};
use simeis_data::ship::resources::Resource;
use simeis_data::ship::upgrade::ShipUpgrade;
use simeis_data::ship::ShipId;
use simeis_data::syslog::SyslogEvent;

use crate::GameState;

pub type ApiResult = Result<Value, Errcode>;

// TODO use score from ntext to build paths

// TODO (#14) Use POST queries also, instead of everything with GET

// TODO (#14) Use query parameters (with ntex::web::types::Query) instead of plain URLs
// TODO (#14) Pass player key in HTTP headers

macro_rules! get_player_key {
    ($req:ident) => {
        'getk: {
            let Some(b64key) = $req.headers().get("Simeis-Key") else {
                return build_response(Err(Errcode::NoPlayerKey));
            };
            let mut key = [0; 128];
            if !BASE64_STANDARD
                .decode_slice(b64key, &mut key)
                .ok()
                .is_some()
            {
                return build_response(Err(Errcode::InvalidPlayerKey));
            };
            break 'getk key;
        }
    };
}

#[inline]
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

#[inline]
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

// Creates a new player in the game
#[web::post("/player/new/{name}")]
async fn new_player(srv: GameState, name: Path<String>) -> impl web::Responder {
    let name = name.to_string();
    let res = srv.new_player(name).await.map(|(id, key)| json!({
        "playerId": id,
        "key": key,
    }));
    build_response(res)
}

// Get the status from the player of a given id. If the ID is yours, give extensive metadata, else, minimal informations
#[web::get("/player/{id}")]
async fn get_player(srv: GameState, id: Path<PlayerId>, req: HttpRequest) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let id = id.as_ref();
    let data = srv.player_to_json(&pkey, id).await;
    build_response(data)
}

// TODO FIXME    Deadlock here
// Get status of a station
#[web::get("/station/{station_id}")]
async fn get_station_status(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let id = id.as_ref();
    let key = get_player_key!(req);
    let data = srv
        .map_station(&key, id, |pid, station| {
            Box::pin(async { Ok(station.to_json(pid).await) })
        })
        .await;
    build_response(data)
}

// List all the ships available for buying
#[web::get("/station/{station_id}/shipyard/list")]
async fn list_shipyard_ships(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let id = id.as_ref();
    let key = get_player_key!(req);
    let data = srv
        .map_station(&key, id, |_, station| {
            Box::pin(async {
                let mut ships = vec![];
                let shipyard = station.shipyard.read().await;
                for ship in shipyard.iter() {
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
                Ok(json!({ "ships": ships }))
            })
        })
        .await;
    build_response(data)
}

// Buy a ship from the station's shop
#[web::post("/station/{station_id}/shipyard/buy/{id}")]
async fn shipyard_buy_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = *args;
    let key = get_player_key!(req);
    let data = srv
        .map_player_mut(&key, |player| {
            Box::pin(async move {
                player
                    .buy_ship(&station_id, &ship_id)
                    .await
                    .map(|v| json!({ "shipId": v }))
            })
        })
        .await;
    build_response(data)
}

// List all upgrades available for buying on a specific ship, on the station
#[web::get("/station/{station_id}/shipyard/upgrade/{ship_id}")]
async fn shipyard_list_upgrades(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_ship_in_station(&pkey, &station_id, &ship_id, |_, station, ship| {
            Box::pin(async move {
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
                Ok(to_value(res).unwrap())
            })
        })
        .await;

    build_response(data)
}

// TODO POST body contains the specific upgrade to apply
// Buy an upgrade and install it on a ship
#[web::post("/station/{station_id}/shipyard/upgrade/{ship_id}/{upgrade_type}")]
async fn shipyard_buy_upgrade(
    srv: GameState,
    args: Path<(StationId, ShipId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, upgrade_type) = args.clone();
    let Ok(upgrade_type) = ShipUpgrade::from_str(&upgrade_type) else {
        return build_response(Err(Errcode::InvalidArgument("upgrade type")));
    };
    let pkey = get_player_key!(req);
    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                player
                    .buy_ship_upgrade(&station_id, &ship_id, &upgrade_type)
                    .await
                    .map(|v| json!({ "cost": v }))
            })
        })
        .await;
    build_response(data)
}

// Hire a new crew member on the station. Unless assigned, it will stay idle
#[web::post("/station/{station_id}/crew/hire/{crewtype}")]
async fn hire_crew(
    srv: GameState,
    args: Path<(StationId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crewtype) = args.as_ref();
    let Ok(crewtype) = CrewMemberType::from_str(crewtype.as_str()) else {
        return build_response(Err(Errcode::InvalidArgument("crewtype")));
    };

    let pkey = get_player_key!(req);
    let data = srv
        .map_station(&pkey, station_id, |pid, station| {
            Box::pin(async move {
                let id = station.hire_crew(pid, crewtype).await;
                Ok(json!({ "id": id}))
            })
        })
        .await;
    let _ = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async {
                player.update_costs().await;
                Ok(())
            })
        })
        .await;
    build_response(data)
}

// List all the upgrades available for the crew of a specific ship
#[web::get("/station/{station_id}/crew/upgrade/ship/{ship_id}")]
async fn get_crew_upgrades(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id) = *args;
    let pkey = get_player_key!(req);
    let data = srv
        .map_player(&pkey, |player| {
            Box::pin(async move {
                if !player.ship_in_station(&ship_id, &station_id)? {
                    return Err(Errcode::ShipNotInStation);
                }
                // SAFETY Checked on the ship_in_station function
                let ship = player.ships.get(&ship_id).unwrap();

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
                Ok(to_value(res).unwrap())
            })
        })
        .await;
    build_response(data)
}

// Upgrade a crew member of a specific ship
#[web::post("/station/{station_id}/crew/upgrade/ship/{ship_id}/{crew_id}")]
async fn upgrade_ship_crew(
    srv: GameState,
    args: Path<(StationId, ShipId, CrewId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, ship_id, crew_id) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                let res = player
                    .upgrade_ship_crew(&station_id, &ship_id, &crew_id)
                    .await;
                match res {
                    Ok((p, r)) => {
                        player.update_costs().await;
                        Ok(json!({ "new-rank": r, "cost": p }))
                    }
                    Err(e) => Err(e),
                }
            })
        })
        .await;
    build_response(data)
}

// Upgrade a crew member of the station
#[web::post("/station/{station_id}/crew/upgrade/{crew_id}")]
async fn upgrade_station_crew(
    args: Path<(StationId, CrewId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                player
                    .upgrade_station_crew(&station_id, &crew_id)
                    .await
                    .map(|(p, r)| json!({ "new-rank": r, "cost": p }))
            })
        })
        .await;
    build_response(data)
}

// TODO (#14) Make this URL generic: work for any role on the station
// Assign a crew member as a trader on a station. The level of the trader will affect the fee rate applied on the market
#[web::post("/station/{station_id}/crew/assign/{crewid}/trading")]
async fn assign_trader(
    args: Path<(StationId, CrewId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_station(&pkey, &station_id, |pid, station| {
            Box::pin(async move {
                station.assign_trader(pid, crew_id).await?;
                Ok(json!({}))
            })
        })
        .await;
    build_response(data)
}

// Assign a crew member as a pilot on a ship. The level of the pilot will affect the speed of the ship, as well as it's fuel consumption
#[web::post("/station/{station_id}/crew/assign/{crewid}/{shipid}/pilot")]
async fn assign_pilot(
    args: Path<(StationId, CrewId, ShipId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id, ship_id) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_ship_mut_in_station(&pkey, &station_id, &ship_id, |_, station, ship| {
            Box::pin(async move {
                if ship.pilot.is_some() {
                    return Err(Errcode::CrewNotNeeded);
                }
                station
                    .onboard_pilot(ship, &crew_id)
                    .await
                    .map(|_| json!({}))
            })
        })
        .await;
    build_response(data)
}

// Assign a crew member as an operator on a ship. The level of the crew member will affect the extraction rate of the resources
#[web::post("/station/{station_id}/crew/assign/{crewid}/{shipid}/{modid}")]
async fn assign_operator(
    args: Path<(StationId, CrewId, ShipId, ShipModuleId)>,
    srv: GameState,
    req: HttpRequest,
) -> impl web::Responder {
    let (station_id, crew_id, ship_id, mod_id) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_ship_mut_in_station(&pkey, &station_id, &ship_id, |_, station, ship| {
            Box::pin(async move {
                station
                    .onboard_operator(ship, &crew_id, &mod_id)
                    .await
                    .map(|_| json!({}))
            })
        })
        .await;
    build_response(data)
}

// Scan for planets around the station
#[web::get("/station/{station_id}/scan")]
async fn scan(id: Path<StationId>, srv: GameState, req: HttpRequest) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let station_id = *id;

    let data = srv
        .scan_galaxy(&pkey, &station_id)
        .await
        .map(|v| to_value(v).unwrap());
    build_response(data)
}

// List all the modules available to buy on the station
#[web::get("/station/{station_id}/shop/modules")]
async fn get_prices_ship_module(
    srv: GameState,
    id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let station_id = *id;
    // We need to ensure the station exist, even if we don't use it here
    let data = srv
        .map_station(&pkey, &station_id, |_, _| {
            Box::pin(async move {
                let mut res: BTreeMap<ShipModuleType, f64> = BTreeMap::new();
                for smod in ShipModuleType::iter() {
                    let price = smod.get_price_buy();
                    res.insert(smod, price);
                }
                Ok(to_value(res).unwrap())
            })
        })
        .await;
    build_response(data)
}

// Buy a ship module and install it on a ship
#[web::post("/station/{station_id}/shop/modules/{ship_id}/buy/{modtype}")]
async fn buy_ship_module(
    srv: GameState,
    args: Path<(StationId, ShipId, String)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, ship_id, modtype) = args.clone();
    let Ok(modtype) = ShipModuleType::from_str(modtype.as_str()) else {
        return build_response(Err(Errcode::InvalidArgument("modtype")));
    };
    let cost = modtype.get_price_buy();

    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                player
                    .buy_ship_module(&station_id, &ship_id, modtype)
                    .await
                    .map(|v| json!({ "id": v, "cost": cost }))
            })
        })
        .await;
    build_response(data)
}

// List the available upgrades for a module on a ship
#[web::get("/station/{station_id}/shop/modules/{ship_id}/upgrade")]
async fn get_ship_module_upgrade_prices(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, ship_id) = *args;

    let data = srv
        .map_ship_in_station(&pkey, &station_id, &ship_id, |_, _, ship| {
            Box::pin(async move {
                let mut res = BTreeMap::new();
                for (id, module) in ship.modules.iter() {
                    res.insert(
                        id,
                        json!({
                            "module-type": module.modtype.clone(),
                            "price": module.price_next_rank(),
                        }),
                    );
                }
                Ok(to_value(res).unwrap())
            })
        })
        .await;
    build_response(data)
}

// Buy an upgrade for a module installed on a ship, the level of a module will affect the extraction rate for a resource, as well as what kind of resources it kind mine.
#[web::post("/station/{station_id}/shop/modules/{ship_id}/upgrade/{modid}")]
async fn buy_ship_module_upgrade(
    srv: GameState,
    args: Path<(StationId, ShipId, ShipModuleId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, ship_id, mod_id) = *args;

    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                player
                    .buy_ship_module_upgrade(&station_id, &ship_id, &mod_id)
                    .await
                    .map(|(c, r)| json!({ "new-rank": r, "cost": c }))
            })
        })
        .await;
    build_response(data)
}

// Buy a storage expansion for the station
#[web::post("/station/{station_id}/shop/cargo/buy/{amount}")]
async fn buy_station_cargo(
    srv: GameState,
    args: Path<(StationId, usize)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, amnt) = *args;

    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                player
                    .buy_station_cargo(&station_id, amnt)
                    .await
                    .map(|v| to_value(v).unwrap())
            })
        })
        .await;
    build_response(data)
}

// List the upgrades for a station currently available
#[web::get("/station/{station_id}/upgrades")]
async fn get_station_upgrades(
    srv: GameState,
    args: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let station_id = *args;

    let data = srv
        .map_station(&pkey, &station_id, |pid, station| {
            Box::pin(async move {
                let cargoprice = station.cargo_price(pid).await;
                let traderprice = station.upgr_trader_price(pid).await;
                Ok(json!({
                    "cargo-expansion": cargoprice,
                    "trader-upgrade": traderprice,
                }))
            })
        })
        .await;
    build_response(data)
}

// Use fuel in storage on the station to refuel the ship
#[web::post("/station/{station_id}/refuel/{ship_id}")]
async fn refuel_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, ship_id) = *args;

    let data = srv
        .map_ship_mut_in_station(&pkey, &station_id, &ship_id, |_, station, ship| {
            Box::pin(async move {
                station
                    .refuel_ship(ship)
                    .await
                    .map(|v| json!({ "added-fuel": v }))
            })
        })
        .await;
    build_response(data)
}

// Use the hull plates in storage on the station to repair the ship
#[web::post("/station/{station_id}/repair/{ship_id}")]
async fn repair_ship(
    srv: GameState,
    args: Path<(StationId, ShipId)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, ship_id) = *args;
    let data = srv
        .map_ship_mut_in_station(&pkey, &station_id, &ship_id, |_, station, ship| {
            Box::pin(async move {
                station
                    .repair_ship(ship)
                    .await
                    .map(|v| json!({ "added-hull": v }))
            })
        })
        .await;
    build_response(data)
}

#[web::get("/ship/{ship_id}")]
async fn get_ship_status(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let ship_id = *id;
    let data = srv
        .map_ship(&pkey, &ship_id, |_, ship| {
            Box::pin(async move { Ok(to_value(ship).unwrap()) })
        })
        .await;
    build_response(data)
}

// Compute how much will cost a travel to a specific position (X, Y, Z)
#[web::get("/ship/{ship_id}/travelcost/{x}/{y}/{z}")]
async fn compute_travel_costs(
    srv: GameState,
    args: Path<(ShipId, SpaceUnit, SpaceUnit, SpaceUnit)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (ship_id, x, y, z) = *args;
    let pkey = get_player_key!(req);

    let data = srv
        .map_ship(&pkey, &ship_id, |_, ship| {
            Box::pin(async move {
                let cost = ship.compute_travel_costs((x, y, z))?;
                Ok(to_value(cost).unwrap())
            })
        })
        .await;
    build_response(data)
}

// Navigate to position (X, Y, Z), ship will have the state InFlight during the travel
#[web::post("/ship/{ship_id}/navigate/{x}/{y}/{z}")]
async fn ask_navigate(
    srv: GameState,
    args: Path<(ShipId, SpaceUnit, SpaceUnit, SpaceUnit)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (id, x, y, z) = *args;
    let data = srv
        .map_ship_mut(&pkey, &id, |_, ship| {
            Box::pin(async move {
                ship.set_travel((x, y, z))
                    .map(|cost| to_value(cost).unwrap())
            })
        })
        .await;
    build_response(data)
}

// Stop the naviguation, ship will become Idle, and stay in place
#[web::post("/ship/{ship_id}/navigation/stop")]
async fn stop_navigation(
    srv: GameState,
    args: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let id = *args;
    let data = srv
        .map_ship_mut(&pkey, &id, |_, ship| {
            Box::pin(async move { ship.stop_navigation().map(|pos| json!({ "position": pos })) })
        })
        .await;
    build_response(data)
}

// Start the extraction of resources on the planet, ship will have the state "Extracting" until its cargo is full
#[web::post("/ship/{ship_id}/extraction/start")]
async fn start_extraction(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let ship_id = *id;
    let data = srv
        .start_player_extraction(&pkey, &ship_id)
        .await
        .map(|v| to_value(v).unwrap());
    build_response(data)
}

// Stop the extraction of resources on the planet
#[web::post("/ship/{ship_id}/extraction/stop")]
async fn stop_extraction(
    srv: GameState,
    id: Path<ShipId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let ship_id = *id;
    let data = srv
        .map_player_mut(&pkey, |player| {
            Box::pin(async move {
                let ship = player.get_ship_mut(&ship_id)?;
                ship.stop_extraction().map(|v| to_value(v).unwrap())
            })
        })
        .await;
    build_response(data)
}

// Unload a specific amount of a specific resource on the station's storage
#[web::post("/ship/{ship_id}/unload/{station_id}/{resource}/{amount}")]
async fn unload_ship_cargo(
    srv: GameState,
    args: Path<(ShipId, StationId, String, f64)>,
    req: HttpRequest,
) -> impl web::Responder {
    let (ship_id, station_id, resource, amnt) = args.clone();
    let Ok(resource) = Resource::from_str(&resource) else {
        return build_response(Err(Errcode::InvalidArgument("resource")));
    };
    let pkey = get_player_key!(req);

    let data = srv
        .map_ship_mut_in_station(&pkey, &station_id, &ship_id, |_, station, ship| {
            Box::pin(async move { ship.unload_cargo(&resource, amnt, station).await })
        })
        .await;

    if let Ok(0.0) = data {
        let (pid, ev) = srv
            .map_ship_in_station(&pkey, &station_id, &ship_id, |pid, station, ship| {
                Box::pin(async move {
                    Ok((
                        pid,
                        SyslogEvent::UnloadedNothing {
                            station_cargo: station.clone_cargo(&pid).await,
                            ship_cargo: ship.cargo.clone(),
                        },
                    ))
                })
            })
            .await
            .unwrap();
        srv.syslog.event(&pid, ev).await;
    }
    build_response(data.map(|v| json!({ "unloaded": v })))
}

// Get prices of each resources on the market
#[web::get("/market/prices")]
async fn get_market_prices(srv: GameState) -> impl web::Responder {
    let res = srv.market.to_json().await;
    build_response(Ok(res))
}

// Buy a specific resource on the market
#[web::post("/market/{station_id}/buy/{resource}/{amnt}")]
async fn buy_resource(
    srv: GameState,
    args: Path<(StationId, String, f64)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, resource, amnt) = args.clone();
    let Ok(resource) = Resource::from_str(&resource) else {
        return build_response(Err(Errcode::InvalidArgument("resource")));
    };
    let data = srv
        .player_market_buy(&pkey, &station_id, &resource, amnt)
        .await
        .map(|tx| to_value(tx).unwrap());
    build_response(data)
}

// Sell a specific resource on the market
#[web::post("/market/{station_id}/sell/{resource}/{amnt}")]
async fn sell_resource(
    srv: GameState,
    args: Path<(StationId, String, f64)>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let (station_id, resource, amnt) = args.clone();
    let Ok(resource) = Resource::from_str(&resource) else {
        return build_response(Err(Errcode::InvalidArgument("resource")));
    };
    let data = srv
        .player_market_sell(&pkey, &station_id, &resource, amnt)
        .await
        .map(|tx| to_value(tx).unwrap());
    build_response(data)
}

// Get the fee rate applied on the market of a station, depends on the level of the trader
#[web::get("/market/{station_id}/fee_rate")]
async fn get_fee_rate(
    srv: GameState,
    station_id: Path<StationId>,
    req: HttpRequest,
) -> impl web::Responder {
    let pkey = get_player_key!(req);
    let data = srv
        .map_station(&pkey, &station_id, |pid, station| {
            Box::pin(async move {
                let rate = station.get_fee_rate(pid).await?;
                Ok(json!({ "fee_rate": rate }))
            })
        })
        .await;
    build_response(data)
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
