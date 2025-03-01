use serde::{Deserialize, Serialize};

use crate::{
    errors::Errcode,
    galaxy::{get_delta, get_direction, get_distance, SpaceCoord},
};

use super::Ship;

#[derive(Serialize, Deserialize)]
pub struct Travel {
    pub destination: SpaceCoord,
}
impl Travel {
    pub fn new(destination: SpaceCoord) -> Travel {
        Travel { destination }
    }

    pub fn compute_costs(&self, ship: &Ship) -> Result<TravelCost, Errcode> {
        if ship.pilot.is_none() {
            return Err(Errcode::NoPilotAssigned);
        }
        let distance = get_distance(&ship.position, &self.destination);
        if distance == 0.0 {
            return Err(Errcode::NullDistance);
        }

        let direction = get_direction(&ship.position, &self.destination);
        let time_secs = distance / ship.stats.speed;
        let fuel_consumption = ship.stats.fuel_consumption * time_secs;
        let hull_usage = ship.stats.hull_usage_rate * distance;

        Ok(TravelCost {
            direction,
            distance,
            duration: time_secs,
            fuel_consumption,
            hull_usage,
        })
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TravelCost {
    pub direction: (f64, f64, f64),
    pub distance: f64,
    pub duration: f64,
    pub fuel_consumption: f64,
    pub hull_usage: f64,
}

impl TravelCost {
    pub fn have_enough(&self, ship: &Ship) -> bool {
        (ship.fuel_tank >= self.fuel_consumption)
            && (ship.hull_decay_capacity - ship.hull_decay) >= self.hull_usage
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct FlightData {
    pub start: SpaceCoord,
    pub destination: SpaceCoord,
    pub delta: (f64, f64, f64),

    pub direction: (f64, f64, f64),
    pub dist_done: f64,
    pub dist_tot: f64,
}

impl FlightData {
    pub fn new(start: SpaceCoord, cost: &TravelCost, travel: &Travel) -> FlightData {
        FlightData {
            dist_done: 0.0,
            dist_tot: cost.distance,
            direction: cost.direction,
            delta: get_delta(&start, &travel.destination),
            destination: travel.destination,
            start,
        }
    }
}
