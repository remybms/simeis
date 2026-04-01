#![allow(unexpected_cfgs)]
use ntex::web;

use simeis_data::game::{Game, GameSignal};

mod api;

pub type GameState = ntex::web::types::State<Game>;

#[ntex::main]
async fn main() -> std::io::Result<()> {
    // console_subscriber::init();

    #[cfg(not(feature = "testing"))]
    let port = 8080;

    #[cfg(feature = "testing")]
    let port = 9345;

    env_logger::builder()
        .parse_default_env()
        .filter_module("ntex_server", log::LevelFilter::Warn)
        .filter_module("ntex_io", log::LevelFilter::Warn)
        .filter_module("ntex_rt", log::LevelFilter::Warn)
        .filter_module("ntex::http::h1", log::LevelFilter::Warn)
        .init();

    log::info!("Running on http://0.0.0.0:{port}");
    let (gamethread, state) = Game::init().await;
    let stop_state = state.clone();

    let res = web::HttpServer::new(async move || {
        let game_state = state.clone();
        web::App::new()
            .middleware(web::middleware::Logger::default())
            .state(game_state)
            .configure(api::configure)
    })
    // .stop_runtime()
    .bind(("0.0.0.0", port))?
    .run()
    .await;

    log::info!("Server stopped, stopping game thread");
    stop_state.send_sig.send(GameSignal::Stop).await.unwrap();
    gamethread.await.unwrap();
    res
}
