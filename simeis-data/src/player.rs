use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::Hasher;
use std::sync::Arc;
use std::time::Instant;

use mea::rwlock::RwLock;
use rand::Rng;

use crate::crew::CrewId;
use crate::errors::Errcode;
use crate::galaxy::station::{Station, StationId};
use crate::ship::module::{ShipModuleId, ShipModuleType};
use crate::ship::upgrade::ShipUpgrade;
use crate::ship::{Ship, ShipId};
use crate::syslog::{SyslogEvent, SyslogRecv};

const INIT_MONEY: f64 = 72000.0;

pub type PlayerId = u16;
pub type PlayerKey = [u8; 128];

// Game state for a single player
pub struct Player {
    pub created: Instant,
    pub id: PlayerId,
    pub key: PlayerKey,
    pub score: f64,
    pub lost: bool,

    pub name: String,
    pub money: f64,
    pub costs: f64,

    pub stations: BTreeMap<StationId, Arc<RwLock<Station>>>,
    pub ships: BTreeMap<ShipId, Ship>,
}

impl Player {
    pub fn new(station: (StationId, Arc<RwLock<Station>>), name: String) -> Player {
        let mut hasher = DefaultHasher::new();
        hasher.write(name.as_bytes());
        let mut rng = rand::rng();
        let mut randbytes = [0; 128];
        rng.fill_bytes(&mut randbytes);

        #[allow(unused_mut)]
        let mut money = INIT_MONEY;

        #[cfg(feature = "testing")]
        if name.starts_with("test-rich") {
            money *= 10000.0;
        }
        let mut stations = BTreeMap::new();
        stations.insert(station.0, station.1);
        Player {
            created: Instant::now(),
            key: randbytes,
            id: (hasher.finish() % (PlayerId::MAX as u64)) as PlayerId,
            lost: false,

            money,
            score: 0.0,
            costs: 0.0,

            name,
            stations,
            ships: BTreeMap::new(),
        }
    }

    // SAFETY Will deadlock if a &mut station exists when this is called
    pub async fn update_costs(&mut self) {
        self.costs = 0.0;

        for station in self.stations.values() {
            // Deadlock because of this
            let station = station.read().await;
            self.costs += station.sum_all_wages(&self.id).await;
        }
        self.costs += self
            .ships
            .values()
            .map(|ship| ship.crew.sum_wages())
            .sum::<f64>();
    }

    pub async fn update_money(&mut self, syslog: &SyslogRecv, tdelta: f64) {
        let before = self.money < (self.costs * 60.0);
        self.money -= self.costs * tdelta;
        let after = self.money < (self.costs * 60.0);
        if after && !before {
            let tleft = std::time::Duration::from_secs_f64(self.money / self.costs);
            syslog.event(self.id, SyslogEvent::LowFunds(tleft)).await;
        }
        if self.money < 0.0 && !self.lost {
            self.lost = true;
            syslog.event(self.id, SyslogEvent::GameLost).await;
            // TODO (#10)  Allow to create a new game with the same name if old one lost
            // TODO (#10)  What to do with its resources, ships, etc...
        }
    }

    pub fn buy_ship(&mut self, station: &mut Station, id: ShipId) -> Result<ShipId, Errcode> {
        let ship_opt = {
            let mut data = None;
            for (n, ship) in station.shipyard.iter().enumerate() {
                if ship.id == id {
                    data = Some((n, ship.compute_price()));
                }
            }
            data
        };

        let Some((index, price)) = ship_opt else {
            return Err(Errcode::ShipNotFound(id));
        };

        if price > self.money {
            return Err(Errcode::NotEnoughMoney(self.money, price));
        }

        let mut ship = station.buy_ship(index);
        let ship_id = ship.id;
        ship.owner = self.id;
        self.money -= price;
        self.ships.insert(id, ship);
        Ok(ship_id)
    }

    pub async fn buy_ship_module(
        &mut self,
        station_id: &StationId,
        ship_id: &ShipId,
        modtype: ShipModuleType,
    ) -> Result<ShipModuleId, Errcode> {
        let Some(station) = self.stations.get(station_id) else {
            return Err(Errcode::NoSuchStation(*station_id));
        };
        let station = station.read().await;

        let Some(ship) = self.ships.get_mut(ship_id) else {
            return Err(Errcode::ShipNotFound(*ship_id));
        };

        if station.position != ship.position {
            return Err(Errcode::ShipNotInStation);
        }

        let price = modtype.get_price_buy();
        if self.money < price {
            return Err(Errcode::NotEnoughMoney(self.money, price));
        }
        self.money -= price;
        let id = (ship.modules.len() + 1) as ShipModuleId;
        ship.modules.insert(id, modtype.new_module());
        Ok(id)
    }

    pub fn buy_ship_upgrade(
        &mut self,
        station: &mut Station,
        ship_id: &ShipId,
        upgrade: &ShipUpgrade,
    ) -> Result<f64, Errcode> {
        let Some(ship) = self.ships.get_mut(ship_id) else {
            return Err(Errcode::ShipNotFound(*ship_id));
        };

        let price = station.get_ship_upgrade_price(ship, upgrade);
        if price > self.money {
            return Err(Errcode::NotEnoughMoney(self.money, price));
        }

        self.money -= price;
        upgrade.install(ship);
        Ok(price)
    }

    pub fn buy_ship_module_upgrade(
        &mut self,
        station: &Station,
        ship_id: &ShipId,
        mod_id: &ShipModuleId,
    ) -> Result<(f64, u8), Errcode> {
        let Some(ship) = self.ships.get_mut(ship_id) else {
            return Err(Errcode::ShipNotFound(*ship_id));
        };
        if ship.position != station.position {
            return Err(Errcode::ShipNotInStation);
        }
        let Some(ref mut module) = ship.modules.get_mut(mod_id) else {
            return Err(Errcode::NoSuchModule(*mod_id));
        };
        let price = module.price_next_rank();
        if price > self.money {
            return Err(Errcode::NotEnoughMoney(self.money, price));
        }

        self.money -= price;
        module.rank += 1;

        Ok((price, module.rank))
    }

    pub fn upgrade_ship_crew(
        &mut self,
        station: &Station,
        ship_id: &ShipId,
        crew_id: &CrewId,
    ) -> Result<(f64, u8), Errcode> {
        let Some(ship) = self.ships.get_mut(ship_id) else {
            return Err(Errcode::ShipNotFound(*ship_id));
        };
        if ship.position != station.position {
            return Err(Errcode::ShipNotInStation);
        }
        let res = {
            let Some(ref mut cm) = ship.crew.0.get_mut(crew_id) else {
                return Err(Errcode::CrewMemberNotFound(*crew_id));
            };

            let price = cm.price_next_rank();
            if price > self.money {
                return Err(Errcode::NotEnoughMoney(self.money, price));
            }

            self.money -= price;
            cm.rank += 1;
            (price, cm.rank)
        };
        ship.update_perf_stats();
        Ok(res)
    }

    pub async fn upgrade_station_crew(
        &mut self,
        station: &mut Station,
        crew_id: &CrewId,
    ) -> Result<(f64, u8), Errcode> {
        station
            .upgrade_station_crew(&self.id, &mut self.money, crew_id)
            .await
    }

    pub async fn find_station<F>(&self, f: F) -> Option<StationId>
        where F: Fn(&Station) -> bool,
    {
        for (id, station) in self.stations.iter() {
            let station = station.read().await;
            if f(&station) {
                return Some(*id);
            }
        }
        None
    }
}
