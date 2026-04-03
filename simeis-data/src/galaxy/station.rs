use std::collections::BTreeMap;

use rand::RngExt;
use serde::{Deserialize, Serialize};
use mea::rwlock::RwLock;

use crate::crew::{Crew, CrewId, CrewMember, CrewMemberType};
use crate::errors::Errcode;
use crate::market::{fee_rate, Market, MarketTx};
use crate::player::{Player, PlayerId};
use crate::ship::cargo::ShipCargo;
use crate::ship::module::ShipModuleId;
use crate::ship::resources::Resource;
use crate::ship::upgrade::ShipUpgrade;
use crate::ship::Ship;

use super::scan::ScanResult;
use super::{Galaxy, SpaceCoord};

const CARGO_BASE_PRICE: f64 = 2.0;
const CARGO_PRICE_INCDIV: f64 = 100.0;
pub const STATION_INIT_CARGO: f64 = 1000.0;

pub type StationId = u16;

// TODO (#7) Add refineries to create fuel & hull plate from raw resources
#[derive(Serialize, Deserialize, Debug)]
pub struct StationInfo {
    pub id: StationId,
    pub position: SpaceCoord,
}

impl StationInfo {
    // TODO (#8) Based on the scanner rank, get informations on crew and cargo
    pub fn scan(_rank: u8, station: &Station) -> StationInfo {
        StationInfo {
            id: station.id,
            position: station.position,
        }
    }
}

#[derive(Default, Debug, Serialize)]
pub struct StationPlayerData {
    pub idle_crew: Crew,
    pub crew: Crew,
    pub trader: Option<CrewId>,
    pub cargo: ShipCargo,
}

impl StationPlayerData {
    pub fn new() -> StationPlayerData {
        StationPlayerData {
            cargo: ShipCargo::with_capacity(STATION_INIT_CARGO),
            ..Default::default()
        }
    }
}

#[derive(Debug)]
pub struct Station {
    pub id: StationId,
    pub position: SpaceCoord,
    pub shipyard: Vec<Ship>,

    pub player_data: BTreeMap<PlayerId, RwLock<StationPlayerData>>,
}

impl Station {
    pub fn init(id: u16, position: super::SpaceCoord) -> Station {
        Station {
            id,
            position,
            shipyard: Ship::init_shipyard(position),
            player_data: BTreeMap::new(),
        }
    }

    // TODO (#8) Allow to build improvements for the scanner
    pub async fn scan(&self, galaxy: &Galaxy) -> ScanResult {
        galaxy.scan_sector(1, &self.position).await
    }

    pub async fn cargo_price(&self, player: &PlayerId) -> f64 {
        let cap = if let Some(data) = self.player_data.get(player) {
            data.read().await.cargo.capacity
        } else {
            STATION_INIT_CARGO
        };
        CARGO_BASE_PRICE.powf((cap - STATION_INIT_CARGO) / CARGO_PRICE_INCDIV)
    }

    pub async fn buy_cargo(
        &mut self,
        player: &mut Player,
        amnt: &usize,
    ) -> Result<ShipCargo, Errcode> {
        let cost = (*amnt as f64) * self.cargo_price(&player.id).await;
        if cost > player.money {
            return Err(Errcode::NotEnoughMoney(player.money, cost));
        }
        player.money -= cost;
        self.ensure_has_player_data(&player.id).await;
        let mut pd = self.player_data.get(&player.id).unwrap().write().await;
        pd.cargo.capacity += *amnt as f64;
        Ok(pd.cargo.clone())
    }

    pub async fn assign_trader(&mut self, pid: &PlayerId, id: CrewId) -> Result<(), Errcode> {
        self.ensure_has_player_data(pid).await;
        let mut pd = self.player_data.get(pid).unwrap().write().await;
        let Some(cm) = pd.idle_crew.0.remove(&id) else {
            return Err(Errcode::CrewMemberNotIdle(id));
        };

        pd.crew.0.insert(id, cm);
        pd.trader = Some(id);
        Ok(())
    }

    pub async fn onboard_pilot(&mut self, id: CrewId, ship: &mut Ship) -> Result<(), Errcode> {
        self.ensure_has_player_data(&ship.owner).await;
        let mut pd = self.player_data.get(&ship.owner).unwrap().write().await;
        let Some(cm) = pd.idle_crew.0.get(&id) else {
            return Err(Errcode::CrewMemberNotIdle(id));
        };

        if cm.member_type != CrewMemberType::Pilot {
            return Err(Errcode::WrongCrewType(CrewMemberType::Pilot));
        }

        if ship.pilot.is_some() {
            return Err(Errcode::CrewNotNeeded);
        }
        ship.pilot = Some(id);
        ship.crew.0.insert(id, pd.idle_crew.0.remove(&id).unwrap());
        ship.update_perf_stats();
        Ok(())
    }

    pub async fn onboard_operator(
        &mut self,
        id: CrewId,
        ship: &mut Ship,
        modid: &ShipModuleId,
    ) -> Result<(), Errcode> {
        self.ensure_has_player_data(&ship.owner).await;
        let mut pd = self.player_data.get(&ship.owner).unwrap().write().await;
        let Some(cm) = pd.idle_crew.0.get(&id) else {
            return Err(Errcode::CrewMemberNotIdle(id));
        };

        if cm.member_type != CrewMemberType::Operator {
            return Err(Errcode::WrongCrewType(CrewMemberType::Pilot));
        }

        let Some(smod) = ship.modules.get_mut(modid) else {
            return Err(Errcode::NoSuchModule(*modid));
        };
        if !smod.need(&cm.member_type) {
            return Err(Errcode::CrewNotNeeded);
        }
        smod.operator = Some(id);
        ship.crew.0.insert(id, pd.idle_crew.0.remove(&id).unwrap());
        Ok(())
    }

    pub async fn buy_resource(
        &mut self,
        resource: &Resource,
        amnt: f64,
        player: &mut Player,
        market: &Market,
    ) -> Result<MarketTx, Errcode> {
        self.ensure_has_player_data(&player.id).await;
        let mut pd = self.player_data.get(&player.id).unwrap().write().await;
        let Some(trader) = pd.trader else {
            return Err(Errcode::NoTraderAssigned);
        };
        let cm = pd.crew.0.get(&trader).unwrap();
        let can_cargo = pd.cargo.space_for(resource);
        let amnt = amnt.min(can_cargo);
        if amnt == 0.0 {
            return Err(Errcode::BuyNothing);
        }

        let tx = market.buy(cm, resource, amnt).await;
        player.money -= tx.removed_money.unwrap();
        player.score -= tx.removed_money.unwrap();
        let (r, a) = tx.added_cargo.unwrap();
        pd.cargo.add_resource(&r, a);
        Ok(tx)
    }

    pub async fn sell_resource(
        &mut self,
        resource: &Resource,
        amnt: f64,
        player: &mut Player,
        market: &Market,
    ) -> Result<MarketTx, Errcode> {
        self.ensure_has_player_data(&player.id).await;
        let mut pd = self.player_data.get(&player.id).unwrap().write().await;
        let Some(trader) = pd.trader else {
            return Err(Errcode::NoTraderAssigned);
        };
        let cm = pd.crew.0.get(&trader).unwrap();
        let Some(can_cargo) = pd.cargo.resources.get(resource) else {
            return Err(Errcode::SellNothing);
        };
        let amnt = amnt.min(*can_cargo);
        if amnt <= 0.0 {
            return Err(Errcode::SellNothing);
        }

        let tx = market.sell(cm, resource, amnt).await;
        player.money += tx.added_money.unwrap();
        player.score += tx.added_money.unwrap();
        let (r, a) = tx.removed_cargo.unwrap();
        let unloaded = pd.cargo.unload(&r, a);
        debug_assert_eq!(unloaded, a);
        Ok(tx)
    }

    pub async fn refuel_ship(&mut self, ship: &mut Ship) -> Result<f64, Errcode> {
        if self.position != ship.position {
            return Err(Errcode::ShipNotInStation);
        }
        let Some(pd) = self.player_data.get(&ship.owner) else {
            return Err(Errcode::NoFuelInCargo);
        };
        let mut pd = pd.write().await;
        let Some(qty) = pd.cargo.resources.get(&Resource::Fuel) else {
            return Err(Errcode::NoFuelInCargo);
        };
        if *qty == 0.0 {
            return Err(Errcode::NoFuelInCargo);
        }
        debug_assert!(ship.fuel_tank >= 0.0);
        debug_assert!(ship.fuel_tank_capacity >= ship.fuel_tank);
        let needed = ship.fuel_tank_capacity - ship.fuel_tank;
        let unload = needed.min(*qty);
        let unloaded = pd.cargo.unload(&Resource::Fuel, unload);
        ship.fuel_tank += unloaded;
        debug_assert!(ship.fuel_tank_capacity >= ship.fuel_tank);
        Ok(unloaded)
    }

    pub async fn repair_ship(&mut self, ship: &mut Ship) -> Result<f64, Errcode> {
        if self.position != ship.position {
            return Err(Errcode::ShipNotInStation);
        }
        let Some(pd) = self.player_data.get(&ship.owner) else {
            return Err(Errcode::NoHullPlateInCargo);
        };
        let mut pd = pd.write().await;
        let Some(qty) = pd.cargo.resources.get(&Resource::HullPlate) else {
            return Err(Errcode::NoHullPlateInCargo);
        };
        if *qty == 0.0 {
            return Err(Errcode::NoHullPlateInCargo);
        }
        debug_assert!(ship.hull_resistance >= ship.hull_decay);

        let amnt = ship.hull_decay.min(*qty);
        if amnt == 0.0 {
            return Ok(0.0);
        }
        let unloaded = pd.cargo.unload(&Resource::HullPlate, amnt);
        ship.hull_decay -= unloaded;
        debug_assert!(
            ship.hull_resistance >= ship.hull_decay,
            "{} < {}",
            ship.hull_resistance,
            ship.hull_decay
        );
        debug_assert!(ship.hull_decay >= 0.0, "{}", ship.hull_decay);
        debug_assert!(unloaded >= 0.0, "{}", unloaded);
        Ok(unloaded)
    }

    pub fn get_ship_upgrade_price(&self, _ship: &Ship, upgrade: &ShipUpgrade) -> f64 {
        // TODO (#9) Modify price based on station economy metrics
        // TODO (#9) Modify price based on upgrades already installed on the ship
        upgrade.get_price()
    }

    pub async fn get_cargo_potential_price(&self, id: &PlayerId) -> f64 {
        let Some(pd) = self.player_data.get(id) else {
            return 0.0;
        };
        pd.read().await
            .cargo
            .resources
            .iter()
            .map(|(r, amnt)| r.base_price() * amnt)
            .sum()
    }

    pub async fn add_resource(&mut self, id: &PlayerId, resource: &Resource, amnt: f64) -> f64 {
        self.ensure_has_player_data(id).await;
        let mut pd = self.player_data.get(id).unwrap().write().await;
        pd.cargo.add_resource(resource, amnt)
    }

    pub fn buy_ship(&mut self, index: usize) -> Ship {
        // Ship starters, always keep them
        let mut ship = if index < 3 {
            self.shipyard.get(index).unwrap().clone()
        } else {
            let ship = self.shipyard.remove(index);
            self.shipyard.push(Ship::random(self.position));
            ship
        };
        ship.update_perf_stats();
        ship.fuel_tank = ship.fuel_tank_capacity;
        ship
    }

    pub async fn ensure_has_player_data(&mut self, id: &PlayerId) {
        if !self.player_data.contains_key(id) {
            self.player_data.insert(*id, RwLock::new(StationPlayerData::new()));
        }
    }

    pub async fn sum_all_wages(&self, id: &PlayerId) -> f64 {
        let Some(pd) = self.player_data.get(id) else {
            return 0.0;
        };
        let pd = pd.read().await;
        pd.crew.sum_wages() + pd.idle_crew.sum_wages()
    }

    pub async fn upgrade_station_crew(
        &mut self,
        id: &PlayerId,
        money: &mut f64,
        crew: &CrewId,
    ) -> Result<(f64, u8), Errcode> {
        let Some(pd) = self.player_data.get(id) else {
            return Err(Errcode::CrewMemberNotFound(*crew));
        };
        let mut pd = pd.write().await;

        let Some(cm) = pd.crew.0.get_mut(crew) else {
            return Err(Errcode::CrewMemberNotFound(*crew));
        };
        let price = cm.price_next_rank();
        if price > *money {
            return Err(Errcode::NotEnoughMoney(*money, price));
        }
        *money -= price;
        cm.rank += 1;
        Ok((price, cm.rank))
    }

    pub async fn hire_crew(&mut self, id: &PlayerId, crewtype: CrewMemberType) -> CrewId {
        let mut rng = rand::rng();
        let crewid = rng.random();
        let member = CrewMember::from(crewtype);

        self.ensure_has_player_data(id).await;
        let mut pd = self.player_data.get(id).unwrap().write().await;
        pd.idle_crew.0.insert(crewid, member);
        crewid
    }

    pub async fn upgr_trader_price(&self, id: &PlayerId) -> Option<f64> {
        let Some(pd) = self.player_data.get(id) else {
            return None;
        };
        let pd = pd.read().await;
        pd.trader.map(|trader| {
            let cm = pd.crew.0.get(&trader).unwrap();
            cm.price_next_rank()
        })
    }

    pub async fn clone_cargo(&self, id: &PlayerId) -> ShipCargo {
        let Some(pd) = self.player_data.get(id) else {
            return ShipCargo::with_capacity(STATION_INIT_CARGO);
        };
        pd.read().await.cargo.clone()
    }

    pub async fn get_fee_rate(&self, id: &PlayerId) -> Result<f64, Errcode> {
        let Some(pd) = self.player_data.get(id) else {
            return Err(Errcode::NoTraderAssigned);
        };
        let pd = pd.read().await;
        let Some(trader) = pd.trader else {
            return Err(Errcode::NoTraderAssigned);
        };
        let cm = pd.crew.0.get(&trader).unwrap();
        Ok(fee_rate(cm.rank))
    }

    pub async fn to_json(&self, id: &PlayerId) -> serde_json::Value {
        if let Some(pd) = self.player_data.get(id) {
            let pd = pd.read().await;
            self._to_json(&pd)
        } else {
            let pd = StationPlayerData::new();
            self._to_json(&pd)
        }
    }

    fn _to_json(&self, data: &StationPlayerData) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "position": self.position,
            "crew": data.crew,
            "cargo": data.cargo,
            "idle_crew": data.idle_crew,
            "trader": data.trader,
        })
    }
}
