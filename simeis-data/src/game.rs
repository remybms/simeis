use std::sync::Arc;
use std::time::{Duration, Instant};

use mea::mpsc::{BoundedReceiver, BoundedSender, RecvError};
use mea::rwlock::RwLock;

#[cfg(not(feature = "testing"))]
use mea::mpsc::TryRecvError;

use base64::{prelude::BASE64_STANDARD, Engine};
use rand::{Rng, RngExt};

use crate::errors::Errcode;
use crate::galaxy::station::{Station, StationId};
use crate::galaxy::Galaxy;
use crate::market::{Market, MARKET_CHANGE_SEC};
use crate::player::{Player, PlayerId, PlayerKey};
use crate::ship::ShipState;
use crate::syslog::{SyslogEvent, SyslogFifo, SyslogRecv, SyslogSend};
use crate::utils::ShardedLockedData;

#[cfg(not(feature = "extraspeed"))]
const ITER_PERIOD: Duration = Duration::from_millis(20);

#[cfg(feature = "extraspeed")]
const ITER_PERIOD: Duration = Duration::from_micros(20);

// TODO (#9) Have a global "inflation" rate for all users, that increases over time
//     Equipment becomes more and more expansive

pub enum GameSignal {
    Stop,
    Tick,
}

#[derive(Clone)]
pub struct Game {
    pub players: Arc<ShardedLockedData<PlayerId, Arc<RwLock<Player>>>>,
    pub taken_names: Arc<RwLock<Vec<String>>>,
    pub player_index: Arc<ShardedLockedData<PlayerKey, PlayerId>>,
    pub galaxy: Arc<RwLock<Galaxy>>,
    pub market: Arc<Market>,
    pub syslog: SyslogSend,
    pub fifo_events: SyslogFifo,
    pub tstart: f64,
    pub send_sig: BoundedSender<GameSignal>,
    pub init_station: (StationId, Arc<RwLock<Station>>),
}

impl Game {
    pub async fn init() -> (std::thread::JoinHandle<()>, Game) {
        let (send_stop, recv_stop) = mea::mpsc::bounded(5);
        let (syssend, sysrecv) = SyslogSend::channel();
        let tstart = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let mut galaxy = Galaxy::init();
        let init_station = galaxy.init_new_station().await;
        let data = Game {
            send_sig: send_stop,
            galaxy: Arc::new(RwLock::new(galaxy)),
            market: Arc::new(Market::init()),
            taken_names: Arc::new(RwLock::new(vec![])),
            players: Arc::new(ShardedLockedData::new(100)),
            player_index: Arc::new(ShardedLockedData::new(100)),
            syslog: syssend.clone(),
            fifo_events: sysrecv.fifo.clone(),
            tstart,
            init_station,
        };

        let thread_data = data.clone();
        let thread = std::thread::spawn(move || {
            let rt = compio::runtime::Runtime::new().unwrap();
            rt.block_on(thread_data.start(recv_stop, sysrecv));
            rt.run();
        });
        // TODO Reduce stack size from this task, > 1024
        (thread, data)
    }

    #[allow(unused_variables, unused_mut)]
    pub async fn start(&self, mut stop: BoundedReceiver<GameSignal>, syslog: SyslogRecv) {
        log::info!("Game thread started");
        let sleepmin_iter = ITER_PERIOD;
        let mut last_iter = Instant::now();
        let mut market_last_tick = Instant::now();
        let mut rng: rand::rngs::SmallRng = rand::make_rng();

        'main: loop {
            #[cfg(feature = "testing")]
            let got = stop.recv().await;

            #[cfg(not(feature = "testing"))]
            let got = match stop.try_recv() {
                Ok(res) => Ok(res),
                Err(TryRecvError::Empty) => Ok(GameSignal::Tick),
                Err(TryRecvError::Disconnected) => {
                    log::error!("Can't get next tick / stop signal: disconnected");
                    Err(RecvError::Disconnected)
                }
            };

            match got {
                Ok(GameSignal::Tick) => {
                    self.threadloop(&mut rng, &mut market_last_tick, &syslog)
                        .await;

                    #[cfg(not(feature = "testing"))]
                    {
                        let took = Instant::now() - last_iter;
                        let t = sleepmin_iter.saturating_sub(took);
                        crate::utils::sleep(t).await;
                        last_iter = Instant::now();
                    }
                }

                Ok(GameSignal::Stop) => break 'main,
                Err(RecvError::Disconnected) => {
                    log::error!("Got disconnected channel in game thread");
                    break 'main;
                }
            }
        }
        log::info!("Exiting game thread");
    }

    async fn threadloop<R: Rng>(&self, rng: &mut R, mlt: &mut Instant, syslog: &SyslogRecv) {
        let market_change_proba = (mlt.elapsed().as_secs_f64() / MARKET_CHANGE_SEC).min(1.0);

        let all_players: Vec<PlayerId> = self.players.get_all_keys().await;
        for player_id in all_players {
            let player = self.players.clone_val(&player_id).await.unwrap();
            let mut player = player.write().await;
            player.update_money(syslog, ITER_PERIOD.as_secs_f64()).await;

            let mut deadship = vec![];
            for (id, ship) in player.ships.iter_mut() {
                match ship.state {
                    ShipState::InFlight(..) => {
                        let finished = ship.update_flight(ITER_PERIOD.as_secs_f64());
                        if finished {
                            ship.state = ShipState::Idle;
                            if ship.hull_decay >= ship.hull_resistance {
                                deadship.push(*id);
                            } else {
                                syslog
                                    .event(player_id, SyslogEvent::ShipFlightFinished(*id))
                                    .await;
                            }
                        }
                    }

                    ShipState::Extracting(..) => {
                        let finished = ship.update_extract(ITER_PERIOD.as_secs_f64());
                        if finished {
                            ship.state = ShipState::Idle;
                            syslog
                                .event(player_id, SyslogEvent::ExtractionStopped(*id))
                                .await;
                        }
                    }
                    _ => {}
                }
            }
            for id in deadship {
                syslog
                    .event(player_id, SyslogEvent::ShipDestroyed(id))
                    .await;
                player.ships.remove(&id);
            }
        }

        if rng.random_bool(market_change_proba) {
            #[cfg(not(feature = "testing"))]
            self.market.update_prices(rng).await;
            *mlt = Instant::now();
        }

        syslog.update().await;
    }

    pub async fn new_player(&self, name: String) -> Result<(PlayerId, String), Errcode> {
        self.taken_names.write().await.push(name.clone());

        let player = Player::new(self.init_station.clone(), name);
        let pid = player.id;
        let key = BASE64_STANDARD.encode(player.key);

        self.player_index.insert(player.key, player.id).await;
        self.players.insert(player.id, Arc::new(RwLock::new(player))).await;
        self.syslog.event(&pid, SyslogEvent::GameStarted).await;
        Ok((pid, key))
    }
}
