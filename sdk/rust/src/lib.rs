#![allow(dead_code, unused_variables)]
use serde_json::Value;

use std::path::PathBuf;
use std::time::Duration;
use std::collections::HashMap;

pub fn get_id(data: &Value) -> u64 {
    json_get_uint("id", data).unwrap()
}

pub fn get_dist(posa: (u64, u64, u64), posb: (u64, u64, u64)) -> f64 {
    let xd = (posa.0 as f64) - (posb.0 as f64);
    let yd = (posa.1 as f64) - (posb.1 as f64);
    let zd = (posa.2 as f64) - (posb.2 as f64);
    (xd.powf(2.0) + yd.powf(2.0) + zd.powf(2.0)).sqrt()
}

pub fn get_position(planet: &Value) -> Option<(u64, u64, u64)> {
    let v = json_get_list("position", planet)?;
    match (
        v.first().and_then(|n| n.as_u64()),
        v.get(1).and_then(|n| n.as_u64()),
        v.get(2).and_then(|n| n.as_u64()),
    ) {
        (Some(x), Some(y), Some(z)) => Some((x, y, z)),
        _ => None,
    }
}

pub fn json_get_key<'a>(key: &str, mut val: &'a Value) -> Option<&'a Value> {
    for k in key.split(".") {
        let obj = val.as_object()?;
        val = obj.get(k)?;
    }
    Some(val)
}
pub fn json_get_list<'a>(key: &str, val: &'a Value) -> Option<Vec<&'a Value>> {
    let val = json_get_key(key, val)?.as_array()?;
    Some(val.iter().collect())
}
pub fn json_get_dict<'a>(key: &str, val: &'a Value) -> Option<HashMap<&'a String, &'a Value>> {
    let val = json_get_key(key, val)?.as_object()?;
    let mut res = HashMap::new();
    for (key, val) in val.iter() {
        res.insert(key, val);
    }
    Some(res)
}
pub fn json_get_float(key: &str, val: &Value) -> Option<f64> {
    json_get_key(key, val)?.as_f64()
}
pub fn json_get_string<'a>(key: &str, val: &'a Value) -> Option<&'a str> {
    json_get_key(key, val)?.as_str()
}
pub fn json_get_uint(key: &str, val: &Value) -> Option<u64> {
    json_get_key(key, val)?.as_u64()
}
pub fn json_get_bool(key: &str, val: &Value) -> Option<bool> {
    json_get_key(key, val)?.as_bool()
}

pub type ApiResult = Result<Value, Value>;

pub struct SimeisSDK {
    url: String,
    player_id: Option<u64>,
    player_key: Option<String>,
}

impl SimeisSDK {
    pub fn new<T: ToString>(username: String, ip: T, port: u16) -> SimeisSDK {
        let url = format!("http://{}:{port}", ip.to_string());
        let mut sdk = SimeisSDK {
            url,
            player_id: None,
            player_key: None,
        };
        assert!(sdk.ping());
        sdk.setup_player(username, false);
        sdk
    }

    pub fn get<T: ToString>(&self, path: T) -> ApiResult {
        debug_assert!(path.to_string().starts_with("/"));

        let mut req = ureq::get(format!("{}{}", self.url, path.to_string()));
        if let Some(ref key) = self.player_key {
            req = req.query("key", key);
        }
        let mut got = req.call().map_err(|err| {
            serde_json::json!({
                "status": 0,
                "error": format!("{err:?}"),
            })
        })?;
        let data = got
            .body_mut()
            .read_to_string()
            .expect("Non textual data from server");
        let mut data: Value = serde_json::from_str(&data)
            .expect("Unable to decode JSON from what the server returned");
        let datamap = data.as_object_mut().unwrap();
        let err = datamap
            .remove("error")
            .expect("Missing error field in reply");
        if err != "ok" { Err(data) } else { Ok(data) }
    }

    pub fn ping(&self) -> bool {
        let Ok(got) = self.get("/ping") else {
            return false;
        };
        matches!(json_get_string("ping", &got), Some("pong"))
    }

    fn setup_player(&mut self, mut username: String, force_register: bool) {
        username = username
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();

        let path = PathBuf::from(format!("./{username}.json"));

        let player = if !path.exists() || force_register {
            let player_tmp = self.get(format!("/player/new/{username}")).unwrap();
            let player_json_str = serde_json::to_string(&player_tmp).unwrap();
            std::fs::write(&path, player_json_str).expect("Unable to write player data to path");
            player_tmp
        } else {
            let player_json_str =
                std::fs::read_to_string(&path).expect("Unable to read player data from path");
            serde_json::from_str(&player_json_str).expect("Unable to load player JSON data")
        };

        self.player_key = Some(json_get_string("key", &player).unwrap().to_string());
        self.player_id = Some(json_get_uint("playerId", &player).unwrap());

        let player_id = json_get_uint("playerId", &player).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        let Ok(player_status) = self.get(format!("/player/{player_id}")) else {
            return self.setup_player(username, true);
        };

        let player_money = json_get_float("money", &player_status).unwrap();
        if player_money <= 0.0 {
            println!("!!! Player already lost, please restart the server to reset the game");
            std::process::exit(0);
        }
    }

    pub fn get_player_status(&self) -> ApiResult {
        self.get(format!("/player/{}", self.player_id.unwrap()))
    }

    pub fn get_ship_status(&self, ship_id: u64) -> ApiResult {
        self.get(format!("/ship/{ship_id}"))
    }

    pub fn get_station_status(&self, station_id: u64) -> ApiResult {
        self.get(format!("/station/{station_id}"))
    }

    pub fn shop_list_modules(&self, station_id: u64) -> Result<Vec<Value>, Value> {
        let mut all = self.get(format!("/station/{station_id}/shop/modules"))?;
        let allvec = all.as_array_mut().unwrap();
        allvec.sort_by(|a, b| {
            let pa = json_get_float("price", a).unwrap();
            let pb = json_get_float("price", b).unwrap();
            pa.partial_cmp(&pb).unwrap()
        });
        let Value::Array(data) = all else {
            unreachable!();
        };
        Ok(data)
    }

    pub fn shop_list_ship(&self, station_id: u64) -> Result<Vec<Value>, Value> {
        let all = self.get(format!("/station/{station_id}/shipyard/list"))?;
        let Value::Object(mut omap) = all else {
            unreachable!();
        };
        let Some(Value::Array(mut allvec)) = omap.remove("ships") else {
            unreachable!();
        };
        allvec.sort_by(|a, b| {
            let pa = json_get_float("price", a).unwrap();
            let pb = json_get_float("price", b).unwrap();
            pa.partial_cmp(&pb).unwrap()
        });
        Ok(allvec)
    }

    pub fn buy_ship(&self, station_id: u64, ship_id: u64) -> ApiResult {
        self.get(format!("/station/{station_id}/shipyard/buy/{ship_id}"))
    }

    pub fn buy_module_on_ship(&self, station_id: u64, ship_id: u64, modtype: &str) -> ApiResult {
        self.get(format!(
            "/station/{station_id}/shop/modules/{ship_id}/buy/{}",
            modtype.to_lowercase()
        ))
    }

    pub fn hire_crew(&self, station_id: u64, crewtype: &str) -> ApiResult {
        self.get(format!(
            "/station/{station_id}/crew/hire/{}",
            crewtype.to_lowercase()
        ))
    }

    pub fn assign_crew_to_ship_module(
        &self,
        station_id: u64,
        ship_id: u64,
        crew_id: u64,
        mod_id: u64,
    ) -> ApiResult {
        self.get(format!(
            "/station/{station_id}/crew/assign/{crew_id}/{ship_id}/{mod_id}"
        ))
    }

    pub fn assign_crew_as_ship_pilot(
        &self,
        station_id: u64,
        ship_id: u64,
        pilot_id: u64,
    ) -> ApiResult {
        self.get(format!(
            "/station/{station_id}/crew/assign/{pilot_id}/{ship_id}/pilot"
        ))
    }

    pub fn station_has_trader(&self, station_id: u64) -> Result<bool, Value> {
        let station = self.get_station_status(station_id)?;
        for cm in json_get_list("crew", &station).unwrap() {
            if json_get_string("member_type", cm).unwrap() == "Trader" {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn assign_trader_to_station(&self, station_id: u64, crew_id: u64) -> ApiResult {
        self.get(format!(
            "/station/{station_id}/crew/assign/{crew_id}/trading"
        ))
    }

    pub fn compute_travel_cost(&self, ship_id: u64, position: (u64, u64, u64)) -> ApiResult {
        let (x, y, z) = position;
        self.get(format!("/ship/{ship_id}/travelcost/{x}/{y}/{z}"))
    }

    pub fn travel(&self, ship_id: u64, position: (u64, u64, u64), wait_end: bool) -> ApiResult {
        let (x, y, z) = position;
        let costs = self.get(format!("/ship/{ship_id}/navigate/{x}/{y}/{z}"))?;
        if wait_end {
            let duration = json_get_float("duration", &costs).unwrap();
            std::thread::sleep(Duration::from_secs_f64(duration));
            self.wait_until_ship_idle(ship_id, Duration::from_secs(1))?;
        }
        Ok(costs)
    }

    pub fn wait_until_ship_idle(&self, ship_id: u64, time_sleep: Duration) -> Result<(), Value> {
        let init_ship = self.get_ship_status(ship_id)?;
        let mut idle_state = matches!(json_get_string("state", &init_ship), Some("Idle"));
        while !idle_state {
            std::thread::sleep(time_sleep);
            let ship = self.get_ship_status(ship_id)?;
            idle_state = matches!(json_get_string("state", &ship), Some("Idle"));
        }
        Ok(())
    }
    pub fn buy_plates_for_repair(&self, station_id: u64, ship_id: u64) -> Result<Option<Value>, Value> {
        let ship = self.get_ship_status(ship_id)?;
        let req = json_get_float("hull_decay", &ship).unwrap();

        // Pas besoin
        if req == 0.0 {
            return Ok(None);
        }

        let cargo = self.get_station_resources(station_id)?;
        let amnt_got = cargo.get("HullPlate").cloned().or(Some(0.0)).unwrap();
        if amnt_got < req {
            let need = req - amnt_got; 
            Ok(Some(self.buy_resource(station_id, "HullPlate", need)?))
        } else {
            Ok(None)
        }
    }

    pub fn repair_ship(&self, station_id: u64, ship_id: u64) -> Result<Option<Value>, Value> {
        let ship = self.get_ship_status(ship_id)?;
        let req = json_get_float("hull_decay", &ship).unwrap();

        // Pas besoin
        if req == 0.0 {
            return Ok(None);
        }

        let cargo = self.get_station_resources(station_id)?;
        let amnt_got = cargo.get("HullPlate").cloned().or(Some(0.0)).unwrap();

        if amnt_got > 0.0 {
            Ok(Some(self.get(format!("/station/{station_id}/repair/{ship_id}"))?))
        } else {
            Ok(None)
        }
    }

    pub fn buy_fuel_for_refuel(&self, station_id: u64, ship_id: u64) -> Result<Option<Value>, Value> {
        let ship = self.get_ship_status(ship_id)?;
        let current = json_get_float("fuel_tank", &ship).unwrap();
        let capacity = json_get_float("fuel_tank_capacity", &ship).unwrap();
        let req = capacity - current;

        // Pas besoin
        if req == 0.0 {
            return Ok(None);
        }

        let cargo = self.get_station_resources(station_id)?;
        let amnt_got = cargo.get("Fuel").cloned().or(Some(0.0)).unwrap();
        if amnt_got < req {
            let need = req - amnt_got; 
            Ok(Some(self.buy_resource(station_id, "Fuel", need)?))
        } else {
            Ok(None)
        }
    }
    pub fn refuel_ship(&self, station_id: u64, ship_id: u64) -> Result<Option<Value>, Value> {
        let ship = self.get_ship_status(ship_id)?;
        let current = json_get_float("fuel_tank", &ship).unwrap();
        let capacity = json_get_float("fuel_tank_capacity", &ship).unwrap();
        let req = capacity - current;

        // Pas besoin
        if req == 0.0 {
            return Ok(None);
        }

        let cargo = self.get_station_resources(station_id)?;
        let amnt_got = cargo.get("Fuel").cloned().or(Some(0.0)).unwrap();

        if amnt_got > 0.0 {
            Ok(Some(self.get(format!("/station/{station_id}/refuel/{ship_id}"))?))
        } else {
            Ok(None)
        }
    }
    pub fn scan_planets(&self, station_id: u64) -> Result<Vec<Value>, Value> {
        let station = self.get_station_status(station_id)?;
        let all_scanned = self.get(format!("/station/{station_id}/scan"))?;
        let mut all_planets = json_get_list("planets", &all_scanned).unwrap();
        all_planets.sort_by(|a, b| {
            let stapos = get_position(&station).unwrap();
            let posa = get_position(a).unwrap();
            let dista = get_dist(stapos, posa);
            let posb = get_position(b).unwrap();
            let distb = get_dist(stapos, posb);
            dista.partial_cmp(&distb).unwrap()
        });
        Ok(all_planets.iter().map(|v| (*v).clone()).collect())
    }
    pub fn mine(&self, ship_id: u64) -> ApiResult {
        self.get(format!("/ship/{ship_id}/extraction/start"))
    }
    pub fn return_station_and_unload(&self, station_id: u64, ship_id: u64) -> Result<Vec<Value>, Value> {
        let ship = self.get_ship_status(ship_id)?;
        let station = self.get_station_status(station_id)?;

        let stapos = get_position(&station).unwrap();
        if get_position(&ship).unwrap() != stapos {
            self.travel(ship_id, stapos, true)?;
        }

        let cargo = json_get_dict("cargo.resources", &ship).unwrap();
        let mut results = vec![];
        for (res, amnt) in cargo {
            let amnt = amnt.as_f64().unwrap();
            assert!(amnt > 0.0);
            if amnt == 0.0 {
                continue;
            }
            let got = self.get(format!("/ship/{ship_id}/unload/{res}/{amnt}"))?;
            results.push(got);
        }
        return Ok(results)
    }
    pub fn get_station_resources(&self, station_id: u64) -> Result<HashMap<String, f64>, Value> {
        let station = self.get_station_status(station_id)?;
        let cargo = json_get_key("cargo.resources", &station).unwrap();
        let mut resources = HashMap::new();
        for (res, amnt) in cargo.as_object().unwrap() {
            resources.insert(res.clone(), amnt.as_f64().unwrap());
        }
        Ok(resources)
    }
    pub fn get_market_prices(&self) -> Result<HashMap<String, f64>, Value> {
        let prices = self.get("/market/prices")?;
        let mut resources = HashMap::new();
        for (res, amnt) in prices.as_object().unwrap() {
            resources.insert(res.clone(), amnt.as_f64().unwrap());
        }
        Ok(resources)
    }
    pub fn sell_resource(&self, station_id: u64, resource: &str, amnt: f64) -> ApiResult {
        self.get(format!("/market/{station_id}/sell/{resource}/{amnt}"))
    }
    pub fn buy_resource(&self, station_id: u64, resource: &str, amnt: f64) -> ApiResult {
        self.get(format!("/market/{station_id}/buy/{resource}/{amnt}"))
    }
}
