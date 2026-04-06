use std::collections::BTreeMap;
use std::str::FromStr;

use ntex::router::IntoPattern;
use ntex::web;
use ntex::web::scope;
use ntex::web::types::Path;
use ntex::web::HttpRequest;
use ntex::web::ServiceConfig;

use serde_json::json;
use serde_json::to_value;
use strum::IntoEnumIterator;

use simeis_data::errors::Errcode;
use simeis_data::galaxy::station::StationId;
use simeis_data::ship::upgrade::ShipUpgrade;
use simeis_data::ship::ShipId;

use crate::api::build_response;
use crate::api::GameState;

// List all the ships available for buying
#[web::get("/list")]
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
#[web::post("/buy/{id}")]
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
#[web::get("/upgrade/{ship_id}")]
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
#[web::post("/upgrade/{ship_id}/{upgrade_type}")]
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

pub fn configure<T: IntoPattern>(base: T, srv: &mut ServiceConfig) {
    srv.service(
        scope(base)
            .service(shipyard_buy_ship)
            .service(list_shipyard_ships)
            .service(shipyard_buy_upgrade)
            .service(shipyard_list_upgrades),
    );
}
