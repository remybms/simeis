use std::str::FromStr;

use ntex::router::IntoPattern;
use ntex::web;
use ntex::web::scope;
use ntex::web::types::Path;
use ntex::web::HttpRequest;
use ntex::web::ServiceConfig;

use serde_json::json;
use serde_json::to_value;
use simeis_data::galaxy::SpaceUnit;
use simeis_data::ship::resources::Resource;
use simeis_data::syslog::SyslogEvent;

use simeis_data::errors::Errcode;
use simeis_data::galaxy::station::StationId;
use simeis_data::ship::ShipId;

use crate::api::build_response;
use crate::api::GameState;

#[web::get("/")]
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
#[web::get("/travelcost/{x}/{y}/{z}")]
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
#[web::post("/navigate/{x}/{y}/{z}")]
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
#[web::post("/navigation/stop")]
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
#[web::post("/extraction/start")]
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
#[web::post("/extraction/stop")]
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
#[web::post("/unload/{station_id}/{resource}/{amount}")]
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

pub fn configure<T: IntoPattern>(base: T, srv: &mut ServiceConfig) {
    srv.service(
        scope(base)
            .service(compute_travel_costs)
            .service(get_ship_status)
            .service(ask_navigate)
            .service(stop_navigation)
            .service(start_extraction)
            .service(stop_extraction)
            .service(unload_ship_cargo),
    );
}
