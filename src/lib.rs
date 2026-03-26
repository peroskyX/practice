pub mod alerts;
pub mod config;
pub mod error;
pub mod models;
pub mod routes;
pub mod state;
pub mod ws;

use axum::Router;

pub use config::Config;
pub use state::SharedState;

pub fn build_app(state: SharedState) -> Router {
    routes::router(state)
}
