use axum::extract::ws::{Message, WebSocket};
use futures::StreamExt;

use crate::state::SharedState;

pub async fn serve(mut socket: WebSocket, state: SharedState) {
    let mut receiver = state.subscribe();

    loop {
        tokio::select! {
            message = receiver.recv() => {
                match message {
                    Ok(event) => {
                        match serde_json::to_string(&event) {
                            Ok(payload) => {
                                if socket.send(Message::Text(payload.into())).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    Err(_) => break,
                }
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }
}
