use anyhow::{anyhow, Context, Result};
use futures::StreamExt;

use multi_party_ecdsa::protocols::multi_party_ecdsa::gg_2020::state_machine::keygen::{ProtocolMessage, Keygen};
use round_based::async_runtime::AsyncProtocol;

use rocket::http::Status;
use rocket::request::{FromRequest, Outcome, Request};
use serde::{Deserialize, Serialize};
use rocket::serde::json::Json;

mod gg20_sm_client;
use gg20_sm_client::join_computation;

async fn join(
    address: &str,
    room_id: &str,
    party_index: u16,
    threshold: u16,
    number_of_parties: u16
) -> Result<()> {
    let base = surf::Url::parse(address)?;
    let (_i, incoming, outgoing) = join_computation::<ProtocolMessage>(base, room_id)
        .await
        .context("keygen finished")?;

    let incoming = incoming.fuse();
    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let keygen = Keygen::new(party_index, threshold, number_of_parties)?;

    // This out put contains the keys for this party and needs to be saved
    let output = AsyncProtocol::new(keygen, incoming, outgoing)
        .run()
        .await
        .map_err(|e| anyhow!("protocol execution terminated with error: {}", e))?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct KeygenConfig {
    party_index: u16,
    threshold: u16,
    number_of_parties: u16
}

#[rocket::post("/keygen/<room_id>", format = "json", data = "<keygen_config>")]
async fn start_keygen(room_id: &str, keygen_config: Json<KeygenConfig>) {
    join(
        "http://localhost:8000/",
        room_id,
        keygen_config.party_index,
        keygen_config.threshold,
        keygen_config.number_of_parties
    ).await;
}

/// Represents a header Last-Event-ID
struct LastEventId(Option<u16>);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for LastEventId {
    type Error = &'static str;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let header = request
            .headers()
            .get_one("Last-Event-ID")
            .map(|id| id.parse::<u16>());
        match header {
            Some(Ok(last_seen_msg)) => Outcome::Success(LastEventId(Some(last_seen_msg))),
            Some(Err(_parse_err)) => {
                Outcome::Failure((Status::BadRequest, "last seen msg id is not valid"))
            }
            None => Outcome::Success(LastEventId(None)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct IssuedUniqueIdx {
    unique_idx: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let figment = rocket::Config::figment().merge(("port", 8001));

    rocket::custom(figment)
        .mount("/", rocket::routes![start_keygen])
        .launch()
        .await?;
    Ok(())
}
