use anyhow::{Result, Context};

use rocket::data::ToByteUnit;
use rocket::http::Status;
use rocket::request::{FromRequest, Outcome, Request};
use serde::{Deserialize, Serialize};

mod gg20_sm_client;
use gg20_sm_client::join_computation;

async fn join(
    address: &str,
    room_id: &str,
) -> Result<()> {
    let base = surf::Url::parse(address);

    match base {
        Ok(v) => {
            let computationResult = join_computation::<String>(v, room_id)
                .await
                .context("keygen finished");
            Ok(())
        },
        Err(e) => Ok(()),
    }
}

#[rocket::post("/keygen/<room_id>")]
async fn start_keygen(room_id: &str) {
    println!("starting keygen in room :{room_id}");

    join("http://localhost:8000/", room_id);

    println!("keygen started in room :{room_id}");
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
    let figment = rocket::Config::figment().merge((
        "limits",
        rocket::data::Limits::new().limit("string", 100.megabytes()),
    ));
    rocket::custom(figment)
        .mount("/", rocket::routes![start_keygen])
        .launch()
        .await?;
    Ok(())
}
