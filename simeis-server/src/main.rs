#![allow(unexpected_cfgs)]
use ntex::web;

use simeis_data::game::{Game, GameSignal};

mod api;

pub type GameState = ntex::web::types::State<Game>;

#[ntex::main]
async fn main() -> std::io::Result<()> {
    #[cfg(not(feature = "testing"))]
    let port = 8080;

    #[cfg(feature = "testing")]
    let port = 9345;

    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .filter_module("ntex_server", log::LevelFilter::Warn)
        .filter_module("ntex_io", log::LevelFilter::Warn)
        .filter_module("ntex_rt", log::LevelFilter::Warn)
        .filter_module("ntex::http::h1", log::LevelFilter::Warn)
        .filter_module("ntex_net::compio::io_impl", log::LevelFilter::Warn)
        .init();

    log::info!("Running on http://0.0.0.0:{port}");
    let (gamethread, state) = Game::init().await;
    let stop_chan = state.send_sig.clone();

    // TODO FIXME Quand worker > 1, et beaucoup de players, je finir par avoir un deadlock
    let res = web::HttpServer::new(async move || {
        let game_state = state.clone();
        web::App::new()
            .middleware(web::middleware::Logger::default())
            .state(game_state)
            .configure(api::configure)
    })
    // .workers(64)
    .workers(1)
    .bind(("0.0.0.0", port))?
    .run()
    .await;
    // 530 OK

    log::info!("Server stopped, stopping game thread");
    stop_chan.send(GameSignal::Stop).await.unwrap();
    gamethread.join().unwrap();
    res
}
