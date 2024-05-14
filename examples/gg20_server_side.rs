use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt, TryStreamExt};
use std::path::PathBuf;
use structopt::StructOpt;

use curv::arithmetic::Converter;
use curv::BigInt;

use multi_party_ecdsa::protocols::multi_party_ecdsa::gg_2020::state_machine::keygen::{ProtocolMessage, Keygen};
use multi_party_ecdsa::protocols::multi_party_ecdsa::gg_2020::state_machine::sign::{
    OfflineStage, SignManual, OfflineProtocolMessage, PartialSignature
};
use round_based::async_runtime::AsyncProtocol;
use round_based::Msg;

use rocket::http::Status;
use rocket::request::{FromRequest, Outcome, Request};
use serde::{Deserialize, Serialize};
use rocket::serde::json::Json;

mod gg20_sm_client;
use gg20_sm_client::join_computation;

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct KeygenConfig {
    party_index: u16,
    threshold: u16,
    number_of_parties: u16
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct SigningConfig {
    data_to_sign: String,
}

async fn join_keygen(
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
    println!("starting keygen in room {room_id} with index {party_index} threshold {threshold} and n {number_of_parties}");

    let incoming = incoming.fuse();
    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let keygen = Keygen::new(party_index, threshold, number_of_parties)?;

    // TODO: This out put contains the keys for this party and needs to be saved
    let output = AsyncProtocol::new(keygen, incoming, outgoing)
        .run()
        .await
        .map_err(|e| anyhow!("protocol execution terminated with error: {}", e))?;
    Ok(())
}

async fn join_signing(room_id: &str, data_to_sign: String) -> Result<()> {
    // TODO: should be read from the DB
    let local_share = tokio::fs::read("local-share2.json")
        .await
        .context("cannot read local share")?;
    let local_share = serde_json::from_slice(&local_share).context("parse local share")?;
    // TODO end

    // Offline stage start
    let base = surf::Url::parse("http://localhost:8000/")?; // This address should be in the env
    let (i, incoming, outgoing) = join_computation::<OfflineProtocolMessage>(base.clone(), &format!("{}-offline", room_id))
        .await?;

    let incoming = incoming.fuse();
    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let signing = OfflineStage::new(i, [1,2].to_vec(), local_share)?;
    let completed_offline_stage = AsyncProtocol::new(signing, incoming, outgoing)
        .run()
        .await
        .map_err(|e| anyhow!("protocol execution terminated with error: {}", e))?;
    // Offline stage end

    // Online stage start
    let (_i, incoming, outgoing) = join_computation::<PartialSignature>(base, &format!("{}-online", room_id))
        .await?;

    let incoming = incoming.fuse();
    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let (signing, partial_signature) = SignManual::new(
        BigInt::from_bytes(data_to_sign.as_bytes()),
        completed_offline_stage,
    )?;

    outgoing
        .send(Msg {
            sender: i,
            receiver: None,
            body: partial_signature,
        })
        .await?;

    let partial_signatures: Vec<_> = incoming
        .take([1,2].to_vec().len() - 1)
        .map_ok(|msg| msg.body)
        .try_collect()
        .await?;

    let signature = signing
        .complete(&partial_signatures)
        .context("online stage failed")?;
    let r = signature.r.to_bytes();
    let signature = serde_json::to_string(&signature).context("serialize signature")?;
    println!("{}", signature);

    // Online stage end

    Ok(())
}

#[rocket::post("/keygen/<room_id>", format = "json", data = "<keygen_config>")]
async fn start_keygen(room_id: &str, keygen_config: Json<KeygenConfig>) {
    join_keygen(
        "http://localhost:8000/",
        room_id,
        keygen_config.party_index,
        keygen_config.threshold,
        keygen_config.number_of_parties
    ).await;
}

#[rocket::post("/sign/<room_id>", format = "json", data = "<signing_config>")]
async fn start_signing(room_id: &str, signing_config: Json<SigningConfig>) {
    join_signing(room_id, signing_config.data_to_sign.clone()).await;
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
        .mount("/", rocket::routes![start_keygen, start_signing])
        .launch()
        .await?;
    Ok(())
}
