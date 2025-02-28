use base64::{prelude::BASE64_STANDARD, Engine};
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rand::Rng;

use crate::errors::Errcode;
use crate::galaxy::Galaxy;
use crate::market::{Market, MARKET_CHANGE_SEC};
use crate::player::{Player, PlayerId, PlayerKey};
use crate::ship::ShipState;
use crate::syslog::{SyslogEvent, SyslogFifo, SyslogRecv, SyslogSend};

const ITER_PERIOD: Duration = Duration::from_millis(50);

// TODO (#23) Have a global "inflation" rate for all users, that increases over time
//     Equipment becomes more and more expansive

#[derive(Clone)]
pub struct Game {
    pub players: Arc<RwLock<BTreeMap<PlayerId, Arc<RwLock<Player>>>>>,
    pub player_index: Arc<RwLock<HashMap<PlayerKey, PlayerId>>>,
    pub galaxy: Galaxy,
    pub market: Arc<RwLock<Market>>,
    pub syslog: SyslogSend,
    pub fifo_events: SyslogFifo,
    pub tstart: f64,
    send_stop: Sender<bool>,
}

impl Game {
    pub fn init() -> (JoinHandle<()>, Game) {
        let (send_stop, recv_stop) = std::sync::mpsc::channel();
        let (syssend, sysrecv) = SyslogSend::channel();
        let tstart = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let data = Game {
            send_stop,
            galaxy: Galaxy::init(),
            market: Arc::new(RwLock::new(Market::init())),
            players: Arc::new(RwLock::new(BTreeMap::new())),
            player_index: Arc::new(RwLock::new(HashMap::new())),
            syslog: syssend.clone(),
            fifo_events: sysrecv.fifo.clone(),
            tstart,
        };
        let thread_data = data.clone();

        let thread = std::thread::spawn(move || thread_data.start(recv_stop, sysrecv));
        (thread, data)
    }

    pub fn start(&self, stop: Receiver<bool>, syslog: SyslogRecv) {
        log::debug!("Started thread");
        let sleepmin_iter = ITER_PERIOD;
        let mut last_iter = Instant::now();
        let mut market_last_tick = Instant::now();
        let mut rng = rand::rng();
        while stop.try_recv().is_err_and(|x| x == TryRecvError::Empty) {
            self.threadloop(&mut rng, &mut market_last_tick, &syslog);
            let took = Instant::now() - last_iter;
            log::debug!(
                "Loop took {took:?}, sleeping {:?}",
                sleepmin_iter.saturating_sub(took)
            );
            std::thread::sleep(sleepmin_iter.saturating_sub(took));
            last_iter = Instant::now();
        }
        log::info!("Exiting game thread");
    }

    fn threadloop<R: Rng>(&self, rng: &mut R, mlt: &mut Instant, syslog: &SyslogRecv) {
        let market_change_proba = (mlt.elapsed().as_secs_f64() / MARKET_CHANGE_SEC).min(1.0);
        if rng.random_bool(market_change_proba) {
            self.market.write().unwrap().update_prices(rng);
            *mlt = Instant::now();
        }

        for (player_id, player) in self.players.read().unwrap().iter() {
            let mut player = player.write().unwrap();
            log::debug!("Update player {player_id} money");
            player.update_money(ITER_PERIOD.as_secs_f64());

            let mut deadship = vec![];
            for (id, ship) in player.ships.iter_mut() {
                match ship.state {
                    ShipState::InFlight(..) => {
                        let finished = ship.update_flight(ITER_PERIOD.as_secs_f64());
                        if finished {
                            ship.state = ShipState::Idle;
                            if ship.hull_decay >= ship.hull_decay_capacity {
                                deadship.push(*id);
                                syslog.event(*player_id, SyslogEvent::ShipDestroyed(*id));
                            } else {
                                syslog.event(*player_id, SyslogEvent::ShipFlightFinished(*id));
                            }
                        }
                    }

                    ShipState::Extracting(..) => {
                        let finished = ship.update_extract(ITER_PERIOD.as_secs_f64());
                        if finished {
                            ship.state = ShipState::Idle;
                            syslog.event(*player_id, SyslogEvent::ExtractionStopped(*id));
                        }
                    }
                    _ => {}
                }
            }
            for id in deadship {
                player.ships.remove(&id);
            }
        }
        log::debug!("Update players finished");
        syslog.update();
        log::debug!("Update syslogs finished");
    }

    pub fn stop(self, handle: JoinHandle<()>) {
        log::info!("Asking game thread to exit");
        self.send_stop.send(true).unwrap();
        let _ = handle.join();
        log::info!("Game stopped");
    }

    pub fn new_player<T: ToString>(&self, name: T) -> Result<(PlayerId, String), Errcode> {
        let name = name.to_string();
        for (_, player) in self.players.read().unwrap().iter() {
            if name == player.read().unwrap().name {
                return Err(Errcode::PlayerAlreadyExists(name));
            }
        }

        let station = self.galaxy.init_new_station();
        let player = Player::new(station, name);
        let pid = player.id;
        let key = BASE64_STANDARD.encode(player.key);
        self.player_index
            .write()
            .unwrap()
            .insert(player.key, player.id);
        self.players
            .write()
            .unwrap()
            .insert(player.id, Arc::new(RwLock::new(player)));
        self.syslog.event(&pid, SyslogEvent::GameStarted);
        Ok((pid, key))
    }
}
